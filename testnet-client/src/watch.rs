//! Vault watcher: follows one vault's coin across the chain by covenant id, re-derives
//! every state change from the transactions it finds (never from state.json after start),
//! and raises an alert the moment a withdrawal is requested.
//!
//! Needs nothing secret: main.rs hands it a `Keys` holding only the `public` block of keys.json.
//!
//! Alerts go to the console, to `<dir>/alerts.log` (raw detail), to `<dir>/alerts.json` (bank
//! words for the phone, through the service: `seen_at` = last good answer from the network,
//! `alerts` newest first, at most ten), and to the program named in the environment variable
//! `VAULT_PUSH_CMD` (called with the alert text as its only argument; point it at a script that
//! posts to ntfy/Pushover for a phone push).
//!
//! `<dir>/watch.json` holds what the watcher believes (state, coin, scan sink) after every good
//! tick; a restart resumes from it and scans the blocks since, so a request made while the
//! watcher was down is still alerted.
//!
//! Trust: `--node <wrpc-url>` pins the node; with two or more urls every question is put to all
//! of them and a disagreement (about the coin, who spent it, or a DAA gap) is an alert. A node
//! whose DAA stops for 60 s is "stalled"; no good answer from any node for 90 s is "silent".
//! The mempool is only a hint: anything first seen there is "(unconfirmed)" until a block
//! holds it, and a losing double-spend rolls the derived state back. Every exit, panics
//! included, prints and pushes "WATCHER DOWN: <reason>".

use crate::autostop::{self, AutoStop};
use crate::contract::{Coin, Dials, HeirClock, State, Vault, after_decline, after_finish, after_spend, heir_clock, kas, spk_hash};
use crate::network::Network;
use crate::keys::Public;
use crate::tx::{Ctx, coin_of, connect};
use futures::FutureExt;
use kaspa_addresses::Address;
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::tx::{ScriptPublicKey, Transaction, TransactionOutpoint, TransactionOutput, UtxoEntry};
use kaspa_wrpc_client::prelude::*;
use serde::{Deserialize, Serialize};
use silverscript_abi::SilAbiArtifact;
use std::collections::VecDeque;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Notify;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POLL: Duration = Duration::from_secs(3);
/// How many recent sink hashes to keep; the oldest is where a block scan starts.
const SINK_HISTORY: usize = 4;
/// A node whose virtual DAA score has not moved for this long is stalled (or lying).
const STALL: Duration = Duration::from_secs(60);
const HEARTBEAT: Duration = Duration::from_secs(600);
/// A coin that left and whose spender was never found is re-alerted this often.
const LOST_REALERT: Duration = Duration::from_secs(180);
/// v5 auto-heir: an `inheritOpen` this watcher sent is not sent again before this much time has
/// passed and the mempool no longer holds it (the coin still being there).
const OPEN_RETRY: Duration = Duration::from_secs(600);
/// A mempool sighting with no block behind it after this long is worth another alert.
const UNCONFIRMED_WARN: Duration = Duration::from_secs(120);
/// Two honest nodes sit within a few DAA of each other; a minute apart is a stall or a lie.
const NODE_DAA_GAP: u64 = 600;
/// Ticks in a row two nodes may disagree about the coin before it is an alert (propagation).
const DISAGREE_TICKS: u32 = 3;
/// Looks in a row one node may show a spender the others do not before it is an alert (item 10):
/// about half a minute at POLL, room for a slow node, far inside any notice period.
const WAITING_TICKS: u32 = 10;
/// Foreign coins at the vault address listed before the log stops naming them (dust spam).
const MAX_FOREIGN: usize = 100;
/// Item 9: pages of blocks (and of the virtual chain) one catch-up walk may take before it stops and says so.
const SCAN_PAGES_MAX: usize = 400;
/// Item 8: confirmations the node must count before a spend is final; None = accepted by the virtual chain once.
const ACCEPT_CONFIRMATIONS: Option<u64> = None;
/// No good answer from any node for this long is "the guard cannot reach the network".
const SILENT: Duration = Duration::from_secs(90);
/// A network fault that goes on (a stalled node, two nodes far apart) is said when it starts
/// and then again only this often.
const REMIND: Duration = Duration::from_secs(3600);
/// How long one node call may take: past it the node counts as not answered this tick.
const NODE_WAIT: Duration = Duration::from_secs(10);
/// One node quiet while another still answers is an alert only after this long: nodes drop
/// and return with every phone sleep and network switch, and that is not news.
const UNCHECKED: Duration = Duration::from_secs(3600);
/// The CLI watchdog: no good answer for this long (guard or racer) and the process exits with
/// RESTART_CODE so its restart loop starts it again on fresh connections.
const STALE: u64 = 90;
const RESTART_CODE: i32 = 4;
/// Unix seconds of the last good answer: the guard's, and the racer's (0 = no racer running).
static GUARD_OK: AtomicU64 = AtomicU64::new(0);
pub(super) static RACER_OK: AtomicU64 = AtomicU64::new(0);
/// Alerts kept in alerts.json (the phone shows the newest first).
const ALERTS_KEPT: usize = 10;
/// Empty ticks before "the guard cannot find the savings coin" when nothing is in view.
const UNWATCHED_TICKS: u32 = 3;

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn stamp() -> String {
    let secs = unix_now();
    // Plain UTC clock, enough for a log line.
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}Z")
}

/// Parses a signature script into its data pushes. A truncated push or any non-push opcode
/// makes the whole script unreadable (None): the watcher fails closed and alerts.
fn pushes(script: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = vec![];
    let mut i = 0;
    while i < script.len() {
        let op = script[i];
        i += 1;
        let n = match op {
            0x00 => 0,
            0x01..=0x4b => op as usize,
            0x4c => {
                let n = *script.get(i)? as usize;
                i += 1;
                n
            }
            0x4d => {
                let n = u16::from_le_bytes([*script.get(i)?, *script.get(i + 1)?]) as usize;
                i += 2;
                n
            }
            0x4e => {
                let n = u32::from_le_bytes([*script.get(i)?, *script.get(i + 1)?, *script.get(i + 2)?, *script.get(i + 3)?]) as usize;
                i += 4;
                n
            }
            0x4f => {
                out.push(vec![0x81]);
                continue;
            }
            0x51..=0x60 => {
                out.push(vec![op - 0x50]);
                continue;
            }
            _ => return None,
        };
        out.push(script.get(i..i.checked_add(n)?)?.to_vec());
        i += n;
    }
    Some(out)
}

/// Script-number decoding: little-endian, sign bit in the last byte. More than 8 bytes is unreadable.
fn script_int(b: &[u8]) -> Option<i64> {
    if b.is_empty() {
        return Some(0);
    }
    if b.len() > 8 {
        return None;
    }
    let mut v: i64 = 0;
    for (i, &x) in b.iter().enumerate() {
        let x = if i == b.len() - 1 { x & 0x7f } else { x };
        v |= (x as i64) << (8 * i);
    }
    Some(if b[b.len() - 1] & 0x80 != 0 { -v } else { v })
}

/// A script public key as the contract sees it: two version bytes then the script.
fn decode_spk(b: &[u8]) -> Option<ScriptPublicKey> {
    (b.len() >= 2).then(|| ScriptPublicKey::from_vec(u16::from_be_bytes([b[0], b[1]]), b[2..].to_vec()))
}

/// `request(sig, newDestSpk, newAmount)` pushes: [sig, spk, amount, tag, bytecode].
fn decode_request(p: &[Vec<u8>]) -> Option<(ScriptPublicKey, i64)> {
    (p.len() >= 4).then(|| Some((decode_spk(&p[1])?, script_int(&p[2])?))).flatten()
}

/// `cancel(sig, which)` pushes: [sig, which, tag, bytecode]; the contract counts stop slots from 0.
fn decode_cancel_slot(p: &[Vec<u8>]) -> Option<i64> {
    (p.len() >= 4).then(|| script_int(&p[p.len() - 3])).flatten()
}

/// The address a script pays, with the folder's own prefix; a script that is no address is shown as hex.
pub fn addr_of(net: &Network, spk: &ScriptPublicKey) -> String {
    net.address_of(spk).map(|a| a.to_string()).unwrap_or_else(|| format!("script {}", faster_hex::hex_string(spk.script())))
}

fn minutes(net: &Network, daa: i64) -> f64 {
    daa as f64 / net.daa_per_sec / 60.0
}

/// The guard needs two different witnesses, so one lying node can never move it alone.
pub fn check_nodes(nodes: &[String]) -> Result<(), String> {
    let mut distinct: Vec<&str> = nodes.iter().map(|s| s.trim()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < 2 {
        return Err(format!("the guard needs at least two different --node urls (your own node and one more) so a lying node can be caught; {} given. Add --node wss://<host>:<port> for each (plain ws:// only for this machine's own node).", nodes.len()));
    }
    Ok(())
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into())
}

static LOG_FAILED: AtomicBool = AtomicBool::new(false);

fn log_line(dir: &Path, line: &str) {
    match std::fs::OpenOptions::new().create(true).append(true).open(dir.join("alerts.log")) {
        Ok(mut f) => {
            let _ = writeln!(f, "{line}");
        }
        Err(e) => {
            if !LOG_FAILED.swap(true, Ordering::Relaxed) {
                println!("{} ALERT alerts.log cannot be opened ({e}); alerts stay on the console only", stamp());
            }
        }
    }
}

/// How long the push hook may run before it is ended.
const PUSH_WAIT: Duration = Duration::from_secs(20);

/// Hands the alert text to the push hook on a thread of its own, so a slow or hung hook
/// never holds the guard's tick.
fn push(text: &str) {
    if let Ok(cmd) = std::env::var("VAULT_PUSH_CMD") {
        let text = text.to_string();
        std::thread::spawn(move || run_hook(&cmd, &text, PUSH_WAIT));
    }
}

/// Runs the hook and waits at most `wait` for it; one still running then is ended. A hook
/// that fails, or is ended, is itself printed at alert level. True when it succeeded.
fn run_hook(cmd: &str, text: &str, wait: Duration) -> bool {
    let mut child = match std::process::Command::new(cmd).arg(text).spawn() {
        Ok(child) => child,
        Err(e) => {
            println!("{} ALERT push hook {cmd} failed: {e}", stamp());
            return false;
        }
    };
    let began = Instant::now();
    let ended = loop {
        match child.try_wait() {
            Ok(None) if began.elapsed() < wait => std::thread::sleep(Duration::from_millis(100)),
            other => break other,
        }
    };
    match ended {
        Ok(Some(status)) if status.success() => true,
        Ok(Some(status)) => {
            println!("{} ALERT push hook {cmd} failed: {status}", stamp());
            false
        }
        other => {
            let _ = child.kill();
            let _ = child.wait();
            println!("{} ALERT push hook {cmd} ended after {} s: {other:?}", stamp(), wait.as_secs());
            false
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Level {
    Info,
    Alert,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Seen {
    Mempool,
    /// In a block a node showed, but not (yet) accepted by its virtual chain: provisional, like the mempool.
    Block,
    /// In a block the virtual chain accepts: final.
    Accepted,
}

enum Tick {
    Go,
    Done(String),
}

/// The alert kinds the phone knows (the field contract the phone reads).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    WithdrawalPending,
    WithdrawalCompleted,
    LineageMismatch,
    WatcherSilent,
    /// One node has been quiet for UNCHECKED while another answers: nothing to check the
    /// lone node's word against. The phone rings for this one; it never rings for WatcherSilent.
    Unchecked,
    Unwatched,
    Stopped,
    /// v5: a stop key sent a waiting withdrawal back to Locked; the account goes on.
    WithdrawalDeclined,
    /// v5: the heir's silence clock (90% warning, the inheritOpen this watcher sent).
    HeirClock,
    /// The spend key paid an address this laptop knows (its own cold or heir
    /// address, or one a command here has paid before, per state.json history).
    Spend,
    /// The spend key paid an address this laptop has never paid; the loud one.
    SpendStranger,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::WithdrawalPending => "withdrawal_pending",
            Kind::WithdrawalCompleted => "withdrawal_completed",
            Kind::LineageMismatch => "lineage_mismatch",
            Kind::WatcherSilent => "watcher_silent",
            Kind::Unchecked => "unchecked",
            Kind::Unwatched => "unwatched",
            Kind::Stopped => "stopped",
            Kind::WithdrawalDeclined => "withdrawal_declined",
            Kind::HeirClock => "heir_clock",
            Kind::Spend => "spend",
            Kind::SpendStranger => "spend_stranger",
        }
    }
}

/// Whether an alert kind (as alerts.json names it) reports the state of the network links
/// and not a move of the funds.
fn about_network(kind: &str) -> bool {
    [Kind::LineageMismatch, Kind::WatcherSilent, Kind::Unchecked, Kind::Unwatched].iter().any(|k| k.as_str() == kind)
}

/// Whether a network fault that is on now should be said: at its start (no sooner than
/// STALL after the last saying, so a fault that flickers stays quiet), then every REMIND.
fn say_fault(was_on: bool, since_said: Option<Duration>) -> bool {
    match since_said {
        None => true,
        Some(gone) => gone >= REMIND || (!was_on && gone >= STALL),
    }
}

// Bank words the phone shows verbatim.
const STOP_HINT: &str = "Press the emergency stop if you did not start this.";
const TWO_ANSWERS: &str = "The network gave two different answers. Press the emergency stop if you did not start this.";
const NO_COIN: &str = "The guard cannot find the savings coin. Check the wallet device.";
const NO_NET: &str = "The guard cannot reach the network.";
const ONE_QUIET: &str =
    "The guard can reach only part of the network and cannot cross-check what it sees.";
const RULES_BROKEN: &str = "The savings moved in a way the rules do not allow. Check by hand. Press the emergency stop if you did not start this.";
const STRANGER_HINT: &str = "The wallet device has never paid that address. Press the emergency stop if you did not start this.";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Alert {
    at: u64,
    kind: String,
    text: String,
}

/// `<dir>/alerts.json`, what the phone reads through the service: `seen_at` is the last good
/// answer from the network (null before the first, frozen while the watcher is silent) and
/// `alerts` is newest first, at most ALERTS_KEPT.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct AlertFile {
    seen_at: Option<u64>,
    alerts: Vec<Alert>,
}

static FILE_FAILED: AtomicBool = AtomicBool::new(false);

/// Writes `name` whole through a temp file and a rename, so a reader never sees half a file.
fn write_json<T: Serialize>(dir: &Path, name: &str, value: &T) {
    let tmp = dir.join(format!("{name}.tmp"));
    let done = serde_json::to_string_pretty(value)
        .map_err(|e| e.to_string())
        .and_then(|s| std::fs::write(&tmp, s).map_err(|e| e.to_string()))
        .and_then(|_| std::fs::rename(&tmp, dir.join(name)).map_err(|e| e.to_string()));
    if let Err(e) = done
        && !FILE_FAILED.swap(true, Ordering::Relaxed) {
            println!("{} ALERT {name} cannot be written ({e}); the phone will not see this watcher", stamp());
        }
}

fn read_json<T: for<'de> Deserialize<'de>>(dir: &Path, name: &str) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(name)).ok()?).ok()
}

impl AlertFile {
    fn load(dir: &Path) -> Self {
        read_json(dir, "alerts.json").unwrap_or_default()
    }

    /// Newest first, at most ALERTS_KEPT. When the list is full the oldest alert about the
    /// network goes first; an alert about the funds leaves only when every kept alert is one.
    fn add(&mut self, kind: Kind, text: &str) {
        self.alerts.insert(0, Alert { at: unix_now(), kind: kind.as_str().into(), text: text.into() });
        if self.alerts.len() > ALERTS_KEPT {
            let oldest_network = self.alerts.iter().rposition(|a| about_network(&a.kind));
            self.alerts.remove(oldest_network.unwrap_or(self.alerts.len() - 1));
        }
        self.alerts.truncate(ALERTS_KEPT);
    }

    fn save(&self, dir: &Path) {
        write_json(dir, "alerts.json", self);
    }
}

/// The wall-clock rule: no good answer from any node for SILENT is an alert, said once per
/// silence; the first good answer after it is a recovery. `now` is time since the watcher
/// started, handed in so a test can drive the clock.
struct Liveness {
    last_good: Duration,
    silent: bool,
    /// How long without a good answer counts as silence.
    after: Duration,
}

impl Liveness {
    fn new() -> Self {
        Liveness::after(SILENT)
    }

    fn after(after: Duration) -> Self {
        Liveness { last_good: Duration::ZERO, silent: false, after }
    }

    /// A good answer; when it ends a silence, how long the silence lasted.
    fn good(&mut self, now: Duration) -> Option<Duration> {
        let gap = now.saturating_sub(self.last_good);
        self.last_good = now;
        std::mem::replace(&mut self.silent, false).then_some(gap)
    }

    /// A failed answer; true the moment the silence becomes an alert.
    fn bad(&mut self, now: Duration) -> bool {
        if !self.silent && now.saturating_sub(self.last_good) >= self.after {
            self.silent = true;
            return true;
        }
        false
    }
}

/// Every node is a witness. With two or more, one that stops answering while another still
/// does leaves the guard following a lone node's word with nothing to check it against, so
/// each node keeps its own UNCHECKED clock. `down[i]` = node i failed a question this tick.
/// Answers (nodes whose silence just became an alert, nodes that just came back with the gap).
fn quiet_witnesses(
    live: &mut [Liveness],
    down: &[bool],
    now: Duration,
) -> (Vec<usize>, Vec<(usize, Duration)>) {
    let (mut quiet, mut back) = (vec![], vec![]);
    if live.len() < 2 {
        return (quiet, back); // a lone node's silence is the all-nodes rule above
    }
    for (i, l) in live.iter_mut().enumerate() {
        if down.get(i).copied().unwrap_or(true) {
            if l.bad(now) {
                quiet.push(i);
            }
        } else if let Some(gap) = l.good(now) {
            back.push((i, gap));
        }
    }
    (quiet, back)
}

/// `<dir>/watch.json`: what the watcher believes, written after every good tick so a restart
/// resumes from it. `sink` is the oldest sink kept, where the block scan for a missed spend starts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Saved {
    state: State,
    /// The coin last seen: outpoint, amount, the DAA score of its block.
    coin: Option<(TransactionOutpoint, u64, u64)>,
    sink: Option<Hash>,
    daa: u64,
    closed: Option<String>,
    /// v5: the `inheritOpen` this watcher sent (txid), so a restart does not send it twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open_sent: Option<String>,
    /// Item 11: a spend seen but not yet accepted, with the state and coin to fall back to if it vanishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provisional: Option<Provisional>,
    /// The covenant id this file follows: a reopened account has a new one, and its guard must not
    /// resume the old account's record (it would end at once, "the account is closed").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cov: Option<Hash>,
}

