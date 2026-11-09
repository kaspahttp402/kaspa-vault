//! `watch --auto-stop` (the public "drain this account" challenge): the watcher holds one stop key
//! and answers every waiting withdrawal by itself. Each answer is a `decline` (the account stays
//! open,
//! nothing leaves but the fee), until `cap` declines have gone out inside `window_secs`; then it
//! sends
//! `cancel` instead (everything to the cold vault), so a stream of requests cannot bleed the
//! account
//! dry through fees. The decision is pure (`choose`) and tested here; watch.rs does the sending.

use crate::contract::{Coin, State, Vault, after_decline, kas};
use crate::tx::Ctx;
use kaspa_consensus_core::tx::{Transaction, TransactionOutput};
use serde::{Deserialize, Serialize};
use silverscript_abi::ArtifactValue as V;
use std::path::Path;
use std::time::Duration;

/// Declines sent (unix seconds) and the cancel, kept beside state.json so a restart keeps the
/// count.
pub const LEDGER_FILE: &str = "auto-stop.json";
/// A stop this watcher sent is not sent again before this long, and only once it left the mempool.
pub const RETRY: Duration = Duration::from_secs(30);
/// A send that failed (no txid) is tried again after this long: the notice period is short.
pub const FAIL_RETRY: Duration = Duration::from_secs(2);
/// Pages of blocks the racer reads in one tick, from where it last saw the coin.
pub const RACE_PAGES: usize = 20;
/// Racer ticks without finding the coin before it takes the account's state from state.json again.
pub const RACE_LOST: u32 = 10;
/// The racer's own record of the account's state, so a restart does not start behind the chain.
pub const RACE_FILE: &str = "auto-stop-race.json";

#[derive(Serialize, Deserialize)]
struct RaceSave {
    cov: String,
    state: State,
}

/// The state the racer last followed for covenant `cov`, if it saved one.
pub fn load_race(dir: &Path, cov: &str) -> Option<State> {
    let text = std::fs::read_to_string(dir.join(RACE_FILE)).ok()?;
    let s: RaceSave = serde_json::from_str(&text).ok()?;
    (s.cov == cov).then_some(s.state)
}

pub fn save_race(dir: &Path, cov: &str, state: &State) -> Result<(), String> {
    let save = RaceSave { cov: cov.to_string(), state: state.clone() };
    let text = serde_json::to_string(&save).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(RACE_FILE), text)
        .map_err(|e| format!("could not write {RACE_FILE}: {e}"))
}

/// Why a stop did not go out: it could not be built (fee ceiling, floor: a cancel may still work),
/// or a node refused it (the coin may already be spent: never a reason to cancel).
#[derive(Debug)]
pub enum SendErr {
    Build(String),
    Submit(String),
}

impl std::fmt::Display for SendErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendErr::Build(e) | SendErr::Submit(e) => f.write_str(e),
        }
    }
}
/// Sends one waiting coin may get (first try and resends) before the watcher only shouts.
pub const TRIES_PER_COIN: u32 = 6;
/// Times kept in the ledger at most (a cap of this size or more is refused at start).
pub const LEDGER_MAX: usize = 1000;

/// The `--auto-stop` settings: stop key `which` (1, 2 or 3); with a `cap`, at most that many
/// declines per `window_secs` before it cancels instead (None: it only ever declines).
#[derive(Clone, Debug)]
pub struct AutoStop {
    pub which: u8,
    pub cap: Option<u32>,
    pub window_secs: u64,
    /// Block explorer base url for the proof file's links (`explorer`).
    pub explorer: String,
    /// The challenge's length: this long after the first start, everything goes to the cold vault.
    pub end_after_secs: Option<u64>,
}

impl AutoStop {
    /// Refuses settings that cannot work: no declines at all is fine (cap 0 = always cancel), but a
    /// window of zero or a cap past the ledger size is a mistake.
    pub fn check(&self) -> Result<(), String> {
        crate::keys::stop_slot(self.which)?;
        if self.window_secs == 0 {
            return Err("--stop-window-hours must be more than 0".into());
        }
        if self.cap.is_some_and(|c| c as usize >= LEDGER_MAX) {
            return Err(format!("--decline-cap must be under {LEDGER_MAX}"));
        }
        Ok(())
    }
}

/// What the watcher sends for a waiting withdrawal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Decline,
    Cancel,
}

impl Choice {
    pub fn entry(self) -> &'static str {
        match self {
            Choice::Decline => "decline",
            Choice::Cancel => "cancel",
        }
    }
}

/// Declines sent inside the window decide: under the cap, decline; at or over it, cancel. No cap:
/// always decline (the challenge stays up until the owner ends it).
pub fn choose(recent: usize, cap: Option<u32>) -> Choice {
    match cap {
        Some(c) if recent >= c as usize => Choice::Cancel,
        _ => Choice::Decline,
    }
}

#[derive(Default, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Ledger {
    /// Unix seconds of every decline this watcher sent (newest last, pruned to the window).
    pub declines: Vec<u64>,
    /// The cancel's txid once one went out.
    pub cancel: Option<String>,
    /// Unix seconds when the challenge ends (set once, at the first start with an end).
    pub ends_at: Option<u64>,
}

impl Ledger {
    pub fn load(dir: &Path) -> Ledger {
        let text = std::fs::read_to_string(dir.join(LEDGER_FILE)).ok();
        text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(LEDGER_FILE), text)
            .map_err(|e| format!("could not write {LEDGER_FILE}: {e}"))
    }

    /// The challenge's end: kept from the first start, so a restart never moves it; set now when
    /// it has none and `after` asks for one.
    pub fn fix_end(&mut self, now: u64, after: Option<u64>) -> Option<u64> {
        if self.ends_at.is_none() {
            self.ends_at = after.map(|a| now.saturating_add(a));
        }
        self.ends_at
    }

    /// Declines inside the last `window` seconds before `now`.
    pub fn recent(&self, now: u64, window: u64) -> usize {
        self.declines.iter().filter(|&&t| t + window > now).count()
    }

    /// Notes a decline at `now`, dropping times older than the window and keeping at most
    /// LEDGER_MAX.
    pub fn note_decline(&mut self, now: u64, window: u64) {
        self.declines.retain(|&t| t + window > now);
        self.declines.push(now);
        let extra = self.declines.len().saturating_sub(LEDGER_MAX);
        self.declines.drain(..extra);
    }
}

/// Builds and sends `choice` for the waiting coin `coin` of vault `v` (whose state is the watcher's
/// Pending state), signed with stop key `which`. Returns (txid, fee in sompi). The fee comes out of
/// the account, never over its max fee dial; what stays (decline) or leaves (cancel) is never under
/// the min remainder dial.
pub async fn send(
    ctx: &Ctx,
    v: &Vault,
    coin: &Coin,
    choice: Choice,
    which: u8,
) -> Result<(String, u64), SendErr> {
    let (tx, fee) = build(ctx, v, coin, choice, which).map_err(SendErr::Build)?;
    let id = ctx.try_submit(&tx, choice.entry()).await.map_err(SendErr::Submit)?;
    Ok((id, fee))
}

/// The signed transaction `send` submits, and its fee: a decline keeps the account in the
/// covenant, a cancel pays everything left to the safe (`ctx.cold_spk()`, an outside address
/// after `set-cold-to`) as the one output.
pub fn build(ctx: &Ctx, v: &Vault, coin: &Coin, choice: Choice, which: u8) -> Result<(Transaction, u64), String> {
    let slot = crate::keys::stop_slot(which)?;
    let key = ctx.keys.try_stop_key(slot)?;
    let next = after_decline(&v.state);
    let cold = ctx.cold_spk();
    let floor = v.dials.min_remainder as u64;
    ctx
        .try_with_fee(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
            let left = coin.1.amount.checked_sub(fee).ok_or("the account cannot pay the fee")?;
            if left < floor {
                let (what, left, floor) = (choice.entry(), kas(left), kas(floor));
                let words = format!("{what} would leave {left:.5} KAS, under the {floor} KAS kept");
                return Err(words);
            }
            let out = match choice {
                Choice::Decline => ctx.continuation(left, &next, v),
                Choice::Cancel => TransactionOutput::new(left, cold.clone()),
            };
            let which = [V::Int(slot as i64)];
            ctx.vault_tx(v, coin, choice.entry(), Some(&key), &which, vec![out], 0, 0)
        })
        .map_err(|e| e.to_string())
}

/// The public record of the challenge, beside state.json: one line per attack and its answer, each
/// with a block explorer link, so anyone can check the rounds on chain. Appended, never rewritten.
pub const PROOF_FILE: &str = "challenge-proof.md";