/// Whether watch.json belongs to the account on disk. Files from before `cov` was kept fall back
/// to the stage: a closed record while the account on disk is open is the old account's.
fn same_account(s: &Saved, v: &Vault) -> bool {
    match s.cov {
        Some(c) => c == v.covenant_id,
        None => !(s.state.stage == 9 && v.state.stage != 9),
    }
}

/// The rollback of a provisional spend, as watch.json keeps it: the state and coin before it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Provisional {
    txid: String,
    prev_state: State,
    prev_coin: (TransactionOutpoint, u64, u64),
    low: Hash,
}

impl Saved {
    fn load(dir: &Path) -> Option<Self> {
        read_json(dir, "watch.json")
    }

    fn save(&self, dir: &Path) {
        write_json(dir, "watch.json", self);
    }
}

/// What a spending transaction did to the vault, before it is applied.
struct Outcome {
    entry: String,
    level: Level,
    kind: Kind,
    /// Bank words for alerts.json (empty for an info outcome).
    bank: String,
    text: String,
    next: Option<State>,
    closed: bool,
    /// What a watched folder's state.json records for this move (watched-follow mode); None for a bad shape.
    rec: Option<Move>,
}

/// One followed move of the covenant, as `follow_record` writes it into a watched account's state.json.
#[derive(Clone, Debug)]
pub(crate) struct Move {
    pub entry: String,
    /// The state the move leads to (None: the coin left the covenant, or a `finish` that emptied it).
    pub next: Option<State>,
    /// The coin ended here (cancel, inherit, a finish that emptied the vault).
    pub closed: bool,
    /// A request's destination; for every other move the address output 0 paid.
    pub to: String,
    /// Sompi: the amount requested, paid, sent, or (topup) added.
    pub moved: u64,
    /// The fee when the vault coin was the only input; None when other inputs carried it.
    pub fee: Option<u64>,
    pub txid: String,
    /// The stop slot (0-based) a cancel or decline was signed with, when readable.
    pub slot: Option<i64>,
    /// The vault's delay dial, for the request line.
    pub delay: i64,
    pub heir: String,
}

/// The end of a `finish` line whose withdrawal emptied the vault (watched-follow mode; watched.rs reads it).
pub(crate) const EMPTIED: &str = "; the vault is now empty";

/// Watched-follow mode: the vault record after a followed move. Same history words as the wallet's own
/// commands (commands.rs), so `contract::is_stopped` and `watched::map_status` read them; `pending_to` is the
/// request's destination while Pending, else None. A line naming the same tx is never added twice (a
/// restart that follows the same move again). Pure.
#[path = "watch_race.rs"]
mod race;

pub(crate) fn follow_record(old: &Vault, m: &Move) -> Vault {
    let mut v = old.clone();
    let id = &m.txid;
    let fee = m.fee.map(|f| format!("{:.5} KAS", kas(f))).unwrap_or_else(|| "unknown".into());
    let which = m.slot.map(|s| (s + 1).to_string()).unwrap_or_else(|| "?".into());
    let dest = old.pending_to.clone().unwrap_or_else(|| "the requested address".into());
    let pending = kas(old.state.amount.max(0) as u64);
    let line = match m.entry.as_str() {
        "spend" => format!("spend {} KAS to {}, fee {fee}, tx {id}", kas(m.moved), m.to),
        "request" => format!("request {} KAS to {}, fee {fee}, tx {id}; finish allowed after {} DAA", kas(m.moved), m.to, m.delay),
        "finish" => format!("finish {} KAS to {}, fee {fee}, tx {id}{}", kas(m.moved), m.to, if m.closed { EMPTIED } else { "" }),
        "decline" => format!("decline (stop key {which}): the request for {pending} KAS to {dest} is withdrawn, the vault is Locked again, fee {fee}, tx {id}"),
        "cancel" => format!("cancel (stop key {which}): everything to cold, fee {fee}, tx {id}"),
        "topup" => format!("topup {} KAS, tx {id}", kas(m.moved)),
        "inherit" | "inheritOpen" if old.state.stage == 1 => {
            format!("{} from Pending: {pending} KAS to {dest} (the pending request), the rest to heir {}, fee {fee}, tx {id}", m.entry, m.heir)
        }
        "inherit" | "inheritOpen" => format!("{}: everything to heir {}, fee {fee}, tx {id}", m.entry, m.heir),
        other => format!("{other}, tx {id}"),
    };
    match (&m.next, m.entry.as_str()) {
        (Some(n), _) => v.state = n.clone(),
        // a finish that emptied the vault: back to Locked, like the wallet's own finish writes
        (None, "finish") => v.state = after_finish(&old.state),
        _ => {}
    }
    v.pending_to = match m.entry.as_str() {
        "request" => Some(m.to.clone()),
        _ if v.state.stage == 1 && !m.closed => old.pending_to.clone(),
        _ => None,
    };
    if !old.history.iter().any(|h| h.contains(&format!("tx {id}"))) {
        v.history.push(line);
    }
    v
}

/// A spend first seen in the mempool: not final until a block holds it.
#[derive(Clone)]
struct Unconfirmed {
    txid: String,
    prev: Coin,
    prev_state: State,
    low: Hash,
    since: Instant,
    level: Level,
    kind: Kind,
    bank: String,
    text: String,
    warned: bool,
}

struct Watcher<'a> {
    /// The contract source, read once at the start. Every script the guard works out comes
    /// from this and `dials`, so no tick depends on a file being readable at that moment.
    source: String,
    ctx: &'a Ctx,
    /// The vault's dials (state.json; the network's preset when the folder holds no vault yet).
    dials: Dials,
    primary: String,
    /// Every `--node` after the first; all are asked and must agree.
    others: Vec<(String, KaspaRpcClient)>,
    cov: Hash,
    state: State,
    coin: Option<Coin>,
    sinks: VecDeque<Hash>,
    /// Item 9: the catch-up cursor: the saved sink after a restart, or where the last capped walk
    /// stopped. It is not part of the sink history, so a long walk is never forgotten mid-way.
    scan_from: Option<Hash>,
    misses: u32,
    last_countdown: Option<i64>,
    foreign_seen: Vec<TransactionOutpoint>,
    unconfirmed: Option<Unconfirmed>,
    /// A coin that vanished without a findable spender: (outpoint, last alert).
    lost: Option<(TransactionOutpoint, Instant)>,
    /// Nothing in view and nothing to resume from: last "unwatched" alert.
    unwatched_said: Option<Instant>,
    closed: Option<String>,
    daa_seen: (u64, Instant),
    stall_said: Option<Instant>,
    gap_said: Option<Instant>,
    /// Whether the last tick saw two nodes more than NODE_DAA_GAP apart.
    gap_on: bool,
    disagree: u32,
    /// Looks in a row that one node showed a spender the others did not (Verdict::Waiting).
    waiting: u32,
    /// The lowest DAA any node reported this tick: the `finish` timing check trusts no one node.
    daa_low: u64,
    last_heartbeat: Instant,
    started: Instant,
    alerts: AlertFile,
    live: Liveness,
    /// One UNCHECKED clock per node, in `nodes()` order (quiet_witnesses).
    witness: Vec<Liveness>,
    /// v5 `--auto-heir`: send the inheritOpen once the silence has passed.
    auto_heir: bool,
    /// The inheritOpen this watcher sent: txid and when.
    open_sent: Option<(String, Instant)>,
    heir_warned: bool,
    /// Woken after every saved alert and every good answer (`guard.wait` on the phone).
    woke: Arc<Notify>,
    /// Watched-follow mode (the folder holds watched.json, watched.rs): every final move is written into the
    /// folder's state.json (`follow_record`). Never true for a wallet's own folder.
    follow: bool,
    /// Moves seen but not yet accepted by the chain; written once confirmed, dropped on a roll-back.
    follow_wait: Vec<Move>,
    /// `--auto-stop` (the public challenge): answer waiting withdrawals with the held stop key.
    auto_stop: Option<AutoStop>,
    /// The stop this watcher sent: txid, the waiting coin it spends, when, sends for that coin.
    stop_sent: Option<(String, TransactionOutpoint, Instant, u32)>,
    /// The racer: the sink when its coin was last seen unspent (a block search starts there).
    race_low: Option<Hash>,
    /// Racer ticks in a row without finding the followed coin or its spender.
    race_lost: u32,
    /// The racer: when the challenge ends (unix seconds); then it sends everything to the vault.
    race_end: Option<u64>,
}

/// One node call capped at `wait`: a node that hangs has failed, and never holds the tick.
async fn in_time_for<T, E: std::fmt::Display>(
    wait: Duration,
    call: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match tokio::time::timeout(wait, call).await {
        Ok(answer) => answer.map_err(|e| e.to_string()),
        Err(_) => Err(format!("no answer in {} s", wait.as_secs())),
    }
}

/// One node call capped at NODE_WAIT.
async fn in_time<T, E: std::fmt::Display>(
    call: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
    in_time_for(NODE_WAIT, call).await
}

async fn fetch_coins(rpc: &KaspaRpcClient, addr: &Address, cov: Hash) -> Result<(Vec<Coin>, Vec<Coin>), String> {
    let entries = in_time(rpc.get_utxos_by_addresses(vec![addr.clone()])).await?;
    Ok(entries.into_iter().map(coin_of).partition(|(_, u)| u.covenant_id == Some(cov)))
}

fn spends(tx: &RpcTransaction, op: &TransactionOutpoint) -> bool {
    tx.inputs.iter().any(|i| i.previous_outpoint.transaction_id == op.transaction_id && i.previous_outpoint.index == op.index)
}

fn txid_of(tx: &RpcTransaction) -> String {
    Transaction::try_from(tx.clone()).map(|t| t.id().to_string()).unwrap_or_else(|_| "unknown-id".into())
}

/// Item 9: where a paged walk (blocks, or the virtual chain) continues from: the newest hash the
/// page brought past `cur`, or None when it brought nothing new (the tip is reached).
fn next_page(hashes: &[Hash], cur: Hash) -> Option<Hash> {
    hashes.iter().rev().find(|h| **h != cur).copied()
}

/// Item 9: what a paged walk does after page `n` brought `hashes`: fetch the next page, stop at the
/// tip, or stop at the page cap and remember where the next tick continues.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Next(Hash),
    Tip,
    Capped(Hash),
}

fn step(hashes: &[Hash], page: Hash, n: usize) -> Step {
    match next_page(hashes, page) {
        None => Step::Tip,
        Some(h) if n < SCAN_PAGES_MAX => Step::Next(h),
        Some(h) => Step::Capped(h),
    }
}

/// Item 8: whether the spender `txid` is among the transaction ids the virtual chain accepted.
fn is_accepted<'a>(txid: &str, accepted: impl IntoIterator<Item = &'a Hash>) -> bool {
    accepted.into_iter().any(|h| h.to_string() == txid)
}

/// Item 10: what the nodes together say about who spent a coin.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Verdict {
    /// No node has seen a spender.
    Nothing,
    /// Some have, some have not yet: hold, one node's word moves nothing.
    Waiting,
    /// Two nodes name two different spenders: alert and hold.
    Split,
    /// Every reachable node names the same spender.
    Agreed,
}

/// Item 10: `ids` are the spender ids the nodes that found one reported; `waiting` is how many
/// reachable nodes found none.
fn verdict(ids: &[String], waiting: usize) -> Verdict {
    if ids.is_empty() {
        Verdict::Nothing
    } else if ids.windows(2).any(|w| w[0] != w[1]) {
        Verdict::Split
    } else if waiting > 0 {
        Verdict::Waiting
    } else {
        Verdict::Agreed
    }
}

/// Item 9/10: where the next tick's walk starts after this one from `low`: nowhere (None) when the
/// spender is agreed, or every node reached the tip and none found one; the page the cap stopped at
/// (`capped`) when nothing was found yet and every node answered; `low` again (hold) when a node did
/// not answer or the nodes disagree. The cursor outlives the sink history, so a long outage is walked
/// to the end over as many ticks as it takes instead of degrading to "lost".
fn next_walk(v: Verdict, low: Hash, capped: Option<Hash>, unanswered: bool) -> Option<Hash> {
    match v {
        Verdict::Agreed => None,
        Verdict::Nothing if unanswered => Some(low),
        Verdict::Nothing => capped,
        Verdict::Waiting | Verdict::Split => Some(low),
    }
}

/// Item 9: what one walk of the blocks found and where the next one starts (see `next_walk`).
struct Scan {
    hit: Option<(RpcTransaction, Seen)>,
    next: Option<Hash>,
}

/// One node's answer: its name, then (our coin(s), the other coins at the address).
type NodeCoins = (String, (Vec<Coin>, Vec<Coin>));

/// What looking for a coin's spender came to. (A whole transaction in one variant and nothing in the others: fine,
/// one value lives at a time.)
#[allow(clippy::large_enum_variant)]
enum Found {
    Spender(RpcTransaction, Seen),
    /// A node has not answered, the nodes disagree, or the walk has pages left: nothing moves yet.
    Hold,
    /// Every node answered and none shows a spender.
    Nothing,
}

/// Item 10: a coin is gone only when every one of the `nodes` answered and none shows it; a node
/// that did not answer holds the state where it is.
fn gone_everywhere(sets: &[Vec<TransactionOutpoint>], nodes: usize, op: &TransactionOutpoint) -> bool {
    nodes > 0 && sets.len() == nodes && sets.iter().all(|s| !s.contains(op))
}

/// Item 10/11: a coin is back only when every one of the `nodes` answered and all show it.
fn all_show(sets: &[Vec<TransactionOutpoint>], nodes: usize, op: &TransactionOutpoint) -> bool {
    nodes > 0 && sets.len() == nodes && sets.iter().all(|s| s.contains(op))
}

/// Item 9: the states the rules can lead to from `s` without a choice the watcher cannot know
/// (a request's destination, a spend's amount): from Pending, `finish` and `decline` both land on
/// one Locked state; from Locked every next address depends on such a choice.
fn next_states(s: &State) -> Vec<(&'static str, State)> {
    if s.stage == 1 { vec![("finished or declined", after_finish(s))] } else { vec![] }
}

impl<'a> Watcher<'a> {
    fn new(ctx: &'a Ctx, cov: Hash, state: State, primary: String, others: Vec<(String, KaspaRpcClient)>) -> Self {
        let now = Instant::now();
        let witness = (0..=others.len()).map(|_| Liveness::after(UNCHECKED)).collect();
        Watcher {
            ctx,
            dials: ctx.dials().unwrap_or_else(|e| panic!("{e}")),
            source: ctx.source().unwrap_or_else(|e| panic!("{e}")),
            primary,
            others,
            cov,
            state,
            coin: None,
            sinks: VecDeque::new(),
            scan_from: None,
            misses: 0,
            last_countdown: None,
            foreign_seen: vec![],
            unconfirmed: None,
            lost: None,
            unwatched_said: None,
            closed: None,
            daa_seen: (0, now),
            stall_said: None,
            gap_said: None,
            gap_on: false,
            disagree: 0,
            waiting: 0,
            daa_low: u64::MAX,
            last_heartbeat: now,
            started: now,
            alerts: AlertFile::load(&ctx.dir),
            live: Liveness::new(),
            witness,
            auto_heir: false,
            open_sent: None,
            heir_warned: false,
            woke: Arc::new(Notify::new()),
            follow: ctx.dir.join(crate::watched::WATCHED_FILE).exists(),
            follow_wait: vec![],
            auto_stop: None,
            stop_sent: None,
            race_low: None,
            race_lost: 0,
            race_end: None,
        }
    }

    /// Watched-follow mode: writes a final move into the folder's state.json (anchored save). A failure is an
    /// alert: the copy of the account would go stale.
    fn record(&mut self, m: &Move) {
        if !self.follow {
            return;
        }
        let saved = self.ctx.try_load_vault().and_then(|v| v.ok_or_else(|| "state.json is missing".to_string())).and_then(|old| self.ctx.save_anchored(&follow_record(&old, m)));
        if let Err(e) = saved {
            self.alert(Kind::Unwatched, "This account's copy could not be updated. Check it by hand.", &format!("could not record `{}` (tx {}) in state.json: {e}", m.entry, m.txid));
        }
    }

    /// Picks up where a previous run left off: its state, its coin and the sink its block scan starts from.
    fn resume(&mut self, saved: Saved) {
        self.state = saved.state;
        self.closed = saved.closed;
        self.daa_seen = (saved.daa, Instant::now());
        self.open_sent = saved.open_sent.map(|id| (id, Instant::now()));
        if let Some(s) = saved.sink {
            self.sinks.push_back(s);
        }
        // Item 9: the first walk after a restart starts where the last run stopped looking.
        self.scan_from = saved.sink;
        if let Some((op, amount, daa)) = saved.coin {
            let spk = self.spk(&self.state);
            self.coin = Some((op, UtxoEntry::new(amount, spk, daa, false, Some(self.cov))));
        }
        // Item 11: a spend that was only provisional when the watcher stopped is not believed on
        // restart: the state and coin before it come back and the next tick looks for the spend again
        // (accepted: applied for good; vanished: the coin is simply there).
        if let Some(p) = saved.provisional {
            self.state = p.prev_state;
            self.closed = None;
            let (op, amount, daa) = p.prev_coin;
            let spk = self.spk(&self.state);
            self.coin = Some((op, UtxoEntry::new(amount, spk, daa, false, Some(self.cov))));
            if p.low != Hash::default() {
                self.sinks.clear();
                self.sinks.push_back(p.low);
                self.scan_from = Some(p.low);
            }
        }
    }

    fn persist(&self, daa: u64) {
        Saved {
            state: self.state.clone(),
            coin: self.coin.as_ref().map(|c| (c.0, c.1.amount, c.1.block_daa_score)),
            sink: self.scan_from.or_else(|| self.sinks.front().copied()),
            daa,
            closed: self.closed.clone(),
            open_sent: self.open_sent.as_ref().map(|(id, _)| id.clone()),
            provisional: self.unconfirmed.as_ref().map(|u| Provisional {
                txid: u.txid.clone(),
                prev_state: u.prev_state.clone(),
                prev_coin: (u.prev.0, u.prev.1.amount, u.prev.1.block_daa_score),
                low: u.low,
            }),
            cov: Some(self.cov),
        }
        .save(&self.ctx.dir);
    }

    /// The contract for a state, from the source and dials read at the start.
    fn art(&self, s: &State) -> SilAbiArtifact {
        crate::contract::compile(&self.source, &self.ctx.keys.public(), &self.dials, s)
    }

    fn spk(&self, s: &State) -> ScriptPublicKey {
        crate::contract::spk_of(&self.art(s))
    }