/// The block explorer for `network` ("mainnet" or "testnet"), unless `--explorer` names another.
pub fn explorer(network: &str, custom: Option<&str>) -> String {
    let base = match (custom, network) {
        (Some(c), _) => c,
        (None, crate::network::MAINNET) => "https://explorer.kaspa.org",
        (None, _) => "https://explorer-tn10.kaspa.org",
    };
    base.trim_end_matches('/').to_string()
}

pub fn tx_link(base: &str, txid: &str) -> String {
    format!("{base}/txs/{txid}")
}

pub fn address_link(base: &str, address: &str) -> String {
    format!("{base}/addresses/{address}")
}

/// Whether the challenge is over at `now` (its end is set and has passed).
pub fn over(ends_at: Option<u64>, now: u64) -> bool {
    ends_at.is_some_and(|t| now >= t)
}

/// The end of the challenge in plain words, for the proof file.
pub fn end_line(when: &str, answer: &str, fee: u64, base: &str) -> String {
    let link = tx_link(base, answer);
    let fee = kas(fee);
    let what = "the challenge ended: everything went to the owner's vault";
    format!("- {when}: {what}: {link} (fee {fee:.8} KAS)")
}

/// One round in plain words: the attack (the request tx that made the waiting coin) and the answer.
pub fn proof_line(
    when: &str,
    attack: &str,
    asked: &str,
    choice: Choice,
    answer: &str,
    fee: u64,
    base: &str,
) -> String {
    let what = match choice {
        Choice::Decline => "declined automatically; the account stays open",
        Choice::Cancel => "stopped automatically; everything went to the cold vault",
    };
    format!(
        "- {when}: attack: withdrawal request of {asked} ({}) -> {what}: {} (fee {:.8} KAS from the account)",
        tx_link(base, attack),
        tx_link(base, answer),
        kas(fee)
    )
}

/// Appends `line` to the proof file, with a heading the first time.
pub fn append_proof(dir: &Path, line: &str) -> Result<(), String> {
    use std::io::Write;
    let path = dir.join(PROOF_FILE);
    let fresh = !path.exists();
    let open = std::fs::OpenOptions::new().create(true).append(true).open(&path);
    let mut f = open.map_err(|e| format!("could not open {PROOF_FILE}: {e}"))?;
    let head = if fresh { "# Drain challenge: every attack and its answer, on chain

" } else { "" };
    write!(f, "{head}{line}
").map_err(|e| format!("could not write {PROOF_FILE}: {e}"))
}

/// The same rounds, one JSON object per line, for the public challenge page
/// (tools/challenge_page.py).
pub const ROUNDS_FILE: &str = "challenge-rounds.jsonl";

/// One round as the challenge page reads it: the attack (request tx), the answer, sompi amounts.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Round {
    /// Unix seconds when the answer went out.
    pub at: u64,
    pub attack: String,
    pub asked: u64,
    /// "decline" or "cancel".
    pub answer_kind: String,
    pub answer: String,
    pub fee: u64,
}

/// Appends one round to the rounds file.
pub fn append_round(dir: &Path, r: &Round) -> Result<(), String> {
    use std::io::Write;
    let line = serde_json::to_string(r).map_err(|e| e.to_string())?;
    let open = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(ROUNDS_FILE));
    let mut f = open.map_err(|e| format!("could not open {ROUNDS_FILE}: {e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("could not write {ROUNDS_FILE}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_auto_cancel_pays_one_output_to_the_outside_safe() {
        use crate::contract::{Dials, locked};
        use crate::keys::{Keys, keypair, xonly};
        use crate::network::Network;
        use kaspa_consensus_core::Hash;
        use kaspa_consensus_core::tx::{TransactionOutpoint, UtxoEntry};
        let net = Network::testnet();
        let mut ctx = Ctx { dir: std::env::temp_dir(), keys: Keys::fresh(&net), rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 };
        let outside = net.p2pk(&xonly(&keypair(&[42u8; 32]))).to_string();
        ctx.keys.set_cold_external(&outside).unwrap();
        assert!(ctx.keys.public().cold_external);
        let want = net.try_to_spk(&outside).unwrap();
        let v = Vault { covenant_id: Hash::from_bytes([1u8; 32]), state: locked(), pending_to: None, history: vec![], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        let entry = UtxoEntry::new(50 * crate::contract::KAS, ctx.vault_spk(&v.state), 1_000, false, Some(v.covenant_id));
        let coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), entry);
        let (tx, fee) = build(&ctx, &v, &coin, Choice::Cancel, 1).unwrap();
        assert_eq!(tx.outputs.len(), 1, "one output");
        assert_eq!(tx.outputs[0].script_public_key, want, "paid to the outside safe");
        assert_eq!(tx.outputs[0].value + fee, coin.1.amount, "everything but the fee");
    }

    #[test]
    fn declines_until_the_cap_then_cancels() {
        assert_eq!(choose(0, Some(20)), Choice::Decline);
        assert_eq!(choose(19, Some(20)), Choice::Decline);
        assert_eq!(choose(20, Some(20)), Choice::Cancel);
        assert_eq!(choose(25, Some(20)), Choice::Cancel);
        // cap 0: every request is answered with cancel
        assert_eq!(choose(0, Some(0)), Choice::Cancel);
        // no cap: decline forever
        assert_eq!(choose(100_000, None), Choice::Decline);
    }

    #[test]
    fn the_window_forgets_old_declines() {
        let day = 86_400;
        let mut l = Ledger::default();
        for i in 0..20 {
            l.note_decline(1_000 + i, day);
        }
        assert_eq!(choose(l.recent(1_100, day), Some(20)), Choice::Cancel, "20 in the day: cancel");
        // a day after the first ones, they no longer count
        assert_eq!(l.recent(1_000 + day + 10, day), 9);
        assert_eq!(choose(l.recent(1_000 + day + 10, day), Some(20)), Choice::Decline);
        l.note_decline(1_000 + 2 * day, day);
        let pruned = "old times are pruned when a new one is noted";
        assert_eq!(l.declines, vec![1_000 + 2 * day], "{pruned}");
    }

    #[test]
    fn the_ledger_is_bounded_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("kv-autostop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut l = Ledger::default();
        for i in 0..(LEDGER_MAX as u64 + 50) {
            l.note_decline(i, u64::MAX / 2);
        }
        assert_eq!(l.declines.len(), LEDGER_MAX);
        l.cancel = Some("abc".into());
        l.save(&dir).unwrap();
        let back = Ledger::load(&dir);
        assert_eq!(back.declines.len(), LEDGER_MAX);
        assert_eq!(back.cancel.as_deref(), Some("abc"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn proof_lines_link_both_transactions() {
        let base = explorer("mainnet", None);
        assert_eq!(base, "https://explorer.kaspa.org");
        assert_eq!(explorer("testnet", Some("https://x.example/")), "https://x.example");
        let when = "2026-01-02 03:00 UTC";
        let line = proof_line(when, "aa", "5 KAS", Choice::Decline, "bb", 2_000, &base);
        assert!(line.contains("https://explorer.kaspa.org/txs/aa"));
        assert!(line.contains("https://explorer.kaspa.org/txs/bb"));
        assert!(line.contains("declined automatically") && line.contains("0.00002000 KAS"));
    }

    #[test]
    fn the_end_is_fixed_at_the_first_start() {
        let week = 7 * 86_400;
        let mut l = Ledger::default();
        assert_eq!(l.fix_end(1_000, None), None, "no end asked: none");
        assert_eq!(l.fix_end(1_000, Some(week)), Some(1_000 + week));
        // a restart later, even asking for a new week, keeps the first end
        assert_eq!(l.fix_end(50_000, Some(week)), Some(1_000 + week));
        assert!(!over(l.ends_at, 1_000 + week - 1));
        assert!(over(l.ends_at, 1_000 + week));
        assert!(!over(None, u64::MAX));
        // a ledger written before ends_at existed still loads
        let old: Ledger = serde_json::from_str(r#"{"declines":[5],"cancel":null}"#).unwrap();
        assert_eq!((old.declines, old.ends_at), (vec![5], None));
    }

    #[test]
    fn settings_are_checked() {
        let explorer = String::new();
        let (cap, window_secs) = (Some(20), 86_400);
        let ok = AutoStop { which: 1, cap, window_secs, explorer, end_after_secs: None };
        assert!(ok.check().is_ok());
        assert!(AutoStop { which: 4, ..ok.clone() }.check().is_err());
        assert!(AutoStop { window_secs: 0, ..ok.clone() }.check().is_err());
        assert!(AutoStop { cap: Some(LEDGER_MAX as u32), ..ok.clone() }.check().is_err());
        assert!(AutoStop { cap: None, ..ok }.check().is_ok());
    }
}