    fn address(&self, s: &State) -> Address {
        self.ctx.network.address_of(&self.spk(s)).expect("p2sh address")
    }

    /// The primary connection and every other `--node`, by name.
    fn nodes(&self) -> Vec<(&str, &KaspaRpcClient)> {
        let mut v = vec![(self.primary.as_str(), self.ctx.rpc())];
        v.extend(self.others.iter().map(|(n, r)| (n.as_str(), r)));
        v
    }

    /// How many nodes must answer before the coin counts as gone or back (item 10).
    fn node_count(&self) -> usize {
        1 + self.others.len()
    }

    /// Item 9: where the walk for a vanished coin starts: the catch-up cursor, else the oldest sink in view.
    fn scan_start(&self) -> Option<Hash> {
        self.scan_from.or_else(|| self.sinks.front().copied())
    }

    fn say(&self, level: Level, text: &str) {
        let tag = if level == Level::Alert { "ALERT" } else { "info " };
        let line = format!("{} {tag} {text}", stamp());
        println!("{line}");
        log_line(&self.ctx.dir, &line);
        if level == Level::Alert {
            push(text);
        }
    }

    /// An alert: raw detail to the console, alerts.log and the push hook; bank words to alerts.json.
    fn alert(&mut self, kind: Kind, bank: &str, raw: &str) {
        self.say(Level::Alert, raw);
        self.alerts.add(kind, bank);
        self.alerts.save(&self.ctx.dir);
        self.woke.notify_waiters();
    }

    /// Every node's view of the chain tip; a node that fails is marked in `down`.
    async fn ask_infos(&self, down: &mut [bool]) -> Vec<(String, u64, Hash)> {
        let mut infos = vec![];
        let nodes = self.nodes();
        let asks = nodes.iter().map(|(_, rpc)| in_time(rpc.get_block_dag_info()));
        let answers = futures::future::join_all(asks).await;
        for (i, ((name, _), answer)) in nodes.iter().zip(answers).enumerate() {
            match answer {
                Ok(info) => infos.push((name.to_string(), info.virtual_daa_score, info.sink)),
                Err(e) => {
                    down[i] = true;
                    println!("{} info  {name} unreachable: {e}", stamp());
                }
            }
        }
        infos
    }

    /// Every node's coins at the account address; a node that fails is marked in `down`.
    async fn ask_coins(&self, addr: &Address, down: &mut [bool]) -> Vec<NodeCoins> {
        let mut answers = vec![];
        let nodes = self.nodes();
        let asks = nodes.iter().map(|(_, rpc)| fetch_coins(rpc, addr, self.cov));
        let coins = futures::future::join_all(asks).await;
        for (i, ((name, _), answer)) in nodes.iter().zip(coins).enumerate() {
            match answer {
                Ok(c) => answers.push((name.to_string(), c)),
                Err(e) => {
                    down[i] = true;
                    println!("{} info  utxo query on {name} failed: {e}", stamp());
                }
            }
        }
        answers
    }

    /// After a tick where at least one node answered: a node quiet for UNCHECKED is
    /// an alert (once per silence), and its return is a log line.
    fn witness_check(&mut self, down: &[bool]) {
        let now = self.started.elapsed();
        let names: Vec<String> = self.nodes().iter().map(|(n, _)| n.to_string()).collect();
        let (quiet, back) = quiet_witnesses(&mut self.witness, down, now);
        for i in quiet {
            let s = now.saturating_sub(self.witness[i].last_good).as_secs();
            let raw = format!(
                "{} has not answered for {s} s while another node still does; no cross-check until it is back",
                names[i]
            );
            self.alert(Kind::Unchecked, ONE_QUIET, &raw);
        }
        for (i, gap) in back {
            let s = gap.as_secs();
            let text = format!("{} answers again after {s} s; cross-check restored", names[i]);
            self.say(Level::Info, &text);
        }
    }

    /// A tick with no good answer from any node.
    fn silence(&mut self) {
        let now = self.started.elapsed();
        if self.live.bad(now) {
            let s = now.saturating_sub(self.live.last_good).as_secs();
            self.alert(Kind::WatcherSilent, NO_NET, &format!("no good answer from any node for {s} s (network, DNS or node down); the heartbeat in alerts.json stays frozen until one answers"));
        }
    }

    /// A good tick: the heartbeat (`seen_at`), the sink history, the recovery line, the hourly-ish log line.
    fn good_answer(&mut self, daa: u64, sink: Hash) {
        if let Some(gap) = self.live.good(self.started.elapsed()) {
            self.say(Level::Info, &format!("the network answers again after {} s of silence; resuming", gap.as_secs()));
        }
        self.alerts.seen_at = Some(unix_now());
        GUARD_OK.store(unix_now(), Ordering::Relaxed);
        self.alerts.save(&self.ctx.dir);
        self.woke.notify_waiters();
        self.sinks.push_back(sink);
        while self.sinks.len() > SINK_HISTORY {
            self.sinks.pop_front();
        }
        if self.last_heartbeat.elapsed() >= HEARTBEAT {
            self.last_heartbeat = Instant::now();
            let coin = self.coin.as_ref().map(|c| format!("coin {} ({:.4} KAS)", c.0, kas(c.1.amount))).unwrap_or_else(|| "no coin in view".into());
            self.say(Level::Info, &format!("heartbeat: up {} min, DAA {daa}, stage {}, {coin}", self.started.elapsed().as_secs() / 60, self.state.stage));
        }
    }

    fn entry_name(&self, art: &SilAbiArtifact, tag: &[u8]) -> Option<String> {
        let c = art.contracts.values().next()?;
        c.entries.iter().find(|(_, e)| e.dispatch_tag.as_bytes().as_slice() == tag).map(|(n, _)| n.clone())
    }

    /// Item 8: whether the node's virtual chain accepts `txid`, walking the chain from `low` to the
    /// tip page by page. A chain the node cannot walk from `low` counts as not accepted (hold).
    async fn accepted(&self, rpc: &KaspaRpcClient, name: &str, low: Hash, txid: &str) -> bool {
        let mut start = low;
        for _ in 0..SCAN_PAGES_MAX {
            match in_time(rpc.get_virtual_chain_from_block(start, true, ACCEPT_CONFIRMATIONS)).await {
                Ok(r) => {
                    if is_accepted(txid, r.accepted_transaction_ids.iter().flat_map(|a| a.accepted_transaction_ids.iter())) {
                        return true;
                    }
                    match next_page(&r.added_chain_block_hashes, start) {
                        Some(next) => start = next,
                        None => return false,
                    }
                }
                Err(e) => {
                    println!("{} info  virtual chain from {start} on {name} failed: {e}; {txid} does not count as accepted yet", stamp());
                    return false;
                }
            }
        }
        false
    }

    /// Every block since `low` on every node, page by page up to the tip or the page cap (item 9),
    /// looking for the spender of `op`; of two conflicting spenders the accepted one is taken (item 8).
    /// The spender counts only when every node names the same one; a node whose call failed, or whose
    /// walk hit the cap without a sighting, has not answered yet (item 10). `next` says where the next
    /// tick's walk starts (`next_walk`). Every node failing on the first page means `low` is gone from
    /// them (pruned): that counts as caught up with nothing, so the caller looks another way.
    async fn scan_blocks(&mut self, low: Hash, op: &TransactionOutpoint) -> Scan {
        let mut found: Vec<(String, RpcTransaction, bool)> = vec![];
        let mut lagging: Vec<String> = vec![];
        let mut capped: Option<Hash> = None;
        let mut failed = 0usize;
        let mut pruned = 0usize;
        for (name, rpc) in self.nodes() {
            let mut hits: Vec<RpcTransaction> = vec![];
            let mut page = low;
            let mut errored = false;
            for n in 1..=SCAN_PAGES_MAX {
                match in_time(rpc.get_blocks(Some(page), true, true)).await {
                    Ok(r) => {
                        hits.extend(r.blocks.into_iter().flat_map(|b| b.transactions).filter(|t| spends(t, op)));
                        match step(&r.block_hashes, page, n) {
                            Step::Next(h) => page = h,
                            Step::Tip => break,
                            Step::Capped(h) => {
                                println!("{} info  {name}: {SCAN_PAGES_MAX} pages of blocks walked from {low} without reaching the tip; the next tick continues from {h}", stamp());
                                capped.get_or_insert(h);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        errored = true;
                        failed += 1;
                        pruned += usize::from(page == low);
                        println!("{} info  get_blocks from {page} on {name} failed: {e}; {name} counts as not answered", stamp());
                        break;
                    }
                }
            }
            if errored {
                lagging.push(name.to_string());
                continue;
            }
            let mut pick: Option<(RpcTransaction, bool)> = None;
            for t in hits {
                let ok = self.accepted(rpc, name, low, &txid_of(&t)).await;
                if ok || pick.is_none() {
                    pick = Some((t, ok));
                }
                if ok {
                    break;
                }
            }
            match pick {
                Some((t, ok)) => found.push((name.to_string(), t, ok)),
                None => lagging.push(name.to_string()),
            }
        }
        if pruned == self.node_count() {
            println!("{} info  no node can show the blocks from {low} any more (pruned); looking for the coin another way", stamp());
            return Scan { hit: None, next: None };
        }
        let named: Vec<(String, String)> = found.iter().map(|(n, t, _)| (n.clone(), txid_of(t))).collect();
        let v = self.agree(&named, &lagging, op, "a block");
        let next = next_walk(v, low, capped, failed > 0);
        if v != Verdict::Agreed {
            return Scan { hit: None, next };
        }
        let seen = if found.iter().all(|(_, _, ok)| *ok) { Seen::Accepted } else { Seen::Block };
        Scan { hit: found.into_iter().next().map(|(_, t, _)| (t, seen)), next }
    }

    /// Item 10: the nodes' joint answer about who spent `op`. A split is an alert; a node that has
    /// not seen the spender yet means wait, since one node's word alone moves nothing.
    fn agree(&mut self, found: &[(String, String)], lagging: &[String], op: &TransactionOutpoint, where_: &str) -> Verdict {
        let ids: Vec<String> = found.iter().map(|(_, id)| id.clone()).collect();
        let v = verdict(&ids, lagging.len());
        match v {
            Verdict::Split => {
                let who: Vec<String> = found.iter().map(|(n, id)| format!("{n} says {id}")).collect();
                self.alert(Kind::LineageMismatch, TWO_ANSWERS, &format!("NODES DISAGREE about who spent {op} ({where_}): {}; holding the last agreed state", who.join(", ")));
            }
            Verdict::Waiting => self.one_node_only(found, lagging, op, where_),
            Verdict::Agreed | Verdict::Nothing => {}
        }
        if v != Verdict::Waiting {
            self.waiting = 0;
        }
        v
    }

    /// Item 10: one node shows a spender of `op` the others do not. Held, as one node's word moves
    /// nothing; WAITING_TICKS looks of it in a row are an alert, so a node that hides a move (or
    /// shows a made-up one) cannot keep the guard quiet.
    fn one_node_only(
        &mut self,
        found: &[(String, String)],
        lagging: &[String],
        op: &TransactionOutpoint,
        where_: &str,
    ) {
        let Some((who, id)) = found.first() else { return };
        self.waiting = self.waiting.saturating_add(1);
        let others = lagging.join(" and ");
        let raw = format!("{who} shows the spender {id} of {op} in {where_}; {others} does not");
        if self.waiting == WAITING_TICKS {
            let text = format!(
                "ONE NODE ONLY: {raw} after {WAITING_TICKS} looks; holding the last agreed state"
            );
            self.alert(Kind::LineageMismatch, TWO_ANSWERS, &text);
        } else {
            let wait = "waiting for every node before believing it";
            println!("{} info  {raw} yet; {wait}", stamp());
        }
    }

    /// Looks for the transaction that spent `op`: blocks first, from the catch-up cursor (item 9),
    /// final once accepted (item 8); the mempool only as a hint, and only once the blocks are caught up.
    /// A node whose call failed has not answered: the answer is a hold, not a miss (item 10).
    async fn find_spender(&mut self, addr: &Address, op: &TransactionOutpoint) -> Found {
        if let Some(low) = self.scan_start() {
            let s = self.scan_blocks(low, op).await;
            self.scan_from = s.next;
            if let Some((tx, seen)) = s.hit {
                return Found::Spender(tx, seen);
            }
            if s.next.is_some() {
                return Found::Hold;
            }
        }
        let mut found: Vec<(String, RpcTransaction)> = vec![];
        let mut lagging: Vec<String> = vec![];
        let mut failed = 0usize;
        for (name, rpc) in self.nodes() {
            match in_time(rpc.get_mempool_entries_by_addresses(vec![addr.clone()], false, false)).await {
                Ok(entries) => match entries.into_iter().flat_map(|e| e.sending).find(|m| spends(&m.transaction, op)) {
                    Some(m) => found.push((name.to_string(), m.transaction)),
                    None => lagging.push(name.to_string()),
                },
                Err(e) => {
                    failed += 1;
                    lagging.push(name.to_string());
                    println!("{} info  mempool query on {name} failed: {e}; {name} counts as not answered", stamp());
                }
            }
        }
        let named: Vec<(String, String)> = found.iter().map(|(n, t)| (n.clone(), txid_of(t))).collect();
        match self.agree(&named, &lagging, op, "the mempool") {
            Verdict::Agreed => found.into_iter().next().map(|(_, t)| Found::Spender(t, Seen::Mempool)).unwrap_or(Found::Nothing),
            Verdict::Nothing if failed == 0 => Found::Nothing,
            _ => Found::Hold,
        }
    }

    /// Item 9: the coin left and no node can show the spend (the blocks it was in may be gone from
    /// the nodes). The addresses the rules lead to next are asked for a coin with this covenant id;
    /// one coin at one of them, the same on every node, is followed with an alert to check by hand.
    async fn probe_next(&mut self, c: &Coin) -> bool {
        for (how, next) in next_states(&self.state) {
            let addr = self.address(&next);
            let mut sets: Vec<Vec<Coin>> = vec![];
            for (_, rpc) in self.nodes() {
                if let Ok((ours, _)) = fetch_coins(rpc, &addr, self.cov).await {
                    sets.push(ours);
                }
            }
            let ops: Vec<Vec<TransactionOutpoint>> = sets.iter().map(|s| s.iter().map(|c| c.0).collect()).collect();
            // item 10: every node must answer, and all alike
            if sets.len() != self.node_count() || ops.windows(2).any(|w| w[0] != w[1]) || sets[0].len() != 1 {
                continue;
            }
            let found = sets.swap_remove(0).pop().unwrap();
            self.alert(
                Kind::Unwatched,
                "The guard lost sight of the savings for a while and found them again. Check by hand that the move was yours.",
                &format!(
                    "coin {} left and its transaction could not be found (the blocks it was in may be gone from the nodes); a coin of {:.4} KAS with this covenant id sits at {addr}, where the rules lead when the withdrawal is {how}; following it from there; check by hand",
                    c.0,
                    kas(found.1.amount)
                ),
            );
            self.state = next;
            self.coin = Some(found);
            self.misses = 0;
            self.lost = None;
            self.last_countdown = None;
            return true;
        }
        false
    }

    /// Whether this laptop knows the address `to`: the vault's own cold or heir address, or one a
    /// command run here has paid (state.json history and the pending destination, read fresh so a
    /// spend sent a moment ago already counts). No state.json means nothing is known: every spend is loud.
    fn known_payee(&self, to: &str) -> bool {
        if to.is_empty() {
            return false;
        }
        let public = self.ctx.keys.public();
        if to == public.cold_address || to == public.heir_address {
            return true;
        }
        self.ctx.try_load_vault().ok().flatten().is_some_and(|v| v.pending_to.as_deref() == Some(to) || v.history.iter().any(|h| h.contains(to)))
    }

    /// A mempool sighting now in a block. Most kinds are alerted again as confirmed; a spend was
    /// already alerted in full from the mempool, so its confirmation is one quiet line (one tx, one alert).
    fn confirmed(&mut self, u: &Unconfirmed) {
        let text = format!("CONFIRMED in a block: {}", u.text);
        let level = if matches!(u.kind, Kind::Spend | Kind::SpendStranger) { Level::Info } else { u.level };
        match level {
            Level::Alert => self.alert(u.kind, &u.bank, &text),
            Level::Info => self.say(Level::Info, &text),
        }
        for m in std::mem::take(&mut self.follow_wait) {
            self.record(&m);
        }
    }

    /// Works out what the spending transaction did to the vault and checks the continuation
    /// output really carries the state it should (same covenant id, same script), and that what
    /// output 0 shows (cold, heir, destination, amount, timing) is what the rules pin. Pure:
    /// nothing here panics on a hostile signature script; every bad shape is an alert text.
    fn judge(&self, tx: &RpcTransaction, prev: &Coin) -> Outcome {
        let txid = txid_of(tx);
        let bad = |entry: &str, kind: Kind, bank: &str, text: String| Outcome { entry: entry.into(), level: Level::Alert, kind, bank: bank.into(), text, next: None, closed: false, rec: None };
        let art = self.art(&self.state);
        let Some(p) = tx.inputs.first().and_then(|i| pushes(&i.signature_script)) else {
            return bad("?", Kind::LineageMismatch, RULES_BROKEN, format!("vault coin spent by tx {txid} with an unreadable signature script (truncated push or non-push opcode)"));
        };
        let entry = if p.len() >= 2 { self.entry_name(&art, &p[p.len() - 2]) } else { None };
        let Some(entry) = entry else {
            return bad("?", Kind::LineageMismatch, RULES_BROKEN, format!("vault coin spent by tx {txid} through no known entry (lineage broken?)"));
        };
        let cont = tx.outputs.iter().position(|o| o.covenant.as_ref().map(|c| c.0.covenant_id) == Some(self.cov));
        let in_value = prev.1.amount;
        let in_daa = prev.1.block_daa_score as i64;
        let request = if entry == "request" { decode_request(&p) } else { None };
        let next: Option<State> = match entry.as_str() {
            "spend" => {
                let out1 = cont.and_then(|i| tx.outputs.get(i)).map(|o| o.value).unwrap_or(0);
                Some(after_spend(&self.state, self.dials.window, in_daa, tx.lock_time as i64, in_value as i64 - out1 as i64))
            }
            "request" => match &request {
                Some((_, amount)) => {
                    let dest_hash = blake2b_simd::Params::new().hash_length(32).hash(&p[1]).as_bytes().to_vec();
                    Some(State { stage: 1, dest_hash, amount: *amount, ..self.state.clone() })
                }
                None => {
                    let n = p.get(1).map(|b| b.len()).unwrap_or(0);
                    return bad(
                        "request",
                        Kind::WithdrawalPending,
                        &format!("A withdrawal was requested in a shape the guard cannot read. {STOP_HINT}"),
                        format!(
                            "WITHDRAWAL REQUESTED by tx {txid} with an unreadable request (destination push of {n} bytes); this watcher cannot follow the coin from here. Not you? Run `cancel` now and everything goes to cold."
                        ),
                    );
                }
            },
            // v3 wipes the destination on the way back to Locked; one rule shared with `finish`
            "finish" => cont.map(|_| after_finish(&self.state)),
            // topup renews the coin with the state exactly as it was
            "topup" => cont.map(|_| self.state.clone()),
            // v5: a stop key takes a Pending coin back to Locked; same lineage, nothing leaves
            "decline" => cont.map(|_| after_decline(&self.state)),
            _ => None,
        };
        // Verify the continuation against the state we derived: same covenant id (checked
        // above) and exactly the script the contract would demand for that state.
        if let (Some(i), Some(n)) = (cont, &next)
            && tx.outputs[i].script_public_key != self.spk(n) {
                return bad(&entry, Kind::LineageMismatch, RULES_BROKEN, format!("tx {txid} ({entry}) carries the vault's covenant id but its output script does not match the state it should have; lineage check FAILED"));
            }
        let out0 = tx.outputs.first();
        let (v0, to) = out0.map(|o| (o.value, addr_of(&self.ctx.network, &o.script_public_key))).unwrap_or((0, "nowhere".into()));
        // Output 0 is checked against what the rules pin, never repeated as fact.
        let paid_to = |want: &ScriptPublicKey| out0.is_some_and(|o| o.script_public_key == *want);
        let wrong = |what: &str| bad(&entry, Kind::LineageMismatch, RULES_BROKEN, format!("tx {txid} ({entry}) paid {:.4} KAS to {to}, which is not {what}; the money went somewhere the rules do not allow; check by hand", kas(v0)));
        // What a watched folder's state.json records (follow_record): the request's destination, the amount moved, the fee.
        let rec_to = request.as_ref().map(|(spk, _)| addr_of(&self.ctx.network, spk)).unwrap_or_else(|| to.clone());
        let moved = match (entry.as_str(), &request, cont) {
            ("request", Some((_, a)), _) => (*a).max(0) as u64,
            ("topup", _, Some(i)) => tx.outputs[i].value.saturating_sub(in_value),
            _ => v0,
        };
        let fee = (tx.inputs.len() == 1).then(|| in_value.saturating_sub(tx.outputs.iter().map(|o| o.value).sum::<u64>()));
        let heir = self.ctx.keys.public().heir_address;
        let ok = |level, kind, bank: String, text, next: Option<State>, closed: bool| {
            let rec = Move { entry: entry.clone(), next: next.clone(), closed, to: rec_to.clone(), moved, fee, txid: txid.clone(), slot: decode_cancel_slot(&p), delay: self.dials.delay, heir: heir.clone() };
            Outcome { entry: entry.clone(), level, kind, bank, text, next, closed, rec: Some(rec) }
        };
        match (entry.as_str(), cont, next) {
            ("spend", Some(i), Some(n)) => {
                // A copied spend key can drain one window's limit forever if the
                // owner is never told, so every spend is an alert; louder when this laptop never paid `to`.
                let known = self.known_payee(&to);
                let (kind, bank, note) = if known {
                    (Kind::Spend, format!("{:.4} KAS was sent to {to}, an address you have paid before. {STOP_HINT}", kas(v0)), "")
                } else {
                    (Kind::SpendStranger, format!("{:.4} KAS was sent to {to}. {STRANGER_HINT}", kas(v0)), ", an address this wallet device has never paid")
                };
                ok(
                    Level::Alert,
                    kind,
                    bank,
                    format!(
                        "SPENT {:.4} KAS to {to}{note} (tx {txid}); lineage ok; window counter {:.4} of {:.4} KAS; vault keeps {:.4} KAS",
                        kas(v0),
                        kas(n.spent as u64),
                        kas(self.dials.cap as u64),
                        kas(tx.outputs[i].value)
                    ),
                    Some(n),
                    false,
                )
            }
            ("request", Some(_), Some(n)) => {
                let to = request.map(|(spk, _)| addr_of(&self.ctx.network, &spk)).unwrap_or_default();
                let eta = minutes(&self.ctx.network, self.dials.delay);
                ok(
                    Level::Alert,
                    Kind::WithdrawalPending,
                    format!("A withdrawal of {:.4} KAS was requested. It can leave in about {eta:.0} minutes. {STOP_HINT}", kas(n.amount as u64)),
                    format!(
                        "WITHDRAWAL REQUESTED: {:.4} KAS to {to} (tx {txid}); lineage ok. It can leave the vault in about {eta:.1} minutes ({} DAA). Not you? Run `cancel` now and everything goes to cold.",
                        kas(n.amount as u64),
                        self.dials.delay
                    ),
                    Some(n),
                    false,
                )
            }
            ("finish", c, n) => {
                if !out0.is_some_and(|o| spk_hash(&o.script_public_key) == self.state.dest_hash) {
                    return wrong("the destination recorded when the withdrawal was requested");
                }
                if v0 != self.state.amount as u64 {
                    return bad(&entry, Kind::LineageMismatch, RULES_BROKEN, format!("tx {txid} (finish) paid {:.4} KAS to {to}, not the recorded {:.4} KAS; check by hand", kas(v0), kas(self.state.amount as u64)));
                }
                // the lowest DAA any node reported, so one node's clock cannot pass an early finish
                let virtual_daa = self.daa_seen.0.min(self.daa_low) as i64;
                if in_daa + self.dials.delay > virtual_daa {
                    return bad(&entry, Kind::LineageMismatch, RULES_BROKEN, format!("tx {txid} (finish) seen at virtual DAA {virtual_daa}, {} DAA before the delay allows; the node may be lying", in_daa + self.dials.delay - virtual_daa));
                }
                let bank = format!("{:.4} KAS left the account to {to}.", kas(v0));
                match (c, n) {
                    (Some(i), Some(n)) => ok(
                        Level::Alert,
                        Kind::WithdrawalCompleted,
                        bank,
                        format!("WITHDRAWAL COMPLETED: {:.4} KAS left to {to} (tx {txid}); lineage ok; vault keeps {:.4} KAS", kas(v0), kas(tx.outputs[i].value)),
                        Some(n),
                        false,
                    ),
                    _ => ok(Level::Alert, Kind::WithdrawalCompleted, bank, format!("WITHDRAWAL COMPLETED: {:.4} KAS left to {to} (tx {txid}); the vault is now empty", kas(v0)), None, true),
                }
            }
            ("topup", Some(i), Some(n)) => ok(
                Level::Info,
                Kind::Stopped,
                String::new(),
                format!("TOP-UP: vault grew from {:.4} to {:.4} KAS (tx {txid}); lineage ok; state unchanged", kas(in_value), kas(tx.outputs[i].value)),
                Some(n),
                false,
            ),
            ("cancel", _, _) => {
                if !paid_to(&self.ctx.cold_spk()) {
                    return wrong("the cold address");
                }
                let who = decode_cancel_slot(&p).map(|s| format!("stop key {}", s + 1)).unwrap_or_else(|| "an unknown stop key".into());
                ok(
                    Level::Alert,
                    Kind::Stopped,
                    "The account was closed by a stop key. Everything went to the cold safe.".into(),
                    format!("CANCELLED by {who}: {:.4} KAS sent to cold {to} (tx {txid}); the vault is closed", kas(v0)),
                    None,
                    true,
                )
            }
            ("decline", Some(i), Some(n)) => {
                let who = decode_cancel_slot(&p).map(|s| format!("stop key {}", s + 1)).unwrap_or_else(|| "an unknown stop key".into());
                ok(
                    Level::Alert,
                    Kind::WithdrawalDeclined,
                    format!("A waiting withdrawal was declined by {who}; the account is Locked again."),
                    format!(
                        "DECLINED by {who}: the request for {:.4} KAS is withdrawn (tx {txid}); lineage ok; vault keeps {:.4} KAS, Locked",
                        kas(self.state.amount as u64),
                        kas(tx.outputs[i].value)
                    ),
                    Some(n),
                    false,
                )
            }
            ("inherit" | "inheritOpen", _, _) => {
                // No heir: the heir slot repeats the cold script, so inherit pays cold.
                let heir = self.ctx.keys.public().heir_address;
                let want = if heir.is_empty() { self.ctx.cold_spk() } else { self.ctx.network.to_spk(&heir) };
                // From Pending the committed destination is paid first (output 0), the heir after it.
                let heir_out = if self.state.stage == 1 {
                    if !out0.is_some_and(|o| spk_hash(&o.script_public_key) == self.state.dest_hash && o.value == self.state.amount as u64) {
                        return wrong("the destination and amount recorded when the withdrawal was requested");
                    }
                    tx.outputs.get(1)
                } else {
                    out0
                };
                if !heir_out.is_some_and(|o| o.script_public_key == want) {
                    return wrong("the heir's address");
                }
                let (hv, hto) = heir_out.map(|o| (o.value, addr_of(&self.ctx.network, &o.script_public_key))).unwrap_or((0, "nowhere".into()));
                // Item 7: from Pending the waiting withdrawal is paid on the way out; report it too.
                let (dest_bank, dest_raw) = if self.state.stage == 1 {
                    (format!(" The waiting withdrawal of {:.4} KAS went to its destination.", kas(v0)), format!("; the pending {:.4} KAS went to {to}", kas(v0)))
                } else {
                    (String::new(), String::new())
                };
                let (bank, tag) = if entry == "inheritOpen" {
                    ("The heir received the account after the silence passed (sent without a key).", "INHERITED (open, no key)")
                } else {
                    ("The account was closed and everything went to the heir.", "INHERITED")
                };
                ok(Level::Alert, Kind::Stopped, format!("{bank}{dest_bank}"), format!("{tag}: {:.4} KAS sent to heir {hto}{dest_raw} (tx {txid}); the vault is closed", kas(hv)), None, true)
            }
            _ => bad(&entry, Kind::LineageMismatch, RULES_BROKEN, format!("tx {txid} spent the vault coin through `{entry}` in a shape this watcher cannot follow")),
        }
    }

    /// Applies a judged spend. From the mempool, or from a block the chain has not accepted, it is
    /// provisional: the pre-state is kept (and saved, item 11) until the accepted chain confirms it.
    fn apply(&mut self, tx: &RpcTransaction, prev: &Coin, seen: Seen) {
        let o = self.judge(tx, prev);
        let suffix = match seen {
            Seen::Mempool => " (unconfirmed: seen in the mempool, not yet in a block)",
            Seen::Block => " (unconfirmed: in a block the network has not accepted yet)",
            Seen::Accepted => "",
        };
        let text = format!("{}{suffix}", o.text);
        match o.level {
            Level::Alert => self.alert(o.kind, &o.bank, &text),
            Level::Info => self.say(Level::Info, &text),
        }
        if seen != Seen::Accepted && self.unconfirmed.is_none() {
            self.unconfirmed = Some(Unconfirmed {
                txid: txid_of(tx),
                prev: prev.clone(),
                prev_state: self.state.clone(),
                low: self.sinks.front().copied().unwrap_or_default(),
                since: Instant::now(),
                level: o.level,
                kind: o.kind,
                bank: o.bank.clone(),
                text: o.text.clone(),
                warned: false,
            });
        }
        if let Some(m) = &o.rec {
            if seen == Seen::Accepted {
                self.follow_wait.clear();
                self.record(m);
            } else {
                self.follow_wait.push(m.clone());
            }
        }
        if o.next.as_ref().map(|n| n.stage) == Some(1) && self.state.stage == 0 {
            // Already alerted; stop the 'pending found' alert from firing again on the next tick.
            self.last_countdown = Some(i64::MIN);
        }
        if let Some(n) = o.next {
            self.state = n;
        }
        if o.closed {
            self.state.stage = 9;
            self.closed = Some(format!("vault closed by `{}`", o.entry));
        }
        self.coin = None;
    }

    /// A provisional sighting (mempool, or a block the chain has not accepted) is confirmed when the
    /// accepted chain spends the same coin with the same tx; a different accepted tx means it lost and
    /// the state is re-derived; the old coin back on every node means it vanished, and the state rolls back (item 11).
    async fn check_unconfirmed(&mut self) {
        let Some(u) = self.unconfirmed.clone() else { return };
        let s = self.scan_blocks(u.low, &u.prev.0).await;
        if let Some((tx, Seen::Accepted)) = s.hit {
            let id = txid_of(&tx);
            self.unconfirmed = None;
            if id == u.txid {
                self.confirmed(&u);
            } else {
                self.alert(Kind::LineageMismatch, TWO_ANSWERS, &format!("the provisional transaction {} LOST to {id}, which spent the same vault coin and was accepted; re-deriving the state from the accepted one", u.txid));
                self.state = u.prev_state.clone();
                self.closed = None;
                self.follow_wait.clear();
                self.apply(&tx, &u.prev, Seen::Accepted);
            }
            return;
        }
        // Item 9: a walk the page cap stopped continues from its last page next tick (and after a restart).
        if let (Some(h), Some(x)) = (s.next, &mut self.unconfirmed) {
            x.low = h;
        }
        if self.coin_back(&u).await {
            self.roll_back(&u);
            return;
        }
        if u.since.elapsed() >= UNCONFIRMED_WARN && !u.warned {
            self.alert(
                Kind::LineageMismatch,
                "A change to the account has waited two minutes without the network confirming it. Check by hand.",
                &format!("tx {} is still not accepted after {} s; nothing about it is final yet", u.txid, u.since.elapsed().as_secs()),
            );
            if let Some(x) = &mut self.unconfirmed {
                x.warned = true;
            }
        }
    }

    /// Item 11: whether every node (none missing, item 10) shows the coin a provisional spend took, back where it was.
    async fn coin_back(&self, u: &Unconfirmed) -> bool {
        let addr = self.address(&u.prev_state);
        let mut sets: Vec<Vec<TransactionOutpoint>> = vec![];
        for (_, rpc) in self.nodes() {
            if let Ok((ours, _)) = fetch_coins(rpc, &addr, self.cov).await {
                sets.push(ours.iter().map(|c| c.0).collect());
            }
        }
        all_show(&sets, self.node_count(), &u.prev.0)
    }

    /// Item 11: the provisional spend vanished and the old coin is back: the state goes back with it.
    fn roll_back(&mut self, u: &Unconfirmed) {
        self.alert(
            Kind::LineageMismatch,
            "A change to the account was dropped by the network. The account is back where it was.",
            &format!(
                "tx {} vanished before the network accepted it and coin {} is back at {}; the state goes back to stage {}",
                u.txid,
                u.prev.0,
                addr_of(&self.ctx.network, &u.prev.1.script_public_key),
                u.prev_state.stage
            ),
        );
        self.unconfirmed = None;
        self.follow_wait.clear();
        self.state = u.prev_state.clone();
        self.closed = None;
        self.coin = Some(u.prev.clone());
        self.misses = 0;
        self.lost = None;
    }

    /// Alerts when the node's clock (its virtual DAA score) stops against wall time.
    fn watch_clock(&mut self, daa: u64) {
        if daa != self.daa_seen.0 {
            self.daa_seen = (daa, Instant::now());
            self.stall_said = None;
        } else if self.daa_seen.1.elapsed() >= STALL && self.stall_said.is_none_or(|t| t.elapsed() >= REMIND) {
            let primary = self.primary.clone();
            self.alert(
                Kind::WatcherSilent,
                "The guard's network source has stopped moving. Check the wallet device.",
                &format!("node stalled: {primary} reports DAA {daa} unchanged for {} s; it may be lying or cut off", self.daa_seen.1.elapsed().as_secs()),
            );
            self.stall_said = Some(Instant::now());
        }
    }

    /// v5 `--auto-heir`: the heir's silence clock on the watched coin. A warning at 90% of the
    /// heir period, and the `inheritOpen` itself once it has passed, sent once (a resend only
    /// after OPEN_RETRY with the coin still here and the first one gone from the mempool).
    async fn heir_clock_tick(&mut self, c: &Coin, daa: u64) {
        let public = self.ctx.keys.public();
        if !(self.auto_heir && public.auto_heir) {
            return;
        }
        let delay = public.inherit_delay(&self.dials);
        let age = (daa as i64).saturating_sub(c.1.block_daa_score as i64);
        match heir_clock(age, delay) {
            HeirClock::Quiet => self.heir_warned = false,
            HeirClock::Warn => {
                if !self.heir_warned {
                    self.heir_warned = true;
                    self.alert(
                        Kind::HeirClock,
                        "The account has been silent for 90% of the heir period; any payment restarts the clock.",
                        &format!("heir clock: the coin is {age} of {delay} DAA old (90% of the heir period); with --auto-heir the inheritOpen goes out at {delay}"),
                    );
                }
            }
            HeirClock::Due => {
                if !open_due(self.auto_heir, public.auto_heir, age, delay) {
                    return;
                }
                if let Some((id, at)) = self.open_sent.clone() {
                    if at.elapsed() < OPEN_RETRY || self.open_in_mempool(&c.0).await {
                        return;
                    }
                    self.say(Level::Info, &format!("inheritOpen tx {id} is no longer in the mempool and the coin is still here; sending it again"));
                }
                match self.send_inherit_open(c, &public, delay).await {
                    Ok(id) => {
                        self.open_sent = Some((id.clone(), Instant::now()));
                        self.alert(Kind::HeirClock, "The silence has passed; the account is being sent to the heir.", &format!("inheritOpen sent, tx {id}"));
                    }
                    Err(e) => self.say(Level::Info, &format!("inheritOpen not sent: {e}")),
                }
            }
        }
    }

    /// Whether the primary node's mempool still holds a transaction spending the watched coin.
    async fn open_in_mempool(&self, op: &TransactionOutpoint) -> bool {
        let addr = self.address(&self.state);
        match in_time(self.ctx.rpc().get_mempool_entries_by_addresses(vec![addr], false, false)).await {
            Ok(entries) => entries.into_iter().flat_map(|e| e.sending).any(|m| spends(&m.transaction, op)),
            Err(_) => true,
        }
    }

    /// Builds and submits the `inheritOpen`, after checking the coin is still there, state.json agrees
    /// with what the watcher follows, and the vault has an heir.
    async fn send_inherit_open(&self, c: &Coin, public: &Public, delay: i64) -> Result<String, String> {
        if public.heir_x.is_empty() || public.heir_address.is_empty() {
            return Err("this vault has no heir (no heir_x in the public part of keys.json)".into());
        }
        let v = self.ctx.try_load_vault()?.ok_or("state.json is missing")?;
        if v.covenant_id != self.cov || v.state != self.state {
            return Err(format!("state.json (stage {}) does not match what the watcher follows (stage {}); not sending", v.state.stage, self.state.stage));
        }
        let (ours, _) = fetch_coins(self.ctx.rpc(), &self.address(&self.state), self.cov).await?;
        if !ours.iter().any(|(op, _)| *op == c.0) {
            return Err("the coin is gone".into());
        }
        let tx = build_inherit_open(self.ctx, &v, c, public, delay)?;
        self.ctx.try_submit(&tx, "inheritOpen").await
    }

    async fn tick(&mut self) -> Tick {
        // Every node is asked; any answer counts for the liveness rule, and the answers must agree.
        let mut down = vec![false; self.node_count()];
        let infos = self.ask_infos(&mut down).await;
        let Some(&(_, daa, sink)) = infos.first() else {
            self.silence();
            return Tick::Go;
        };
        self.watch_clock(daa);
        self.daa_low = infos.iter().map(|i| i.1).min().unwrap_or(daa);
        let was_apart = self.gap_on;
        self.gap_on = infos[1..].iter().any(|i| daa.abs_diff(i.1) > NODE_DAA_GAP);
        for (name, daa2, _) in &infos[1..] {
            let gap = daa.abs_diff(*daa2);
            if gap > NODE_DAA_GAP && say_fault(was_apart, self.gap_said.map(|t| t.elapsed())) {
                self.gap_said = Some(Instant::now());
                let primary = self.primary.clone();
                self.alert(Kind::LineageMismatch, TWO_ANSWERS, &format!("NODES DISAGREE: {primary} is at DAA {daa} and {name} at {daa2} ({gap} apart); one is behind or lying"));
            }
        }
        self.check_unconfirmed().await;
        if self.state.stage == 9 && self.unconfirmed.is_none() {
            self.persist(daa);
            return Tick::Done(self.closed.clone().unwrap_or_else(|| "vault closed".into()));
        }
        let addr = self.address(&self.state);
        let mut answers = self.ask_coins(&addr, &mut down).await;
        if answers.is_empty() {
            self.silence();
            return Tick::Go;
        }
        self.witness_check(&down);
        self.good_answer(daa, sink);
        let sets: Vec<Vec<TransactionOutpoint>> = answers.iter().map(|(_, (ours, _))| ours.iter().map(|c| c.0).collect()).collect();
        if sets.windows(2).all(|w| w[0] == w[1]) {
            self.disagree = 0;
        } else {
            self.disagree += 1;
            if self.disagree == DISAGREE_TICKS {
                let who: Vec<String> = answers.iter().zip(&sets).map(|((n, _), s)| format!("{n} sees {s:?}")).collect();
                self.alert(Kind::LineageMismatch, TWO_ANSWERS, &format!("NODES DISAGREE about the vault coin at {addr}: {}; one of them is lying or far behind", who.join("; ")));
            }
        }
        // Item 10: the coin counts as gone only when every node answered and says so; one node's word
        // moves nothing. While they differ the answer still showing the coin is followed, and
        // DISAGREE_TICKS of that is the alert above (a stale view replayed by one node is loud, not followed).
        let pick = match &self.coin {
            Some(c) if !gone_everywhere(&sets, self.node_count(), &c.0) => sets.iter().position(|s| s.contains(&c.0)).unwrap_or(0),
            _ => 0,
        };
        let (_, (mut ours, foreign)) = answers.swap_remove(pick);
        for (op, u) in foreign {
            if self.foreign_seen.contains(&op) {
                continue;
            }
            if self.foreign_seen.len() >= MAX_FOREIGN {
                if self.foreign_seen.len() == MAX_FOREIGN {
                    self.say(Level::Info, &format!("more than {MAX_FOREIGN} foreign coins at the vault address; no longer listing them"));
                    self.foreign_seen.push(op);
                }
                break;
            }
            self.foreign_seen.push(op);
            self.say(Level::Info, &format!("a coin of {:.4} KAS sits at the vault address without this vault's lineage ({op}); ignored", kas(u.amount)));
        }
        match self.coin.clone() {
            None => {
                if ours.len() == 1 {
                    let c = ours.pop().unwrap();
                    let stage = if self.state.stage == 1 { "PENDING" } else { "Locked" };
                    self.say(Level::Info, &format!("watching coin {} of {:.4} KAS at {addr} ({stage}, lineage ok)", c.0, kas(c.1.amount)));
                    if self.state.stage == 1 && self.last_countdown.is_none() {
                        let left = c.1.block_daa_score as i64 + self.dials.delay - daa as i64;
                        let mins = minutes(&self.ctx.network, left.max(0));
                        self.alert(
                            Kind::WithdrawalPending,
                            &format!("A withdrawal of {:.4} KAS is waiting. It can leave in about {mins:.0} minutes. {STOP_HINT}", kas(self.state.amount as u64)),
                            &format!("PENDING WITHDRAWAL of {:.4} KAS found; it can leave in about {mins:.1} minutes", kas(self.state.amount as u64)),
                        );
                    }
                    self.coin = Some(c);
                    self.misses = 0;
                    self.lost = None;
                    self.unwatched_said = None;
                } else if ours.len() > 1 {
                    self.alert(Kind::LineageMismatch, RULES_BROKEN, &format!("{} coins carry this vault's covenant id at {addr}; the contract allows only one", ours.len()));
                } else if let Some((op, last)) = self.lost {
                    if last.elapsed() >= LOST_REALERT {
                        self.lost = Some((op, Instant::now()));
                        self.alert(Kind::Unwatched, NO_COIN, &format!("VAULT UNWATCHED: coin {op} left and its transaction was never found; no coin with covenant id {} at {addr}; check the vault by hand", self.cov));
                    }
                } else {
                    self.misses += 1;
                    let due = match self.unwatched_said {
                        None => self.misses >= UNWATCHED_TICKS,
                        Some(t) => t.elapsed() >= LOST_REALERT,
                    };
                    if due {
                        self.unwatched_said = Some(Instant::now());
                        self.alert(Kind::Unwatched, NO_COIN, &format!("VAULT UNWATCHED: no coin with covenant id {} at {addr} after {} looks (DAA {daa}) and nothing to resume from; fund the vault or check it by hand", self.cov, self.misses));
                    } else if self.misses % 10 == 1 {
                        println!("{} info  no coin with covenant id {} at {addr} yet (DAA {daa})", stamp(), self.cov);
                    }
                }
            }
            Some(c) => {
                if ours.iter().any(|(op, _)| *op == c.0) {
                    // item 9: the coin is in view, so there is nothing to catch up on
                    self.scan_from = None;
                    if self.state.stage == 1 {
                        let left = c.1.block_daa_score as i64 + self.dials.delay - daa as i64;
                        let mins = minutes(&self.ctx.network, left.max(0)).ceil() as i64;
                        if self.last_countdown != Some(mins) {
                            self.last_countdown = Some(mins);
                            if left > 0 {
                                println!("{} info  pending {:.4} KAS: finish possible in {mins} min ({left} DAA)", stamp(), kas(self.state.amount as u64));
                            } else {
                                self.alert(
                                    Kind::WithdrawalPending,
                                    &format!("The waiting time has passed. {:.4} KAS can leave the account at any moment. {STOP_HINT}", kas(self.state.amount as u64)),
                                    &format!("the delay has passed: {:.4} KAS can now leave at any moment; cancel if this is not yours", kas(self.state.amount as u64)),
                                );
                            }
                        }
                    }
                    self.heir_clock_tick(&c, daa).await;
                } else {
                    match self.find_spender(&addr, &c.0).await {
                        Found::Spender(tx, seen) => self.apply(&tx, &c, seen),
                        // item 9/10: pages left to walk, or a node that has not answered: nothing moves and no miss counts
                        Found::Hold => {}
                        Found::Nothing => {
                            self.misses += 1;
                            if self.misses >= 5 && self.probe_next(&c).await {
                                // item 9: found again at the address the rules lead to
                            } else if self.misses >= 5 {
                                self.alert(Kind::Unwatched, NO_COIN, &format!("VAULT UNWATCHED: coin {} left {addr} and its transaction could not be found after {} tries; check the vault by hand", c.0, self.misses));
                                self.lost = Some((c.0, Instant::now()));
                                self.coin = None;
                                self.misses = 0;
                            } else {
                                println!("{} info  coin {} is gone from {addr}; looking for the transaction that moved it", stamp(), c.0);
                            }
                        }
                    }
                }
            }
        }
        self.persist(daa);
        Tick::Go
    }
}

// `HeirClock` and `heir_clock` live in contract.rs since v6, shared with the phone service's
// `heir_seconds_left`.

/// Whether the watcher sends the `inheritOpen` now: only with `--auto-heir`, only on a vault made
/// with the auto-heir dial, only once the silence has passed.
fn open_due(flag: bool, auto_heir: bool, age: i64, delay: i64) -> bool {
    flag && auto_heir && heir_clock(age, delay) == HeirClock::Due
}

/// The `inheritOpen` transaction without a lift (the watcher holds no funding key). The output
/// rules are those of commands.rs `heir_exit` (private there, copied): from Pending the request's
/// destination is paid first, then the heir gets the rest minus the fee, never under minRemainder.
fn build_inherit_open(ctx: &Ctx, v: &Vault, coin: &Coin, public: &Public, delay: i64) -> Result<Transaction, String> {
    let heir_spk = ctx.network.try_to_spk(&public.heir_address)?;
    let pending = if v.state.stage == 1 {
        let dest = v.pending_to.clone().ok_or("the vault is Pending but state.json holds no pending destination")?;
        let dest_script = ctx.network.try_to_spk(&dest)?;
        if spk_hash(&dest_script) != v.state.dest_hash {
            return Err("stored destination does not match recorded hash".into());
        }
        Some((v.state.amount as u64, dest_script))
    } else {
        None
    };
    let paid = pending.as_ref().map_or(0, |p| p.0);
    let (tx, _fee) = ctx
        .try_with_fee(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
            let left = coin.1.amount.checked_sub(paid + fee).ok_or("the vault cannot pay the pending amount and the fee")?;
            if left < v.dials.min_remainder as u64 {
                return Err(format!("the heir's share would be {} KAS, under the {} KAS minimum a closing output must carry", kas(left), kas(v.dials.min_remainder as u64)));
            }
            let mut outputs = Vec::new();
            if let Some((amount, dest_script)) = &pending {
                outputs.push(TransactionOutput::new(*amount, dest_script.clone()));
            }
            outputs.push(TransactionOutput::new(left, heir_spk.clone()));
            ctx.vault_tx(v, coin, "inheritOpen", None, &[], outputs, delay as u64, 0)
        })
        .map_err(|e| e.to_string())?;
    Ok(tx)
}

/// Runs until the vault closes. `nodes`: the pinned wRPC urls from `--node` (the first is already
/// the `ctx` connection; every other one is polled beside it and must agree with it). `auto_heir`
/// is the v5 `--auto-heir` flag: send the inheritOpen once the silence has passed.
/// What to say when `--node` names nothing: the guard then asks a public node picked at random, and
/// a lying node needs no access to this laptop to be chosen.
fn no_node_warning(nodes: &[String]) -> Option<&'static str> {
    nodes.is_empty().then_some("No --node given: the guard asks a public node picked at random, which anyone could run. Give it your own node and one more with --node.")
}

/// The CLI form: today's `watch` command. Refusals print and exit(2); a panic inside a tick is
/// logged with its location by the hook here and alerted by `watcher_down`, then exit(2).
pub async fn run(ctx: &Ctx, nodes: &[String], auto_heir: bool, auto_stop: Option<AutoStop>) {
    let dir = ctx.dir.clone();
    std::panic::set_hook(Box::new(move |info| {
        let msg = panic_text(info.payload());
        let loc = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let line = format!("{} ALERT WATCHER DOWN: panic{loc}: {msg}", stamp());
        println!("{line}");
        log_line(&dir, &line);
    }));
    let poll = Arc::new(AtomicU64::new(POLL.as_secs()));
    let (_stop_tx, stop) = tokio::sync::watch::channel(false);
    let woke = Arc::new(Notify::new());
    GUARD_OK.store(unix_now(), Ordering::Relaxed);
    watchdog(ctx.dir.clone());
    let guard = run_guard_with(ctx.clone(), nodes.to_vec(), auto_heir, poll, stop, woke, auto_stop);
    if let Err(e) = guard.await {
        eprintln!("Refused: {e}");
        std::process::exit(2);
    }
}

/// The first of `nodes` that connects, and its index; every failure is said. All down is an error.
pub async fn first_live(nodes: &[String], net: &crate::network::Network) -> Result<(KaspaRpcClient, usize), String> {
    let mut last = String::from("no --node given");
    for (i, url) in nodes.iter().enumerate() {
        match connect(Some(url), net).await {
            Ok(rpc) => return Ok((rpc, i)),
            Err(e) => {
                println!("{} node {url} did not answer: {e}", stamp());
                last = e;
            }
        }
    }
    Err(last)
}

/// Which part has gone quiet past STALE at `now`, if any. A racer stamp of 0 is no racer.
fn stale_part(now: u64, guard: u64, racer: u64) -> Option<&'static str> {
    if now.saturating_sub(guard) >= STALE {
        return Some("guard");
    }
    (racer != 0 && now.saturating_sub(racer) >= STALE).then_some("auto-stop racer")
}

/// A plain thread (it runs even when the async side hangs) that checks the stamps every 15 s and
/// ends the process when one is stale; capped at about 30 days of checks.
fn watchdog(dir: PathBuf) {
    std::thread::spawn(move || {
        for _ in 0..(30 * 24 * 240) {
            std::thread::sleep(Duration::from_secs(15));
            let (g, r) = (GUARD_OK.load(Ordering::Relaxed), RACER_OK.load(Ordering::Relaxed));
            if let Some(part) = stale_part(unix_now(), g, r) {
                let why = format!("no good answer to the {part} for {STALE} s");
                let line = format!("{} ALERT WATCHER RESTART: {why}; exiting so it restarts", stamp());
                println!("{line}");
                log_line(&dir, &line);
                std::process::exit(RESTART_CODE);
            }
        }
    });
}

/// The guard went down: the alerts.log line, the WatcherSilent alert in alerts.json and the push.
fn watcher_down(dir: &Path, msg: &str) {
    let line = format!("{} ALERT WATCHER DOWN: {msg}", stamp());
    println!("{line}");
    log_line(dir, &line);
    let mut alerts = AlertFile::load(dir);
    alerts.add(Kind::WatcherSilent, "The guard stopped unexpectedly. Check the wallet device.");
    alerts.save(dir);
    push(&format!("WATCHER DOWN: {msg}"));
}

/// The library form (the guard inside the phone app, D5): the same body as `run`, but every refusal is
/// an `Err` and nothing exits the process. `poll` is the seconds between ticks, read every tick;
/// `stop` ends the loop at the next tick (a dropped sender too); `woke` is notified after every saved
/// alert and every good answer. Returns Ok(()) when the vault closes or `stop` is pulled.
pub async fn run_guard(
    ctx: Ctx,
    nodes: Vec<String>,
    auto_heir: bool,
    poll: Arc<AtomicU64>,
    stop: tokio::sync::watch::Receiver<bool>,
    woke: Arc<Notify>,
) -> Result<(), String> {
    run_guard_with(ctx, nodes, auto_heir, poll, stop, woke, None).await
}

/// `run_guard` with the laptop-only `--auto-stop` (the CLI's `watch`; the phone's guard never holds
/// a hot stop key). With it the folder's state.json follows every move, so a stop typed by hand
/// still works.
#[allow(clippy::too_many_arguments)]
pub async fn run_guard_with(
    ctx: Ctx,
    nodes: Vec<String>,
    auto_heir: bool,
    poll: Arc<AtomicU64>,
    mut stop: tokio::sync::watch::Receiver<bool>,
    woke: Arc<Notify>,
    auto_stop: Option<AutoStop>,
) -> Result<(), String> {
    if let Some(a) = &auto_stop {
        a.check()?;
        ctx.keys.try_stop_key(crate::keys::stop_slot(a.which)?)?;
    }
    let ctx = &ctx;
    let nodes = &nodes[..];
    if auto_heir && !ctx.keys.public().auto_heir {
        return Err("This vault was set up without the auto-heir dial; nothing to send.".into());
    }
    let v = ctx.try_load_vault()?.ok_or_else(|| format!("state.json is missing from {}: the guard starts from the vault's covenant id and last known state.", ctx.dir.display()))?;
    // Watcher::new panics on unreadable dials or contract source; said here as an error instead.
    ctx.dials()?;
    ctx.source()?;
    let mut others = vec![];
    for url in nodes.iter().skip(1) {
        // A node that does not answer now is left out (the guard runs on the ones that do); the restart
        // loop brings it back. A node on another network is still refused outright.
        match connect(Some(url), &ctx.network).await {
            Ok(rpc) => others.push((url.clone(), rpc)),
            Err(e) if e.contains("It is lying or set up wrong") => return Err(e),
            Err(e) => {
                let line = format!("{} ALERT witness {url} left out for now: {e}", stamp());
                println!("{line}");
                log_line(&ctx.dir, &line);
            }
        }
    }
    // Watched-follow mode follows on the wallet's own connection; alarms come from the phone polling watch.status.
    let follow = ctx.dir.join(crate::watched::WATCHED_FILE).exists();
    if !follow && let Some(warning) = no_node_warning(nodes) {
        let line = format!("{} ALERT {warning}", stamp());
        println!("{line}");
        log_line(&ctx.dir, &line);
    }
    let primary = nodes.first().cloned().unwrap_or_else(|| "a public node (resolver)".into());
    let mut w = Watcher::new(ctx, v.covenant_id, v.state.clone(), primary, others);
    w.auto_heir = auto_heir;
    w.woke = woke;
    if let Some(a) = &auto_stop {
        w.follow = true;
        let after = match a.cap {
            Some(c) => {
                let hours = a.window_secs / 3600;
                format!("after {c} declines in {hours} h, cancel (all to the cold vault)")
            }
            None => "no cap: it only declines; only a decline that cannot be built cancels".into(),
        };
        let proof = ctx.dir.join(autostop::PROOF_FILE);
        let line = format!(
            "--auto-stop: stop key {} declines every withdrawal request; {after}; proof file {}",
            a.which,
            proof.display()
        );
        w.say(Level::Info, &line);
    }
    let (cov, state, primary) = (v.covenant_id, v.state.clone(), w.primary.clone());
    let racing = |a| race::race(ctx.clone(), cov, state, primary, a, stop.clone());
    let racer = auto_stop.map(|a| tokio::spawn(racing(a)));
    let mut racer_said = false;
    let resumed = Saved::load(&ctx.dir).filter(|s| same_account(s, &v)).map(|s| {
        let coin = s.coin.map(|(op, amount, _)| format!("coin {op} ({:.4} KAS)", kas(amount))).unwrap_or_else(|| "no coin in view".into());
        let line = format!("resuming from watch.json: stage {}, {coin}, block scan from {}", s.state.stage, s.sink.map(|h| h.to_string()).unwrap_or_else(|| "the first sink seen".into()));
        let line = match &s.provisional {
            Some(p) => format!("{line}; the spend {} was not yet accepted when the watcher stopped: starting from the state before it and looking again", p.txid),
            None => line,
        };
        w.resume(s);
        line
    });
    let witnesses = std::iter::once(w.primary.clone()).chain(w.others.iter().map(|(n, _)| n.clone())).collect::<Vec<_>>().join(" and ");
    w.say(
        Level::Info,
        &format!(
            "watcher started for covenant id {}; delay {} DAA (~{:.1} min); asking {witnesses}; alerts also go to {} and {}",
            w.cov,
            w.dials.delay,
            minutes(&ctx.network, w.dials.delay),
            ctx.dir.join("alerts.log").display(),
            ctx.dir.join("alerts.json").display()
        ),
    );
    match resumed {
        Some(line) => w.say(Level::Info, &line),
        None => w.say(Level::Info, "no watch.json: starting from state.json; a coin that is already gone cannot be traced until one tick has seen a sink"),
    }
    if auto_heir {
        let delay = ctx.keys.public().inherit_delay(&w.dials);
        w.say(Level::Info, &format!("--auto-heir: the inheritOpen goes out once the coin is {delay} DAA (~{:.1} min) old; a warning at 90%", minutes(&ctx.network, delay)));
    }
    if let Some(a) = w.alerts.alerts.first() {
        let line = format!("alerts.json holds {} alert(s), last good answer at {:?}; newest: {} at {}: {}", w.alerts.alerts.len(), w.alerts.seen_at, a.kind, a.at, a.text);
        w.say(Level::Info, &line);
    }
    loop {
        if *stop.borrow() {
            w.say(Level::Info, "guard stopped by its owner");
            return Ok(());
        }
        if !racer_said && racer.as_ref().is_some_and(|h| h.is_finished()) {
            racer_said = true;
            let raw = "AUTO-STOP racer ended; this guard goes on alerting only";
            w.alert(Kind::WatcherSilent, "The automatic emergency stop has ended.", raw);
        }
        match AssertUnwindSafe(w.tick()).catch_unwind().await {
            Ok(Tick::Go) => {
                let secs = poll.load(Ordering::Relaxed).max(1);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
                    changed = stop.changed() => {
                        // A pulled stop, or an owner that let go of the handle: the guard ends.
                        if changed.is_err() || *stop.borrow() {
                            w.say(Level::Info, "guard stopped by its owner");
                            return Ok(());
                        }
                    }
                }
            }
            Ok(Tick::Done(reason)) => {
                w.alert(Kind::Stopped, "The account is closed. The guard has finished.", &format!("WATCHER DOWN: {reason}; watcher done"));
                return Ok(());
            }
            Err(payload) => {
                let msg = format!("panic: {}", panic_text(&*payload));
                watcher_down(&ctx.dir, &msg);
                return Err(format!("The guard stopped unexpectedly ({msg})."));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{self, DELAY, KAS, WINDOW, locked};
    use crate::keys::Keys;
    use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
    use kaspa_consensus_core::tx::{CovenantBinding, TransactionInput, TransactionOutput, UtxoEntry};
    use kaspa_txscript::script_builder::ScriptBuilder;

    /// A push hook that cannot start fails at once; one that hangs is ended at the limit.
    #[test]
    fn a_hung_push_hook_is_ended() {
        assert!(!run_hook("kv-no-such-hook-program", "x", Duration::from_secs(5)));
        #[cfg(windows)]
        {
            let began = Instant::now();
            assert!(!run_hook("powershell", "Start-Sleep 60", Duration::from_secs(2)));
            assert!(began.elapsed() < Duration::from_secs(30), "the hook was not ended");
            assert!(run_hook("powershell", "exit 0", Duration::from_secs(30)));
        }
    }

    /// A state.json that cannot be read while the guard runs does not stop it: the guard
    /// still works out the account's script and judges a spend from what it read at the start.
    #[test]
    fn an_unreadable_state_file_does_not_stop_a_running_guard() {
        let ctx = test_ctx("nofile");
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let spk = ctx.vault_spk(&state);
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, spk.clone(), 1_000, false, Some(cov)));
        let tx = spend_tx(&ctx, &state, "cancel", vec![vec![0u8; 65], vec![]], &prev, vec![TransactionOutput::new(2 * KAS - 1000, ctx.cold_spk())]);
        let w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        std::fs::write(ctx.state_path(), "{ half a file").unwrap();
        std::fs::remove_file(ctx.dir.join(contract::SOURCE_FILE)).unwrap();
        assert!(ctx.try_compile(&state).is_err(), "the folder itself can no longer compile");
        assert_eq!(w.spk(&state), spk);
        assert!(w.judge(&tx, &prev).closed, "a stop to the vault is still read as one");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// A full list drops its oldest network alert to make room, and a funds alert only when
    /// nothing else is left to drop.
    #[test]
    fn network_alerts_never_push_out_a_funds_alert() {
        let mut f = AlertFile::default();
        f.add(Kind::WithdrawalPending, "a withdrawal is waiting");
        for _ in 0..3 * ALERTS_KEPT {
            f.add(Kind::LineageMismatch, "two answers");
        }
        assert_eq!(f.alerts.len(), ALERTS_KEPT);
        assert_eq!(f.alerts.last().unwrap().kind, "withdrawal_pending");
        assert_eq!(f.alerts[0].kind, "lineage_mismatch", "newest first");
        for _ in 0..ALERTS_KEPT {
            f.add(Kind::Spend, "paid");
        }
        assert_eq!(f.alerts.len(), ALERTS_KEPT);
        assert!(f.alerts.iter().all(|a| a.kind == "spend"), "only funds alerts left: the oldest goes");
    }

    /// A lasting network fault is said at its start and then once per REMIND; one that
    /// comes back within STALL of the last saying stays quiet.
    #[test]
    fn a_lasting_fault_is_said_once_then_slowly() {
        assert!(say_fault(false, None), "the first time");
        assert!(!say_fault(true, Some(STALL)), "same fault a minute on: quiet");
        assert!(!say_fault(true, Some(REMIND - Duration::from_secs(1))));
        assert!(say_fault(true, Some(REMIND)), "the slow reminder");
        assert!(say_fault(false, Some(STALL)), "a new fault after a quiet minute");
        assert!(!say_fault(false, Some(STALL - Duration::from_secs(1))), "a flicker");
    }

    /// A node call that never answers comes back as a failure once the wait is over; an answer
    /// and a node's own error pass through unchanged.
    #[tokio::test]
    async fn a_hung_node_call_fails_in_time() {
        let wait = Duration::from_millis(50);
        let began = Instant::now();
        let hung = in_time_for(wait, std::future::pending::<Result<u8, String>>()).await;
        assert_eq!(hung, Err("no answer in 0 s".to_string()));
        assert!(began.elapsed() < Duration::from_secs(5), "the cap did not end the wait");
        assert_eq!(in_time_for(wait, async { Ok::<u8, String>(7) }).await, Ok(7));
        let refused = in_time_for(wait, async { Err::<u8, String>("refused".into()) }).await;
        assert_eq!(refused, Err("refused".to_string()));
    }

    #[test]
    fn watchdog_trips_on_a_quiet_guard_or_racer_only() {
        assert_eq!(stale_part(1000, 950, 0), None, "fresh guard, no racer");
        assert_eq!(stale_part(1000, 905, 0), Some("guard"));
        assert_eq!(stale_part(1000, 950, 950), None);
        assert_eq!(stale_part(1000, 950, 900), Some("auto-stop racer"));
        assert_eq!(stale_part(5, 10, 10), None, "a stamp ahead of the clock is not stale");
    }

    #[test]
    fn check_nodes_wants_two_different_witnesses() {
        let one = vec!["ws://a:1".to_string()];
        let same = vec!["ws://a:1".to_string(), "ws://a:1 ".to_string()];
        let two = vec!["ws://a:1".to_string(), "ws://b:1".to_string()];
        assert!(check_nodes(&[]).unwrap_err().contains("two different --node"));
        assert!(check_nodes(&one).unwrap_err().contains("1 given"));
        assert!(check_nodes(&same).is_err(), "the same url twice is one witness");
        assert!(check_nodes(&two).is_ok());
    }

    #[test]
    fn hostile_sigscripts_decode_to_none_not_panic() {
        assert_eq!(pushes(&[]), Some(vec![]));
        assert_eq!(pushes(&[0x01]), None, "1-byte push with no data");
        assert_eq!(pushes(&[0x02, 0xaa]), None, "truncated push");
        assert_eq!(pushes(&[0x4c]), None, "pushdata1 without length");
        assert_eq!(pushes(&[0x4c, 0x05, 1, 2]), None, "pushdata1 truncated");
        assert_eq!(pushes(&[0x4d, 0x10]), None, "pushdata2 half length");
        assert_eq!(pushes(&[0x4d, 0x10, 0x00, 1, 2]), None, "pushdata2 truncated");
        assert_eq!(pushes(&[0x4e, 0xff, 0xff, 0xff, 0xff]), None, "pushdata4 absurd length");
        assert_eq!(pushes(&[0x01, 0x07, 0x00, 0x51, 0x4f]), Some(vec![vec![7], vec![], vec![1], vec![0x81]]));
        assert_eq!(pushes(&[0x01, 0x07, 0x00, 0x51, 0x4f, 0xac]), None, "a non-push opcode (OP_CHECKSIG) is unreadable, not an empty push");
        assert_eq!(pushes(&[0xac]), None, "fail closed on an unknown opcode alone");
        assert_eq!(pushes(&[0x01, 0x07, 0x61]), None, "OP_NOP after a push is still unreadable");
        assert_eq!(script_int(&[]), Some(0));
        assert_eq!(script_int(&[0x81]), Some(-1));
        assert_eq!(script_int(&[0xe8, 0x03]), Some(1000));
        assert_eq!(script_int(&[1, 2, 3, 4, 5, 6, 7, 8]), Some(0x0807060504030201));
        assert_eq!(script_int(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), None, "9 bytes would shift past 64 bits");
        assert!(decode_spk(&[]).is_none() && decode_spk(&[0]).is_none());
        assert_eq!(decode_spk(&[0, 0]).map(|s| s.script().len()), Some(0));
        let sig = vec![0u8; 65];
        assert!(decode_request(&[sig.clone(), vec![0x00], vec![0x01], vec![1], vec![2]]).is_none(), "1-byte newDestSpk is unreadable");
        assert!(decode_request(&[sig.clone(), vec![0, 0, 0x20], vec![0x01]]).is_none(), "too few pushes");
        assert!(decode_request(&[sig.clone(), vec![0, 0, 0x20], vec![1, 2, 3, 4, 5, 6, 7, 8, 9], vec![1], vec![2]]).is_none(), "9-byte amount");
        let (spk, amount) = decode_request(&[sig.clone(), vec![0, 0, 0x20, 0xaa], vec![0xe8, 0x03], vec![1], vec![2]]).unwrap();
        assert_eq!((spk.version(), spk.script().to_vec(), amount), (0, vec![0x20, 0xaa], 1000));
        assert_eq!(decode_cancel_slot(&[sig.clone(), vec![0x01], vec![1], vec![2]]), Some(1));
        assert_eq!(decode_cancel_slot(&[sig.clone(), vec![], vec![1], vec![2]]), Some(0));
        assert_eq!(decode_cancel_slot(&[sig, vec![1], vec![2]]), None);
    }

    fn test_ctx(tag: &str) -> Ctx {
        let dir = std::env::temp_dir().join(format!("kv-watch-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        contract::pin_source(&dir).unwrap();
        Ctx { dir, keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 }
    }

    /// A spending tx of the vault coin `prev` through `entry` with the given argument pushes and outputs.
    fn spend_tx(ctx: &Ctx, state: &State, entry: &str, args: Vec<Vec<u8>>, prev: &Coin, outputs: Vec<TransactionOutput>) -> RpcTransaction {
        let art = ctx.compile(state);
        let tag = art.contracts.values().next().unwrap().entries[entry].dispatch_tag.as_bytes().to_vec();
        let mut b = ScriptBuilder::new();
        for push in args.into_iter().chain([tag, contract::bytecode(&art)]) {
            b.add_data(&push).unwrap();
        }
        let inputs = vec![TransactionInput::new(prev.0, b.drain(), 0, 0)];
        (&Transaction::new(1, inputs, outputs, 0, SUBNETWORK_ID_NATIVE, 0, vec![])).into()
    }

    /// D5: the library form never exits the process: a folder with no state.json is an Err in words.
    #[tokio::test]
    async fn run_guard_returns_err_without_state_json() {
        let ctx = test_ctx("noguard");
        std::fs::remove_file(ctx.dir.join("state.json")).ok();
        let poll = Arc::new(AtomicU64::new(1));
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let err = run_guard(ctx.clone(), vec!["ws://a:1".into(), "ws://b:1".into()], false, poll, rx, Arc::new(Notify::new())).await.unwrap_err();
        assert!(err.contains("state.json"), "{err}");
        // auto-heir on a vault made without the dial is a refusal too, said before any node is asked
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let err = run_guard(ctx.clone(), vec![], true, Arc::new(AtomicU64::new(1)), rx, Arc::new(Notify::new())).await.unwrap_err();
        assert!(err.contains("auto-heir"), "{err}");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// Pass 8: alerts.json has the shape the phone reads, newest first, at most ten, and survives a reload.
    #[test]
    fn alerts_json_shape_newest_first_capped_at_ten() {
        let dir = std::env::temp_dir().join(format!("kv-alerts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = AlertFile::default();
        assert_eq!(f.seen_at, None);
        for i in 1..=12 {
            f.add(if i % 2 == 0 { Kind::WithdrawalPending } else { Kind::LineageMismatch }, &format!("alert {i}"));
        }
        f.seen_at = Some(1_700_000_000);
        assert_eq!(f.alerts.len(), 10);
        // the two that left are the oldest network alerts (1 and 3); funds alert 2 stays
        assert_eq!((f.alerts[0].text.as_str(), f.alerts[9].text.as_str()), ("alert 12", "alert 2"));
        f.save(&dir);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("alerts.json")).unwrap()).unwrap();
        assert_eq!(v["seen_at"], 1_700_000_000);
        let alerts = v["alerts"].as_array().unwrap();
        assert_eq!(alerts.len(), 10);
        let kinds = [
            "withdrawal_pending", "withdrawal_completed", "lineage_mismatch", "watcher_silent",
            "unchecked", "unwatched", "stopped", "withdrawal_declined", "heir_clock", "spend",
            "spend_stranger",
        ];
        for a in alerts {
            assert_eq!(a.as_object().unwrap().len(), 3, "{a}");
            assert!(a["at"].is_u64() && a["text"].is_string(), "{a}");
            assert!(kinds.contains(&a["kind"].as_str().unwrap()), "{a}");
        }
        assert_eq!(alerts[0]["kind"], "withdrawal_pending");
        assert_eq!(AlertFile::load(&dir), f, "reload keeps the list as written");
        assert!(!dir.join("alerts.json.tmp").exists(), "the temp file is renamed away");
        let all = [
            Kind::WithdrawalPending, Kind::WithdrawalCompleted, Kind::LineageMismatch,
            Kind::WatcherSilent, Kind::Unchecked, Kind::Unwatched, Kind::Stopped,
            Kind::WithdrawalDeclined, Kind::HeirClock, Kind::Spend, Kind::SpendStranger,
        ];
        for k in all {
            assert!(kinds.contains(&k.as_str()));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Pass 8: the 90 s rule, driven by an injected clock: one alert per silence, a recovery when it ends.
    #[test]
    fn silence_alerts_once_after_90s_and_recovers() {
        let s = Duration::from_secs;
        let mut l = Liveness::new();
        assert!(!l.bad(s(30)), "30 s without an answer is not yet silence");
        assert!(!l.bad(s(89)));
        assert!(l.bad(s(90)), "90 s is the alert");
        assert!(!l.bad(s(100)) && !l.bad(s(1000)), "said once per silence");
        assert_eq!(l.good(s(1200)), Some(s(1200)), "recovery reports the silence length");
        assert_eq!(l.good(s(1203)), None, "a good answer in good times says nothing");
        assert!(!l.bad(s(1290)), "87 s since the last good answer");
        assert!(l.bad(s(1293)), "90 s since the last good answer: a new silence");
    }

    /// With two nodes, one going quiet while the other answers is an alert after
    /// an hour, once, and its return is reported; a lone node is left to the all-nodes rule.
    #[test]
    fn one_quiet_witness_alerts_once_and_recovers() {
        let s = Duration::from_secs;
        let mut two = vec![Liveness::after(UNCHECKED), Liveness::after(UNCHECKED)];
        let none: (Vec<usize>, Vec<(usize, Duration)>) = (vec![], vec![]);
        assert_eq!(quiet_witnesses(&mut two, &[false, false], s(10)), none);
        assert_eq!(quiet_witnesses(&mut two, &[false, true], s(1800)), none, "half an hour");
        assert_eq!(quiet_witnesses(&mut two, &[false, true], s(3700)).0, vec![1], "an hour");
        assert_eq!(quiet_witnesses(&mut two, &[false, true], s(9000)), none, "said once");
        assert_eq!(quiet_witnesses(&mut two, &[false, false], s(9100)).1, vec![(1, s(9090))]);
        assert_eq!(quiet_witnesses(&mut two, &[true, false], s(13000)).0, vec![0], "either node");
        let mut one = vec![Liveness::after(UNCHECKED)];
        assert_eq!(quiet_witnesses(&mut one, &[true], s(50000)), none, "a lone node: the old rule");
    }

    /// Pass 8: watch.json round-trips what a restart needs.
    #[test]
    fn saved_state_round_trips() {
        let dir = std::env::temp_dir().join(format!("kv-saved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Saved::load(&dir).is_none());
        let s = Saved {
            state: State { stage: 1, dest_hash: vec![7; 32], amount: 5, window_start: 9, spent: 2 },
            coin: Some((TransactionOutpoint::new(Hash::from_bytes([4u8; 32]), 1), 3 * KAS, 777)),
            sink: Some(Hash::from_bytes([8u8; 32])),
            daa: 1234,
            closed: None,
            open_sent: Some("abc".into()),
            provisional: Some(Provisional {
                txid: "def".into(),
                prev_state: locked(),
                prev_coin: (TransactionOutpoint::new(Hash::from_bytes([2u8; 32]), 0), 4 * KAS, 700),
                low: Hash::from_bytes([6u8; 32]),
            }),
            cov: Some(Hash::from_bytes([5u8; 32])),
        };
        s.save(&dir);
        assert_eq!(Saved::load(&dir), Some(s.clone()));
        std::fs::remove_dir_all(&dir).ok();
        // A reopened account (new covenant id) does not resume the old account's record.
        let open = |cov: u8, stage: i64| Vault { covenant_id: Hash::from_bytes([cov; 32]),
            state: State { stage, ..s.state.clone() }, pending_to: None, history: vec![],
            contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        assert!(same_account(&s, &open(5, 1)));
        assert!(!same_account(&s, &open(6, 0)));
        let old = Saved { cov: None, state: State { stage: 9, ..s.state.clone() }, ..s.clone() };
        assert!(!same_account(&old, &open(6, 0)), "old closed record, open account: not resumed");
        assert!(same_account(&old, &open(6, 9)));
    }

    /// a `cancel` paying a stranger is an alert about the rules, never
    /// "CANCELLED ... sent to cold"; one paying the cold address closes the vault.
    #[test]
    fn cancel_to_a_stranger_is_an_alert_not_cancelled() {
        let ctx = test_ctx("cancel");
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let args = vec![vec![0u8; 65], vec![]];
        let stranger = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        let tx = spend_tx(&ctx, &state, "cancel", args.clone(), &prev, vec![TransactionOutput::new(2 * KAS - 1000, stranger)]);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind, o.closed, o.next.is_none()), (Level::Alert, Kind::LineageMismatch, false, true));
        assert!(o.text.contains("not the cold address") && !o.text.contains("CANCELLED"), "{}", o.text);
        assert_eq!(o.bank, RULES_BROKEN);
        let tx = spend_tx(&ctx, &state, "cancel", args, &prev, vec![TransactionOutput::new(2 * KAS - 1000, ctx.cold_spk())]);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind, o.closed), (Level::Alert, Kind::Stopped, true));
        assert!(o.text.contains("CANCELLED by stop key 1"), "{}", o.text);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// v5: a `decline` continues the lineage with after_decline (Locked, destination wiped, window
    /// counters kept) and names the stop key; a continuation with any other state is a lineage alert.
    #[test]
    fn decline_continues_locked_and_names_the_stop_key() {
        let ctx = test_ctx("decline");
        let cov = Hash::from_bytes([3u8; 32]);
        let dest = ScriptPublicKey::from_vec(0, vec![0xbb; 34]);
        let state = State { stage: 1, dest_hash: spk_hash(&dest), amount: KAS as i64, window_start: 7, spent: 5 };
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(3 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        // stop key 2 (slot 1 on chain): [sig, slot, tag, bytecode]
        let args = vec![vec![0u8; 65], vec![1]];
        let cont = |v: u64, s: &State| TransactionOutput::with_covenant(v, ctx.vault_spk(s), Some(CovenantBinding::new(0, cov)));
        let tx = spend_tx(&ctx, &state, "decline", args.clone(), &prev, vec![cont(3 * KAS - 1000, &after_decline(&state))]);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind, o.closed), (Level::Alert, Kind::WithdrawalDeclined, false));
        assert_eq!(o.next, Some(State { stage: 0, dest_hash: vec![0; 32], amount: 0, window_start: 7, spent: 5 }));
        assert_eq!(o.bank, "A waiting withdrawal was declined by stop key 2; the account is Locked again.");
        assert!(o.text.contains("DECLINED by stop key 2") && o.text.contains("keeps 3.0000 KAS"), "{}", o.text);
        // the same lineage with the pending state kept is not a decline the rules allow
        let tx = spend_tx(&ctx, &state, "decline", args, &prev, vec![cont(3 * KAS - 1000, &state)]);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.kind, o.next.is_none()), (Kind::LineageMismatch, true));
        assert!(o.text.contains("lineage check FAILED"), "{}", o.text);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// v5: `inheritOpen` ends the lineage like inherit; from Pending the recorded destination is paid
    /// first and the heir after it; anything else paid to the heir slot is an alert.
    #[test]
    fn inherit_open_ends_the_lineage_at_the_heir() {
        let ctx = test_ctx("inheritopen");
        let cov = Hash::from_bytes([3u8; 32]);
        let heir = ctx.network.to_spk(&ctx.keys.public().heir_address);
        let state = locked();
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        let tx = spend_tx(&ctx, &state, "inheritOpen", vec![], &prev, vec![TransactionOutput::new(2 * KAS - 1000, heir.clone())]);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind, o.closed, o.next.is_none()), (Level::Alert, Kind::Stopped, true, true));
        assert_eq!(o.bank, "The heir received the account after the silence passed (sent without a key).");
        assert!(o.text.contains("INHERITED (open, no key)"), "{}", o.text);
        let stranger = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let tx = spend_tx(&ctx, &state, "inheritOpen", vec![], &prev, vec![TransactionOutput::new(2 * KAS - 1000, stranger.clone())]);
        let o = w.judge(&tx, &prev);
        assert!(o.kind == Kind::LineageMismatch && !o.closed && o.text.contains("not the heir's address"), "{}", o.text);
        // from Pending: the recorded destination first, then the heir
        let dest = ScriptPublicKey::from_vec(0, vec![0xbb; 34]);
        let pending = State { stage: 1, dest_hash: spk_hash(&dest), amount: KAS as i64, ..locked() };
        let prev: Coin = (prev.0, UtxoEntry::new(3 * KAS, ctx.vault_spk(&pending), 1_000, false, Some(cov)));
        let w = Watcher::new(&ctx, cov, pending.clone(), "test".into(), vec![]);
        let tx = spend_tx(&ctx, &pending, "inheritOpen", vec![], &prev, vec![TransactionOutput::new(KAS, dest.clone()), TransactionOutput::new(2 * KAS - 1000, heir.clone())]);
        let o = w.judge(&tx, &prev);
        assert!(o.kind == Kind::Stopped && o.closed && o.text.contains("2.0000 KAS sent to heir"), "{}", o.text);
        let tx = spend_tx(&ctx, &pending, "inheritOpen", vec![], &prev, vec![TransactionOutput::new(KAS, stranger), TransactionOutput::new(2 * KAS - 1000, heir)]);
        let o = w.judge(&tx, &prev);
        assert!(o.kind == Kind::LineageMismatch && o.text.contains("not the destination and amount recorded"), "{}", o.text);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// A plain `inherit` during Pending pays the recorded destination at output 0 and the
    /// heir at output 1; the watcher reports both and closes the vault instead of crying "rules broken".
    #[test]
    fn inherit_from_pending_reports_the_destination_and_closes() {
        let ctx = test_ctx("inherit-pending");
        let cov = Hash::from_bytes([3u8; 32]);
        let heir = ctx.network.to_spk(&ctx.keys.public().heir_address);
        let dest = ScriptPublicKey::from_vec(0, vec![0xbb; 34]);
        let pending = State { stage: 1, dest_hash: spk_hash(&dest), amount: KAS as i64, ..locked() };
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(3 * KAS, ctx.vault_spk(&pending), 1_000, false, Some(cov)));
        let w = Watcher::new(&ctx, cov, pending.clone(), "test".into(), vec![]);
        let tx = spend_tx(&ctx, &pending, "inherit", vec![vec![0u8; 65]], &prev, vec![TransactionOutput::new(KAS, dest.clone()), TransactionOutput::new(2 * KAS - 1000, heir.clone())]);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind, o.closed, o.next.is_none()), (Level::Alert, Kind::Stopped, true, true), "{}", o.text);
        assert!(o.text.contains("INHERITED: 2.0000 KAS sent to heir") && o.text.contains(&format!("the pending 1.0000 KAS went to {}", addr_of(&ctx.network, &dest))), "{}", o.text);
        assert_eq!(o.bank, "The account was closed and everything went to the heir. The waiting withdrawal of 1.0000 KAS went to its destination.");
        assert!(!o.text.contains("rules") && !o.bank.contains("audit"), "{}", o.text);
        // the heir alone at output 0 while Pending skips the destination: that is the broken shape
        let tx = spend_tx(&ctx, &pending, "inherit", vec![vec![0u8; 65]], &prev, vec![TransactionOutput::new(3 * KAS - 1000, heir.clone())]);
        let o = w.judge(&tx, &prev);
        assert!(o.kind == Kind::LineageMismatch && !o.closed && o.text.contains("not the destination and amount recorded"), "{}", o.text);
        // the right destination with the wrong amount is just as broken
        let tx = spend_tx(&ctx, &pending, "inherit", vec![vec![0u8; 65]], &prev, vec![TransactionOutput::new(KAS / 2, dest), TransactionOutput::new(2 * KAS, heir)]);
        let o = w.judge(&tx, &prev);
        assert!(o.kind == Kind::LineageMismatch && !o.closed, "{}", o.text);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// A block is not final. A spender in a block the virtual chain has not accepted
    /// is provisional like a mempool sighting (pre-state kept, text says so); only an accepted one
    /// is applied for good. `is_accepted` is the pure test against the node's accepted ids.
    #[test]
    fn block_sighting_is_final_only_when_accepted() {
        let a = Hash::from_bytes([1u8; 32]);
        let b = Hash::from_bytes([2u8; 32]);
        assert!(is_accepted(&a.to_string(), [&a, &b]));
        assert!(!is_accepted(&Hash::from_bytes([9u8; 32]).to_string(), [&a, &b]));
        assert!(!is_accepted(&a.to_string(), []));
        let ctx = test_ctx("accepted");
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let stranger = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let tx = spend_to(&ctx, &state, cov, &prev, KAS / 2, stranger);
        let mut w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        w.coin = Some(prev.clone());
        w.apply(&tx, &prev, Seen::Block);
        let u = w.unconfirmed.clone().expect("a block the chain has not accepted is provisional");
        assert_eq!((u.prev_state.clone(), u.prev.0), (state.clone(), prev.0));
        assert_ne!(w.state, state, "the derived state is followed provisionally");
        let log = std::fs::read_to_string(ctx.dir.join("alerts.log")).unwrap();
        assert!(log.contains("in a block the network has not accepted yet"), "{log}");
        let mut w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        w.apply(&tx, &prev, Seen::Accepted);
        assert!(w.unconfirmed.is_none(), "an accepted spend is final");
        assert_ne!(w.state, state);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// A catch-up walk pages until a page brings nothing new, and when the coin's
    /// spend cannot be found the addresses the rules lead to are known for Pending (one Locked
    /// state for finish and decline) and unknowable from Locked.
    #[test]
    fn catch_up_pages_until_the_tip_and_knows_the_next_addresses() {
        let low = Hash::from_bytes([1u8; 32]);
        let a = Hash::from_bytes([2u8; 32]);
        let b = Hash::from_bytes([3u8; 32]);
        assert_eq!(next_page(&[low, a, b], low), Some(b), "continue from the newest block of the page");
        assert_eq!(next_page(&[low], low), None, "a page holding only the start is the tip");
        assert_eq!(next_page(&[], low), None, "an empty page is the tip");
        assert_eq!(next_page(&[a, b], low), Some(b), "a page without the start still moves on");
        assert_eq!(next_page(&[b], b), None);
        let pending = State { stage: 1, dest_hash: vec![7; 32], amount: 5, window_start: 9, spent: 2 };
        let next = next_states(&pending);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].0, "finished or declined");
        assert_eq!(next[0].1, after_finish(&pending));
        assert_eq!(next[0].1, after_decline(&pending));
        assert!(next_states(&locked()).is_empty(), "from Locked the next address depends on a request or a spend amount");
    }

    /// One node showing a spender the others never show is held, then an alert, once.
    #[test]
    fn one_node_showing_a_spender_is_an_alert_after_a_few_looks() {
        let ctx = test_ctx("one-only");
        let dir = ctx.dir.clone();
        let op = TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0);
        let x = "x".to_string();
        let mut w = Watcher::new(&ctx, Hash::from_bytes([3u8; 32]), locked(), "n1".into(), vec![]);
        let look = |w: &mut Watcher<'_>| {
            w.agree(&[("n1".into(), x.clone())], &["n2".into()], &op, "a block")
        };
        for _ in 1..WAITING_TICKS {
            assert_eq!(look(&mut w), Verdict::Waiting);
        }
        assert!(AlertFile::load(&dir).alerts.is_empty(), "a node a little behind is not an alert");
        assert_eq!(look(&mut w), Verdict::Waiting);
        let json = AlertFile::load(&dir);
        assert_eq!((json.alerts.len(), json.alerts[0].text.as_str()), (1, TWO_ANSWERS));
        let log = std::fs::read_to_string(dir.join("alerts.log")).unwrap();
        assert!(log.contains("ONE NODE ONLY: n1 shows the spender x of"), "{log}");
        look(&mut w);
        assert_eq!(AlertFile::load(&dir).alerts.len(), 1, "one alert, not one per look");
        // the nodes agree again: the count starts over
        let both = [("n1".to_string(), x.clone()), ("n2".to_string(), x.clone())];
        assert_eq!(w.agree(&both, &[], &op, "a block"), Verdict::Agreed);
        assert_eq!(w.waiting, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One node alone moves nothing. The spender counts only when every reachable
    /// node names the same one; a lagging node means wait (no alert), two spenders is the
    /// "two answers" alert and a hold; the coin is gone only when every node says so.
    #[test]
    fn one_node_alone_moves_nothing() {
        let x = "x".to_string();
        let y = "y".to_string();
        assert_eq!(verdict(&[], 2), Verdict::Nothing);
        assert_eq!(verdict(&[x.clone()], 1), Verdict::Waiting);
        assert_eq!(verdict(&[x.clone()], 0), Verdict::Agreed);
        assert_eq!(verdict(&[x.clone(), x.clone()], 0), Verdict::Agreed);
        assert_eq!(verdict(&[x.clone(), y.clone()], 0), Verdict::Split);
        assert_eq!(verdict(&[x.clone(), y.clone()], 1), Verdict::Split);
        let op = TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0);
        let other = TransactionOutpoint::new(Hash::from_bytes([6u8; 32]), 0);
        assert!(gone_everywhere(&[vec![], vec![other]], 2, &op));
        assert!(!gone_everywhere(&[vec![], vec![op]], 2, &op), "one node still shows it: not gone");
        assert!(!gone_everywhere(&[], 1, &op), "no answer is not gone");
        let ctx = test_ctx("one-node");
        let dir = ctx.dir.clone();
        let mut w = Watcher::new(&ctx, Hash::from_bytes([3u8; 32]), locked(), "n1".into(), vec![]);
        assert_eq!(w.agree(&[("n1".into(), x.clone())], &["n2".into()], &op, "a block"), Verdict::Waiting);
        assert!(AlertFile::load(&dir).alerts.is_empty(), "a lagging node is not an alert");
        assert_eq!(w.agree(&[("n1".into(), x.clone()), ("n2".into(), y)], &[], &op, "a block"), Verdict::Split);
        let json = AlertFile::load(&dir);
        assert_eq!((json.alerts.len(), json.alerts[0].text.as_str()), (1, TWO_ANSWERS));
        let log = std::fs::read_to_string(dir.join("alerts.log")).unwrap();
        assert!(log.contains("NODES DISAGREE about who spent") && log.contains("holding the last agreed state"), "{log}");
        assert_eq!(w.agree(&[("n1".into(), x.clone()), ("n2".into(), x)], &[], &op, "a block"), Verdict::Agreed);
        assert_eq!(AlertFile::load(&dir).alerts.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A walk the page cap stops resumes from its last page, not from where
    /// the sinks start; the cursor outlives the sink history and a restart, and clears only when the
    /// tip is reached with every node answering, or the spender is agreed.
    #[test]
    fn capped_walk_resumes_from_its_last_page() {
        let low = Hash::from_bytes([1u8; 32]);
        let a = Hash::from_bytes([2u8; 32]);
        let b = Hash::from_bytes([3u8; 32]);
        assert_eq!(step(&[low, a, b], low, 1), Step::Next(b));
        assert_eq!(step(&[low, a, b], low, SCAN_PAGES_MAX), Step::Capped(b), "the last page with more to walk remembers where");
        assert_eq!(step(&[low], low, SCAN_PAGES_MAX), Step::Tip);
        assert_eq!(next_walk(Verdict::Nothing, low, Some(b), false), Some(b), "nothing yet: continue from the last page");
        assert_eq!(next_walk(Verdict::Nothing, low, None, false), None, "the tip, every node answered: caught up");
        assert_eq!(next_walk(Verdict::Nothing, low, Some(b), true), Some(low), "a node did not answer: hold at low");
        assert_eq!(next_walk(Verdict::Waiting, low, Some(b), false), Some(low));
        assert_eq!(next_walk(Verdict::Split, low, None, false), Some(low));
        assert_eq!(next_walk(Verdict::Agreed, low, Some(b), false), None);
        let ctx = test_ctx("cursor");
        let cov = Hash::from_bytes([3u8; 32]);
        let mut w = Watcher::new(&ctx, cov, locked(), "n1".into(), vec![]);
        w.sinks.push_back(low);
        assert_eq!(w.scan_start(), Some(low), "no cursor: the walk starts at the oldest sink");
        w.scan_from = next_walk(Verdict::Nothing, low, Some(a), false);
        for i in 0..(SINK_HISTORY as u8 + 2) {
            w.good_answer(u64::from(i), Hash::from_bytes([40 + i; 32]));
        }
        assert_ne!(w.sinks.front(), Some(&low), "the sink history has moved on");
        assert_eq!(w.scan_start(), Some(a), "the walk resumes from its last page, not from the sinks");
        w.persist(7);
        let saved = Saved::load(&ctx.dir).unwrap();
        assert_eq!(saved.sink, Some(a), "a restart continues the walk too");
        let mut fresh = Watcher::new(&ctx, cov, locked(), "n1".into(), vec![]);
        fresh.resume(saved);
        assert_eq!(fresh.scan_start(), Some(a));
        w.scan_from = next_walk(Verdict::Nothing, a, None, false);
        assert_eq!(w.scan_start(), w.sinks.front().copied(), "caught up: back to the sinks");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// A node whose call failed has not answered; the coin is gone, or back,
    /// only when every configured node says so, and the walk holds where it is.
    #[test]
    fn a_node_that_did_not_answer_holds_the_state() {
        let op = TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0);
        assert!(!gone_everywhere(&[vec![]], 2, &op), "one of two nodes answered: not gone");
        assert!(gone_everywhere(&[vec![], vec![]], 2, &op));
        assert!(!gone_everywhere(&[vec![], vec![op]], 2, &op));
        assert!(!gone_everywhere(&[], 1, &op));
        assert!(!all_show(&[vec![op]], 2, &op), "one of two nodes answered: not back");
        assert!(all_show(&[vec![op], vec![op]], 2, &op));
        assert!(!all_show(&[vec![op], vec![]], 2, &op));
        assert!(!all_show(&[], 1, &op));
        let low = Hash::from_bytes([1u8; 32]);
        assert_eq!(next_walk(Verdict::Nothing, low, None, true), Some(low), "a failed call holds the walk at low");
        assert_eq!(verdict(&[], 1), Verdict::Nothing, "no sighting anywhere is nothing, however many wait");
    }

    /// A provisional spend is saved with its rollback (the state and coin before it,
    /// and the scan start), and a restart does not believe it: it resumes at the previous state with
    /// the previous coin and scans from the saved start, so the spend is re-found or found gone.
    #[test]
    fn provisional_state_is_saved_with_its_rollback_and_restored() {
        let ctx = test_ctx("provisional");
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let low = Hash::from_bytes([8u8; 32]);
        let newer = Hash::from_bytes([9u8; 32]);
        let mut w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        w.sinks.push_back(low);
        w.sinks.push_back(newer);
        w.coin = Some(prev.clone());
        w.persist(50);
        assert!(Saved::load(&ctx.dir).unwrap().provisional.is_none(), "nothing provisional: nothing to roll back");
        let stranger = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let tx = spend_to(&ctx, &state, cov, &prev, KAS / 2, stranger);
        w.apply(&tx, &prev, Seen::Mempool);
        assert!(w.unconfirmed.is_some() && w.state != state && w.coin.is_none());
        w.persist(60);
        let saved = Saved::load(&ctx.dir).unwrap();
        let p = saved.provisional.clone().expect("the rollback is saved alongside");
        let id = txid_of(&tx);
        assert_eq!((p.txid.as_str(), &p.prev_state, p.prev_coin, p.low), (id.as_str(), &state, (prev.0, 2 * KAS, 1_000), low));
        assert_ne!(saved.state, state, "the provisional state itself is saved too");
        let mut fresh = Watcher::new(&ctx, cov, saved.state.clone(), "test".into(), vec![]);
        fresh.resume(saved);
        assert_eq!(fresh.state, state, "a restart starts from the state before the provisional spend");
        assert_eq!(fresh.coin.as_ref().map(|c| (c.0, c.1.amount, c.1.block_daa_score)), Some((prev.0, 2 * KAS, 1_000)));
        assert_eq!(fresh.coin.as_ref().map(|c| c.1.script_public_key.clone()), Some(ctx.vault_spk(&state)));
        assert_eq!(fresh.sinks.front(), Some(&low), "the scan starts where the provisional spend could be");
        assert!(fresh.closed.is_none() && fresh.unconfirmed.is_none());
        // in-run: the spend vanished and the coin is back: roll_back restores the pre-state
        let u = w.unconfirmed.clone().unwrap();
        w.roll_back(&u);
        assert!(w.unconfirmed.is_none() && w.state == state && w.coin.as_ref().map(|c| c.0) == Some(prev.0));
        let json = AlertFile::load(&ctx.dir);
        assert!(json.alerts[0].text.contains("dropped by the network") && json.alerts[0].text.contains("back where it was"), "{}", json.alerts[0].text);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// v5: the "send now" decision is a pure function of the flag, the dial, the coin's age and the delay.
    #[test]
    fn inherit_open_is_due_only_with_flag_dial_and_age() {
        assert_eq!(heir_clock(0, 3000), HeirClock::Quiet);
        assert_eq!(heir_clock(2699, 3000), HeirClock::Quiet, "just under 90%");
        assert_eq!(heir_clock(2700, 3000), HeirClock::Warn, "90% on the dot");
        assert_eq!(heir_clock(2999, 3000), HeirClock::Warn);
        assert_eq!(heir_clock(3000, 3000), HeirClock::Due);
        assert_eq!(heir_clock(i64::MAX, 4_000_000_000), HeirClock::Due, "no overflow at the far clock");
        assert!(open_due(true, true, 3000, 3000));
        assert!(!open_due(true, true, 2999, 3000), "one DAA early");
        assert!(!open_due(false, true, 9000, 3000), "no --auto-heir flag: the watcher only watches");
        assert!(!open_due(true, false, 9000, 3000), "vault made without the dial: the contract would refuse anyway");
    }

    /// `finish` is checked against the recorded destination, amount and the
    /// delay (virtual DAA), not reported from output 0.
    #[test]
    fn finish_is_checked_against_destination_amount_and_delay() {
        let ctx = test_ctx("finish");
        let cov = Hash::from_bytes([3u8; 32]);
        let dest = ScriptPublicKey::from_vec(0, vec![0xbb; 34]);
        let state = State { stage: 1, dest_hash: spk_hash(&dest), amount: KAS as i64, ..locked() };
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(3 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let cont = |v: u64| TransactionOutput::with_covenant(v, ctx.vault_spk(&after_finish(&state)), Some(CovenantBinding::new(0, cov)));
        let mut w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        w.daa_seen.0 = 1_000 + DELAY as u64;
        let good = spend_tx(&ctx, &state, "finish", vec![], &prev, vec![TransactionOutput::new(KAS, dest.clone()), cont(2 * KAS - 1000)]);
        let o = w.judge(&good, &prev);
        assert_eq!((o.level, o.kind, o.closed), (Level::Alert, Kind::WithdrawalCompleted, false));
        assert_eq!(o.next, Some(after_finish(&state)));
        assert!(o.bank.starts_with("1.0000 KAS left the account to"), "{}", o.bank);
        // one DAA too early for the delay
        w.daa_seen.0 -= 1;
        let o = w.judge(&good, &prev);
        assert_eq!(o.kind, Kind::LineageMismatch);
        assert!(o.text.contains("before the delay allows"), "{}", o.text);
        w.daa_seen.0 += 1;
        // another node a DAA behind: the lowest reading decides, so one node's clock cannot pass it
        w.daa_low = w.daa_seen.0 - 1;
        assert_eq!(w.judge(&good, &prev).kind, Kind::LineageMismatch);
        w.daa_low = u64::MAX;
        // wrong amount
        let short = spend_tx(&ctx, &state, "finish", vec![], &prev, vec![TransactionOutput::new(KAS - 1, dest.clone()), cont(2 * KAS - 999)]);
        let o = w.judge(&short, &prev);
        assert!(o.kind == Kind::LineageMismatch && o.text.contains("not the recorded 1.0000 KAS"), "{}", o.text);
        // wrong destination
        let elsewhere = spend_tx(&ctx, &state, "finish", vec![], &prev, vec![TransactionOutput::new(KAS, ScriptPublicKey::from_vec(0, vec![0xcc; 34])), cont(2 * KAS - 1000)]);
        let o = w.judge(&elsewhere, &prev);
        assert!(o.kind == Kind::LineageMismatch && o.text.contains("not the destination recorded"), "{}", o.text);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// W1: a thief's `request` with a 1-byte newDestSpk must become an alert, never a panic
    /// (the old watcher indexed `p[1][1]` and died before alerting).
    #[test]
    fn unreadable_request_alerts_instead_of_panicking() {
        let ctx = test_ctx("request");
        let dir = ctx.dir.clone();
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let art = ctx.compile(&state);
        let tag = art.contracts.values().next().unwrap().entries["request"].dispatch_tag.as_bytes().to_vec();
        let mut b = ScriptBuilder::new();
        for push in [vec![0u8; 65], vec![0x00], vec![0xe8, 0x03], tag, contract::bytecode(&art)] {
            b.add_data(&push).unwrap();
        }
        let prev_op = TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0);
        let prev: Coin = (prev_op, UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let inputs = vec![TransactionInput::new(prev_op, b.drain(), 0, 0)];
        let outputs = vec![TransactionOutput::with_covenant(2 * KAS - 1000, ctx.vault_spk(&state), Some(CovenantBinding::new(0, cov)))];
        let tx = Transaction::new(1, inputs, outputs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
        let rpc_tx: RpcTransaction = (&tx).into();
        let mut w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        let o = w.judge(&rpc_tx, &prev);
        assert_eq!((o.level, o.kind), (Level::Alert, Kind::WithdrawalPending));
        assert!(o.text.contains("unreadable request") && o.text.contains("1 bytes"), "{}", o.text);
        assert!(o.next.is_none() && !o.closed);
        // from the mempool the alert is marked unconfirmed and the pre-state is kept for a rollback
        w.apply(&rpc_tx, &prev, Seen::Mempool);
        let u = w.unconfirmed.as_ref().expect("unconfirmed sighting kept");
        assert_eq!((u.prev_state.clone(), u.level), (state, Level::Alert));
        let log = std::fs::read_to_string(dir.join("alerts.log")).unwrap();
        assert!(log.contains("ALERT WITHDRAWAL REQUESTED") && log.contains("(unconfirmed"), "{log}");
        let json = AlertFile::load(&dir);
        assert_eq!(json.alerts.len(), 1);
        assert_eq!((json.alerts[0].kind.as_str(), json.alerts[0].text.as_str()), ("withdrawal_pending", "A withdrawal was requested in a shape the guard cannot read. Press the emergency stop if you did not start this."));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A rule-abiding `spend` of `v0` sompi to `dest` from the Locked coin `prev` (fee 1000).
    fn spend_to(ctx: &Ctx, state: &State, cov: Hash, prev: &Coin, v0: u64, dest: ScriptPublicKey) -> RpcTransaction {
        let keep = prev.1.amount - v0 - 1000;
        let next = after_spend(state, WINDOW, prev.1.block_daa_score as i64, 0, (prev.1.amount - keep) as i64);
        let cont = TransactionOutput::with_covenant(keep, ctx.vault_spk(&next), Some(CovenantBinding::new(0, cov)));
        spend_tx(ctx, state, "spend", vec![vec![0u8; 65]], prev, vec![TransactionOutput::new(v0, dest), cont])
    }

    /// Every spend is an alert that names the amount and the address;
    /// an address this laptop never paid gets the loud kind, one from its own history (or the
    /// vault's cold address) the quiet one. No state.json: everything is loud.
    #[test]
    fn every_spend_alerts_louder_for_an_address_this_laptop_never_paid() {
        let ctx = test_ctx("spend-alert");
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        let stranger = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let to = addr_of(&ctx.network, &stranger);
        let tx = spend_to(&ctx, &state, cov, &prev, KAS / 2, stranger.clone());
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind, o.closed), (Level::Alert, Kind::SpendStranger, false), "{}", o.text);
        assert!(o.next.is_some(), "the lineage goes on after a spend");
        assert!(o.bank.contains("0.5000 KAS") && o.bank.contains(&to) && o.bank.contains("never paid"), "{}", o.bank);
        assert!(o.bank.contains("emergency stop"), "{}", o.bank);
        assert!(o.text.contains("SPENT 0.5000 KAS to") && o.text.contains(&to) && o.text.contains("never paid"), "{}", o.text);
        assert!(!o.bank.contains("audit"), "{}", o.bank);
        // the vault's own cold address is known without any history
        let tx = spend_to(&ctx, &state, cov, &prev, KAS / 2, ctx.cold_spk());
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind), (Level::Alert, Kind::Spend), "{}", o.text);
        assert!(o.bank.contains("paid before") && !o.text.contains("never paid"), "{} / {}", o.bank, o.text);
        // once this laptop's own history names the address, the same spend is the quiet kind
        let pinned = contract::sha256_hex(&std::fs::read(ctx.dir.join(contract::SOURCE_FILE)).unwrap());
        let v = Vault { covenant_id: cov, state: state.clone(), pending_to: None, history: vec![format!("spend 0.5 KAS to {to}, fee 0.00001 KAS, tx abc")], contract_sha256: Some(pinned), network: String::new(), dials: Dials::testnet() };
        ctx.try_save_vault(&v).unwrap();
        let tx = spend_to(&ctx, &state, cov, &prev, KAS / 2, stranger);
        let o = w.judge(&tx, &prev);
        assert_eq!((o.level, o.kind), (Level::Alert, Kind::Spend), "{}", o.text);
        assert!(o.bank.contains("0.5000 KAS") && o.bank.contains(&to) && o.bank.contains("paid before"), "{}", o.bank);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// The mempool sighting and the block sighting of one spend raise one alert
    /// (alerts.json gets one entry; the confirmation is an info line), while other kinds keep
    /// alerting again on confirmation as before.
    #[test]
    fn one_spend_seen_twice_raises_one_alert() {
        let ctx = test_ctx("spend-once");
        let dir = ctx.dir.clone();
        let cov = Hash::from_bytes([3u8; 32]);
        let state = locked();
        let prev: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(2 * KAS, ctx.vault_spk(&state), 1_000, false, Some(cov)));
        let mut w = Watcher::new(&ctx, cov, state.clone(), "test".into(), vec![]);
        let stranger = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let tx = spend_to(&ctx, &state, cov, &prev, KAS / 2, stranger);
        w.apply(&tx, &prev, Seen::Mempool);
        let u = w.unconfirmed.clone().expect("mempool sighting kept");
        assert_eq!((u.level, u.kind), (Level::Alert, Kind::SpendStranger));
        let json = AlertFile::load(&dir);
        assert_eq!(json.alerts.len(), 1, "the mempool sighting is the alert");
        assert_eq!(json.alerts[0].kind, "spend_stranger");
        assert!(json.alerts[0].text.starts_with("0.5000 KAS was sent to "), "{}", json.alerts[0].text);
        // the block holds the same tx: said, not alerted again
        w.confirmed(&u);
        assert_eq!(AlertFile::load(&dir).alerts.len(), 1, "no second alert for the same spend");
        let log = std::fs::read_to_string(dir.join("alerts.log")).unwrap();
        assert!(log.contains("ALERT SPENT 0.5000 KAS") && log.contains("info  CONFIRMED in a block: SPENT"), "{log}");
        // a withdrawal request still gets its confirmation alert (existing kinds unchanged)
        let req = Unconfirmed { kind: Kind::WithdrawalPending, bank: "A withdrawal was requested.".into(), text: "WITHDRAWAL REQUESTED".into(), ..u };
        w.confirmed(&req);
        let json = AlertFile::load(&dir);
        assert_eq!((json.alerts.len(), json.alerts[0].kind.as_str()), (2, "withdrawal_pending"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_pinned_node_is_said_out_loud() {
        assert!(no_node_warning(&[]).is_some_and(|w| w.contains("--node")));
        assert_eq!(no_node_warning(&["ws://127.0.0.1:17210".to_string()]), None);
    }

    fn mv(entry: &str, next: Option<State>, closed: bool, to: &str, moved: u64, slot: Option<i64>) -> Move {
        Move { entry: entry.into(), next, closed, to: to.into(), moved, fee: Some(1_000), txid: format!("{entry}tx"), slot, delay: 600, heir: "kaspatest:heir".into() }
    }

    fn rec(state: State, pending_to: Option<&str>) -> Vault {
        Vault { covenant_id: Hash::from_bytes([7u8; 32]), state, pending_to: pending_to.map(str::to_string), history: vec!["fund".into()], contract_sha256: None, network: String::new(), dials: Dials::testnet() }
    }

    #[test]
    fn follow_record_request_finish_decline_cancel_topup() {
        let lockd = locked();
        let pending = State { stage: 1, dest_hash: vec![9; 32], amount: (2 * KAS) as i64, ..locked() };
        // request: Pending, the destination recorded, the wallet's own words
        let v = follow_record(&rec(lockd.clone(), None), &mv("request", Some(pending.clone()), false, "kaspatest:dest", 2 * KAS, None));
        assert_eq!(v.state, pending);
        assert_eq!(v.pending_to.as_deref(), Some("kaspatest:dest"));
        assert_eq!(v.history.last().unwrap(), "request 2 KAS to kaspatest:dest, fee 0.00001 KAS, tx requesttx; finish allowed after 600 DAA");
        // the same move followed again (a restart) adds no second line
        let again = follow_record(&v, &mv("request", Some(pending.clone()), false, "kaspatest:dest", 2 * KAS, None));
        assert_eq!(again.history.len(), v.history.len());
        // finish with something left: Locked, no destination
        let f = follow_record(&v, &mv("finish", Some(after_finish(&pending)), false, "kaspatest:dest", 2 * KAS, None));
        assert_eq!((f.state.clone(), f.pending_to.clone()), (after_finish(&pending), None));
        assert!(f.history.last().unwrap().starts_with("finish 2 KAS to kaspatest:dest, fee"));
        assert!(!contract::is_stopped(&f));
        // finish that empties the vault: Locked, said so
        let f0 = follow_record(&v, &mv("finish", None, true, "kaspatest:dest", 2 * KAS, None));
        assert_eq!(f0.state, after_finish(&pending));
        assert!(f0.history.last().unwrap().ends_with("; the vault is now empty"));
        // decline by stop slot 1 (stop key 2): Locked again, the old destination named
        let d = follow_record(&v, &mv("decline", Some(after_decline(&pending)), false, "", 0, Some(1)));
        assert_eq!((d.state.clone(), d.pending_to.clone()), (after_decline(&pending), None));
        assert_eq!(d.history.last().unwrap(), "decline (stop key 2): the request for 2 KAS to kaspatest:dest is withdrawn, the vault is Locked again, fee 0.00001 KAS, tx declinetx");
        // cancel from Pending: the record is a stop, nothing waits any more
        let c = follow_record(&v, &mv("cancel", None, true, "kaspatest:cold", 2 * KAS, Some(2)));
        assert_eq!(c.history.last().unwrap(), "cancel (stop key 3): everything to cold, fee 0.00001 KAS, tx canceltx");
        assert_eq!(c.pending_to, None);
        assert!(contract::is_stopped(&c));
        // topup: state as it was (Pending keeps its destination)
        let t = follow_record(&v, &mv("topup", Some(pending.clone()), false, "", KAS / 2, None));
        assert_eq!((t.state.clone(), t.pending_to.as_deref()), (pending.clone(), Some("kaspatest:dest")));
        assert_eq!(t.history.last().unwrap(), "topup 0.5 KAS, tx topuptx");
        // inherit from Pending: a stop-kind record naming the pending payment and the heir
        let i = follow_record(&v, &mv("inherit", None, true, "kaspatest:dest", 2 * KAS, None));
        assert!(i.history.last().unwrap().starts_with("inherit from Pending: 2 KAS to kaspatest:dest (the pending request), the rest to heir kaspatest:heir"));
        assert!(contract::is_stopped(&i));
        // the record keeps everything else
        assert_eq!((i.covenant_id, i.history[0].as_str()), (v.covenant_id, "fund"));
    }
}
