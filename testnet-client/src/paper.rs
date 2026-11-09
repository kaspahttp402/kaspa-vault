//! The paper stop button (docs/PAPER_STOP.md). Stop slot 2 (or 3 when a guardian holds 2)
//! holds a key the owner keeps as 24 words on paper, so paper alone can stop a lost phone's
//! account: the stop pays only the owner's own vault, where two vault seeds take over.
//! `paper.make` makes the stop's words and keeps only its public key pending, `paper.confirm`
//! (the words typed back) leaves it waiting like a guardian (the next opening applies it),
//! `paper.sheet` answers the watch code printed beside the words, `paper.check` finds the
//! account's coin from that code, and `paper.stop` signs the stop with the typed words, also on
//! a phone with no key set. The secret lives only in memory (`Zeroizing`, then `Armed`) for one
//! call: never written, never logged, and no answer but `paper.make`'s carries a word.

use crate::contract::{Vault, is_stopped, kas};
use crate::core::{Core, GUARD_KIND, NOT_GUARD_FILE, UNHELD_FILE, cold_slot, plain, refusal,
    set_cold};
use crate::keys::{Keys, Public, p2sh_address, sheet_from_words, sheet_words, try_xonly_hex};
use crate::indexer::Indexer;
use crate::lineage::{self, Limits, WithNode};
use crate::network::Network;
use crate::tx::Ctx;
use crate::watched::{Armed, watch_ctx};
use kaspa_addresses::Address;
use kaspa_consensus_core::tx::TransactionOutpoint;
use secp256k1::Secp256k1;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub const PAPER_WRONG: &str = "Those words are not a paper stop for this account.";
pub const BOTH_GUARDIANS: &str = "Remove a guardian first, then make a paper stop.";
pub const SLOT_UNSURE: &str = "This phone cannot tell whether stop slot 2 or 3 holds a guardian \
    added before it kept its list. Choose the slot for the paper stop.";
pub const GUARDIAN_SLOT: &str = "A guardian holds that stop slot. Choose the other slot.";
pub const PAPER_ELSEWHERE: &str = "Your paper stop is in the other slot; a new one replaces it \
    there.";
const BAD_SLOT: &str = "Choose stop slot 2 or 3.";
pub const NOT_PENDING: &str = "No new paper stop is waiting to be checked. Make a new paper stop.";
pub const WORDS_DIFFER: &str = "Those words are not the paper stop just made. Check each word \
    against your paper.";
pub const ALREADY_STOPPED: &str = "This account is already stopped.";
pub const WAIT: &str = "Please wait a moment and try again.";
/// paper.takeout's refusals (docs/PAPER_STOP.md "AGREED NEXT").
pub const SEEDS_WRONG: &str = "These vault seeds do not open this account's vault.";
pub const SEEDS_SHAPE: &str = "Type two vault seeds of 24 words each.";
pub const VAULT_EMPTY: &str = "The vault holds no funds yet.";
pub const CODE_VAULT_DIFFERS: &str = "This account's code does not match its vault. Get the code \
     again from a phone that watches the account.";
pub const TO_NEEDED: &str = "Enter the address the funds should go to.";
pub const TO_WRONG: &str = "That address is not right for this network. Check it and try again.";
/// airgap.rs's words for a vault with no coin to spend.
const NO_COLD_COINS: &str = "no mature coins";
const MAX_SEEDS_TEXT: usize = 2 * MAX_WORDS_TEXT;
const MAX_ADDRESS: usize = 200;
/// paper.make's waiting stop until paper.confirm: {x, slot}, the PUBLIC key only.
const PENDING_FILE: &str = "paper-pending.json";
/// Keyless calls (paper.check, paper.stop): at most one per gap per process.
const KEYLESS_GAP: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(2) };
static LAST_KEYLESS: Mutex<Option<Instant>> = Mutex::new(None);
/// Our own scratch folders older than this are left over (a call never takes so long).
const STALE_SCRATCH: Duration = Duration::from_secs(600);
const MAX_SWEEP: usize = 4096;
/// After a stop is sent: looks at the node for the stopped coin, `GONE_TRIES` x `GONE_WAIT`.
const GONE_TRIES: u32 = 5;
const GONE_WAIT: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(2) };
pub const MOVED: &str = "The account has moved since this paper sheet was printed, and this phone \
    cannot follow it yet. Use a newer paper sheet, or ask a guardian for the account's watch code.";
/// The walk ran out of its time or questions (lineage::TOO_LONG); a later try may get through.
const TOOK_TOO_LONG: &str = "Checking the account's history took too long. Try again in a few \
    minutes. No funds were moved.";
/// The start of the node's own plain refusal for more than one coin (tx.rs try_vault_coin).
const MANY_COINS: &str = "This account should hold one covenant coin, but the network shows";
const NOT_MADE: &str = "The paper stop could not be made. Nothing was changed.";
const UNWRITABLE: &str = "The account's folder on this device could not be written.";
/// The most typed text read as words (24 words of at most 8 letters, generous spacing) and the
/// longest watch code read; anything longer is refused before it is parsed.
const MAX_WORDS_TEXT: usize = 1_000;
const MAX_WATCH_CODE: usize = 64 * 1024;
/// Scratch folders under the wallet's folder: `paper-check-<process>-<n>`, removed on every path.
const SCRATCH_PREFIX: &str = "paper-check";
static SCRATCH_N: AtomicU64 = AtomicU64::new(0);

/// `unheld.json` as an object (no file, or an unreadable one = {}); `record_unheld` keeps its
/// `paper` list.
pub(crate) fn unheld_object(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join(UNHELD_FILE)).unwrap_or_default();
    serde_json::from_str::<Value>(&text).ok().filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// The paper stops this phone made (x-only hex): unheld.json's `paper` list.
pub(crate) fn paper_keys(dir: &Path) -> Vec<String> {
    let file = unheld_object(dir);
    let list = file["paper"].as_array().cloned().unwrap_or_default();
    list.into_iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
}

/// Adds `x` to the `paper` list (sorted, never shrinks), keeping the rest of unheld.json as it is.
fn record_paper(dir: &Path, x: &str) -> Result<(), String> {
    let mut list = paper_keys(dir);
    list.push(x.to_string());
    list.sort();
    list.dedup();
    let mut file = unheld_object(dir);
    file["paper"] = json!(list);
    write_atomic(&dir.join(UNHELD_FILE), &file.to_string())
}

/// Writes `text` to `path` whole or not at all: a sibling `.tmp` file, then a rename over `path`.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|_| UNWRITABLE.to_string())
}

/// `stop_holders` with each slot whose key is one of this phone's paper stops marked `paper`.
pub(crate) fn mark_paper(mut holders: Value, p: &Public, dir: &Path) -> Value {
    let paper = paper_keys(dir);
    for (i, x) in [(1usize, &p.cancel2_x), (2, &p.cancel3_x)] {
        let is_paper = !x.is_empty() && *x != p.cancel_x && paper.contains(x);
        if is_paper && holders.get(i).is_some_and(Value::is_object) {
            holders[i]["holder"] = json!("paper");
        }
    }
    holders
}

/// The stop slot a new paper stop takes. A slot holding a paper stop is taken again (the new words
/// replace the old, so two sheets never both work); else a clearly free slot (`you`, `unheld`), 2
/// before 3. `unknown` (a key set older than the unheld list) may be a guardian, so it ranks like
/// one: with no clearly free slot the owner chooses (`asked`), and a guardian's is never taken.
fn pick_slot(holders: &Value, asked: Option<u8>) -> Result<u8, &'static str> {
    let holder = |s: u8| {
        let h = &holders[usize::from(s).saturating_sub(1)]["holder"];
        h.as_str().unwrap_or("").to_string()
    };
    let rank = |s: u8| match holder(s).as_str() { "paper" => 0, "you" | "unheld" => 1, _ => 9 };
    if holder(2) == "guardian" && holder(3) == "guardian" {
        return Err(BOTH_GUARDIANS);
    }
    let paper = [2u8, 3].into_iter().find(|&s| rank(s) == 0);
    match (asked, paper) {
        (Some(s), Some(p)) if s != p => Err(PAPER_ELSEWHERE),
        (Some(s), _) if holder(s) == "guardian" => Err(GUARDIAN_SLOT),
        (Some(s), _) => Ok(s),
        (None, _) if rank(2).min(rank(3)) == 9 => Err(SLOT_UNSURE),
        (None, _) => Ok(if rank(3) < rank(2) { 3 } else { 2 }),
    }
}

/// The `slot` argument: absent or null = the engine chooses, else exactly 2 or 3.
fn slot_arg(args: &Value) -> Result<Option<u8>, &'static str> {
    match (&args["slot"], args["slot"].as_u64()) {
        (Value::Null, _) => Ok(None),
        (_, Some(2)) => Ok(Some(2)),
        (_, Some(3)) => Ok(Some(3)),
        _ => Err(BAD_SLOT),
    }
}

/// This account's rule block as it will be once the waiting guardian/heir changes apply.
fn after_waiting(core: &Core) -> Result<Public, String> {
    let mut p = core.ctx.keys.public();
    let waiting = crate::core::read_people_next(&core.ctx.dir)?.unwrap_or_default();
    let field = |name: &str| waiting.get(name).and_then(Value::as_str).map(str::to_string);
    if let Some(x) = field("cancel2_x") {
        p.cancel2_x = x;
    }
    if let Some(x) = field("cancel3_x") {
        p.cancel3_x = x;
    }
    Ok(p)
}

/// A fresh stop secret and its x-only key (hex); the library's own copy is erased before it drops.
fn fresh_stop() -> (Zeroizing<[u8; 32]>, String) {
    let (mut sk, _) = Secp256k1::new().generate_keypair(&mut secp256k1::rand::thread_rng());
    let secret = Zeroizing::new(sk.secret_bytes());
    sk.non_secure_erase();
    let x = try_xonly_hex(&secret).unwrap_or_default();
    (secret, x)
}

/// The slot a paper stop takes now (`asked`: the owner's choice), as the owner's block will be
/// once the waiting changes apply.
fn slot_now(core: &Core, asked: Option<u8>) -> Result<u8, String> {
    let dir = &core.ctx.dir;
    let after = after_waiting(core)?;
    let holders = crate::core::stop_holders(&after, &crate::core::read_unheld(dir));
    pick_slot(&mark_paper(holders, &after, dir), asked).map_err(str::to_string)
}

/// `paper.make {account?, slot?}` (the Java side asks a fresh fingerprint first): new paper stop
/// words for slot 2, or 3 when a guardian holds 2 (`slot` 2|3 when the phone cannot tell). Nothing
/// changes yet: only the new PUBLIC key and its slot wait in paper-pending.json (a second make
/// replaces them) until paper.confirm proves the words were written down. Answers {ok, words,
/// slot, error}; `words` (24) is the one place the secret ever leaves, and it is never stored.
pub async fn make(core: &Core, args: &Value) -> Value {
    let fail = |e: &str| json!({"ok": false, "words": [], "slot": null, "error": plain(e)});
    let slot = match slot_arg(args).map_err(str::to_string).and_then(|a| slot_now(core, a)) {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let (secret, x) = fresh_stop();
    if x.is_empty() {
        return fail(NOT_MADE);
    }
    let pending = json!({"x": x, "slot": slot}).to_string();
    if let Err(e) = write_atomic(&core.ctx.dir.join(PENDING_FILE), &pending) {
        return fail(&e);
    }
    json!({"ok": true, "words": sheet_words(&secret), "slot": slot, "error": null})
}

/// The paper stop paper.make left waiting: (x-only key, slot).
fn read_pending(dir: &Path) -> Option<(String, u8)> {
    let text = std::fs::read_to_string(dir.join(PENDING_FILE)).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let slot = match v["slot"].as_u64() {
        Some(2) => 2,
        Some(3) => 3,
        _ => return None,
    };
    Some((v["x"].as_str()?.to_string(), slot))
}

/// `paper.confirm {account?, words}` (fresh fingerprint: it changes who can stop the account): the
/// words typed back from the paper must give the key paper.make left waiting. Then the stop waits
/// key-set wide exactly as a guardian's (rules.change; the next opening applies it) and the
/// pending file goes. Answers rules.change's answer, its rule words moved to `rule_words` (no
/// `words`), with `slot`; no word of the paper stop is ever in it.
pub async fn confirm(core: &Core, args: &Value) -> Value {
    let fail = |e: &str| json!({"ok": false, "rule_words": [], "slot": null, "error": plain(e)});
    let dir = &core.ctx.dir;
    let Some((want, slot)) = read_pending(dir) else { return fail(NOT_PENDING) };
    let typed = secret_of(&args["words"]).ok().and_then(|s| try_xonly_hex(&s));
    if !typed.is_some_and(|x| x.eq_ignore_ascii_case(&want)) {
        return fail(WORDS_DIFFER);
    }
    if let Err(e) = slot_now(core, Some(slot)) {
        return fail(&e);
    }
    // Listed first: a listed key that never lands in a slot is harmless, an unlisted paper stop
    // would show as a guardian.
    if let Err(e) = record_paper(dir, &want) {
        return fail(&e);
    }
    let change = json!({"guardian_x": want, "guardian_which": slot});
    let mut answer = core.rules_change_paper(&change).await;
    if answer["ok"] != true {
        return fail(answer["error"].as_str().unwrap_or(NOT_MADE));
    }
    let _ = std::fs::remove_file(dir.join(PENDING_FILE));
    answer["rule_words"] = answer["words"].take();
    if let Some(o) = answer.as_object_mut() {
        o.remove("words");
    }
    answer["slot"] = json!(slot);
    answer
}

/// `paper.sheet {account?, nodes?, name?}` (offline): the watch code printed beside the words,
/// exactly guard.export's answer, as text (the page prints it as a QR). It holds no secret.
pub async fn sheet(core: &Core, args: &Value) -> Value {
    let (_, out) = core.guard_call("guard.export", args).await;
    if out["kind"] == GUARD_KIND {
        return json!({"ok": true, "watch_code": out.to_string(), "error": null});
    }
    let e = out["error"].as_str().unwrap_or(NOT_GUARD_FILE);
    json!({"ok": false, "watch_code": null, "error": plain(e)})
}

/// `paper.check` / `paper.stop`, answered before Core::call's key gate: a new phone with no key set
/// (after a loss) uses them. Safe without keys: they work only in a scratch folder made from the
/// watch code, never read or write this phone's own key set, and their one secret is the typed
/// words, held for one signing.
pub async fn call_keyless(core: &Core, method: &str, args: &Value) -> (u16, Value) {
    sweep_scratch(&core.ctx.dir);
    if too_soon(&LAST_KEYLESS, Instant::now(), KEYLESS_GAP) {
        let wait = match method {
            "paper.stop" => fail_stop(WAIT),
            "paper.takeout" => fail_takeout(WAIT),
            _ => fail_check(WAIT),
        };
        return (200, wait);
    }
    match method {
        "paper.check" => check(core, args).await,
        "paper.stop" => stop(core, args).await,
        "paper.takeout" => takeout(core, args).await,
        _ => (404, refusal("Not found")),
    }
}

/// Whether a keyless call comes within `gap` of the last one let through (`last`); one let
/// through is remembered.
fn too_soon(last: &Mutex<Option<Instant>>, now: Instant, gap: Duration) -> bool {
    let mut seen = last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.is_some_and(|t| now.saturating_duration_since(t) < gap) {
        return true;
    }
    *seen = Some(now);
    false
}

/// Removes left-over scratch folders under `dir`: another process's (a crash or a kill skipped
/// their drop), and this process's own once older than `STALE_SCRATCH`.
fn sweep_scratch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let any = format!("{SCRATCH_PREFIX}-");
    let ours = format!("{SCRATCH_PREFIX}-{}-", std::process::id());
    for e in entries.flatten().take(MAX_SWEEP) {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&any) {
            continue;
        }
        let age = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
        if !name.starts_with(&ours) || age.is_some_and(|a| a > STALE_SCRATCH) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A scratch folder holding a watched copy of the account a watch code names; removed on drop.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The watch code read into a fresh scratch folder under `dir` with guard.import's checks (those of
/// watch.import: the code matches its rule block, `Public::check_foreign`), and its public keys.
fn read_code(dir: &Path, network: &Network, code: &Value) -> Result<(Scratch, Keys), String> {
    let code = code.as_str().filter(|c| c.len() <= MAX_WATCH_CODE).ok_or(NOT_GUARD_FILE)?;
    let n = SCRATCH_N.fetch_add(1, Ordering::Relaxed);
    let scratch = Scratch(dir.join(format!("{SCRATCH_PREFIX}-{}-{n}", std::process::id())));
    let _ = std::fs::remove_dir_all(&scratch.0);
    crate::core::guard_import(&scratch.0, network, code)?;
    let keys = Keys::load_public(&scratch.0.join("keys.json")).map_err(|_| NOT_GUARD_FILE)?;
    Ok((scratch, keys))
}

/// The stop secret typed words hold (`words`: 24 strings, or one text of 24 words), refused in one
/// plain sentence whatever is wrong, never echoing a word. At most 25 words are looked at.
fn secret_of(words: &Value) -> Result<Zeroizing<[u8; 32]>, String> {
    // Limit: `words` is borrowed from the caller's args (Core::call takes &Value), so the typed
    // text cannot be wiped here; only borrowed &str views are made, and the secret is Zeroizing.
    let list: Vec<&str> = match words {
        Value::String(t) => t.split_whitespace().take(25).collect(),
        Value::Array(a) => a.iter().take(25).map(|w| w.as_str().unwrap_or("")).collect(),
        _ => Vec::new(),
    };
    let size: usize = list.iter().map(|w| w.len()).sum();
    if list.len() != 24 || size > MAX_WORDS_TEXT {
        return Err(PAPER_WRONG.into());
    }
    sheet_from_words(&list).map(Zeroizing::new).map_err(|_| PAPER_WRONG.to_string())
}

/// The stop slot (2 or 3) the secret's key fills in this rule block; a slot repeating stop 1
/// never matches.
fn slot_of(p: &Public, secret: &[u8; 32]) -> Option<u8> {
    let x = try_xonly_hex(secret)?;
    if x.eq_ignore_ascii_case(&p.cancel_x) {
        return None;
    }
    let fills = |slot: &str| !slot.is_empty() && slot.eq_ignore_ascii_case(&x);
    if fills(&p.cancel2_x) {
        return Some(2);
    }
    fills(&p.cancel3_x).then_some(3)
}

/// The account's public keys with the paper stop in its slot, for one signing (Armed wipes them).
fn armed_keys(mut keys: Keys, which: u8, secret: &[u8; 32]) -> Keys {
    if which == 2 {
        keys.cancel2 = Some(*secret);
    } else {
        keys.cancel3 = Some(*secret);
    }
    keys
}

/// The account's coin now, as found from the scratch record. `live` false = the account is
/// already stopped or ended (no coin); `coin` = where the coin sat and its outpoint, for the
/// after-stop look.
struct Live {
    kas: f64,
    hops: u32,
    live: bool,
    stage: &'static str,
    coin: Option<(Address, TransactionOutpoint)>,
}

/// A record that already ended (watched.rs's words): `ended` (an heir's claim or an emptied
/// vault); None for an account still open. A record ending in a stop this phone sent is None too:
/// the node accepting a stop is not the stop landing, so `find_live` checks it on the node.
fn ended_stage(v: &Vault) -> Option<&'static str> {
    let last = v.history.last().map(String::as_str).unwrap_or("");
    if last.starts_with("cancel") {
        None
    } else if is_stopped(v) || last.ends_with(crate::watch::EMPTIED) {
        Some("ended")
    } else {
        None
    }
}

/// THE one place the live coin is found (paper.check and paper.stop both come here): the printed
/// coin while it is live; else the stop seen on the node; else `follow` walks the history.
/// A record that already ended answers offline; one ending in a sent stop needs the node (offline
/// it is the 503 "could not check", never `stopped`): its coin still live = live (the stop can be
/// sent again); gone = `stopped` only when the vault holds that very stop's coin, else `follow`.
/// Errors carry the status to answer (503 = no network).
async fn find_live(core: &Core, ctx: &Ctx) -> Result<Live, (u16, String)> {
    let loaded = ctx.try_load_vault().map_err(user_text);
    let v = loaded.and_then(|v| v.ok_or_else(|| NOT_GUARD_FILE.to_string()));
    let v = v.map_err(|e| (200, e))?;
    if let Some(stage) = ended_stage(&v) {
        return Ok(Live { kas: 0.0, hops: 0, live: false, stage, coin: None });
    }
    core.connected().await.map_err(|e| (503, e))?;
    let addr = ctx.try_vault_address(&v.state).map_err(|e| (200, user_text(e)))?;
    let coin = match ctx.try_vault_coin(&v).await {
        Ok(c) => c,
        Err(e) if e.starts_with("no vault coin") => return gone_coin(ctx, &v).await,
        Err(e) => return Err((200, user_text(e))),
    };
    let (kas_now, stage) = (kas(coin.1.amount), open_stage(&v));
    Ok(Live { kas: kas_now, hops: 0, live: true, stage, coin: Some((addr, coin.0)) })
}

/// The tx id of the stop this phone sent, when the record ends in one (do_cancel's line ends
/// "tx {id}").
fn sent_stop(v: &Vault) -> Option<&str> {
    let last = v.history.last().filter(|h| h.starts_with("cancel"))?;
    let (_, id) = last.rsplit_once(" tx ")?;
    Some(id.trim()).filter(|id| !id.is_empty())
}

/// A sent stop's coin is gone: `stopped` only when our node shows the vault coin made by that
/// very stop (`want`; `seen` is stop_from_elsewhere filtered by it); false = walk the history.
fn stop_landed(want: &str, seen: Result<Option<String>, String>) -> Result<bool, String> {
    Ok(seen?.is_some_and(|t| t == want))
}

/// The stage of a record whose coin is still live: a sent stop that has not landed does not make
/// it `stopped` (the stage before that stop line).
fn open_stage(v: &Vault) -> &'static str {
    if !v.history.last().is_some_and(|h| h.starts_with("cancel")) {
        return stage_of(v);
    }
    let mut before = v.clone();
    before.history.pop();
    stage_of(&before)
}

/// The printed coin is gone: the history walk decides (`follow`). A safe coin from this address
/// alone never answers `stopped`: a sibling account with the same rules shares the address, so
/// its old stop's coin would block this account's stop. History down = an error, never `stopped`.
/// A record ending in a stop this phone sent answers `stopped` here only when our node shows the
/// vault coin made by that very stop (its tx id from the record).
async fn gone_coin(ctx: &Ctx, v: &Vault) -> Result<Live, (u16, String)> {
    if let Some(t) = sent_stop(v) {
        let seen = ctx.stop_from_elsewhere(v, Some(t)).await;
        if stop_landed(t, seen).map_err(|e| (200, user_text(e)))? {
            return Ok(Live { kas: 0.0, hops: 0, live: false, stage: "stopped", coin: None });
        }
    }
    follow(ctx, v).await
}

/// An error as the user reads it: our own plain sentences pass; anything else (raw node or
/// history text) goes to the log and is answered as the check-failed refusal.
fn user_text(e: String) -> String {
    let known = [
        lineage::CHECK_FAILED, lineage::NOT_FOUND, lineage::OLD_RULES, lineage::SETTLING,
        lineage::UNREADABLE, MOVED, TOOK_TOO_LONG,
    ];
    if known.contains(&e.as_str()) || e.starts_with(MANY_COINS) {
        return e;
    }
    let shown: String = e.chars().take(200).collect();
    eprintln!("paper: check failed: {shown}");
    lineage::CHECK_FAILED.into()
}

/// A walk error as the user reads it: out of time or questions is "try again"; past the most
/// hops is MOVED (a newer sheet); the rest as `user_text`.
fn walk_error(e: String) -> String {
    if e == lineage::TOO_LONG {
        TOOK_TOO_LONG.into()
    } else if lineage::too_long(&e) {
        MOVED.into()
    } else {
        user_text(e)
    }
}

/// The walk ended (the history shows the coin stopped, finished or inherited). The history alone
/// never answers that: `stopped` only when our node shows a vault coin whose tx id is the walk's
/// end (`end`, `stop_from_elsewhere`) AND our node's copy of that tx spends the walked coin
/// (`spend_proof`), else the check-failed refusal. So a forged history cannot pass a sibling's
/// old stop coin off as this stop (no false `stopped`); it can still make the check fail (an
/// honest "try again"), and a long-pruned stop block also answers check-failed.
fn ended_on_node(hops: u32, end: Option<String>, seen: Result<Option<String>, String>)
    -> Result<Live, (u16, String)> {
    match seen {
        Ok(Some(t)) if end.as_deref() == Some(t.as_str()) => {
            Ok(Live { kas: 0.0, hops, live: false, stage: "stopped", coin: None })
        }
        Ok(_) => Err((200, lineage::CHECK_FAILED.into())),
        Err(e) => Err((200, user_text(e))),
    }
}

/// Walks the history from the printed record to the live coin (lineage.rs: every hop checked
/// against the rule block, one walk-wide time and request budget; `walk_error` when it fails),
/// confirms that coin on our own node, and saves the record at it so do_cancel signs that coin.
async fn follow(ctx: &Ctx, v: &Vault) -> Result<Live, (u16, String)> {
    let history = Indexer::new(&ctx.network).map_err(|e| (200, user_text(e)))?;
    let source = WithNode::new(&history, ctx);
    let public = ctx.keys.public();
    let walked = lineage::walk_to_live_from_record(&public, v, &source, Limits::WALK).await;
    let walk = walked.map_err(|e| (200, walk_error(e)))?;
    let hops = u32::try_from(walk.hops).unwrap_or(u32::MAX);
    let Some((found, at)) = walk.live else {
        let end = walk.end_txid.map(|t| t.to_string());
        let seen = end_seen(ctx, &walk).await;
        return ended_on_node(hops, end, seen);
    };
    let coin = confirm_live(&found, ctx.try_vault_coin(&at).await).map_err(|e| (200, e))?;
    ctx.save_anchored(&at).map_err(|e| (200, user_text(e)))?;
    let addr = ctx.try_vault_address(&at.state).map_err(|e| (200, user_text(e)))?;
    let (kas_now, stage) = (kas(coin.1.amount), stage_of(&at));
    Ok(Live { kas: kas_now, hops, live: true, stage, coin: Some((addr, coin.0)) })
}

/// The vault coin a walk's end made counts only when our node's copy of that tx spends the walked
/// coin (`spends`, Ctx::spent_by); a node that cannot say is an error (check failed), never a yes.
fn spend_proof(seen: Option<String>, spends: Result<bool, String>)
    -> Result<Option<String>, String> {
    match (seen, spends) {
        (Some(x), Ok(true)) => Ok(Some(x)),
        (Some(_), Err(e)) => Err(e),
        _ => Ok(None),
    }
}

/// The vault coin the walk's ending spend made, as our node shows it (`stop_from_elsewhere` by
/// that tx id), kept only with the node's proof that the spend took the walked coin.
async fn end_seen(ctx: &Ctx, walk: &lineage::Walk) -> Result<Option<String>, String> {
    let (Some(id), Some(from)) = (walk.end_txid, walk.end_from) else { return Ok(None) };
    let Some(x) = ctx.stop_from_elsewhere(&walk.last, Some(&id.to_string())).await? else {
        return Ok(None);
    };
    spend_proof(Some(x), ctx.spent_by(id, &walk.end_blocks, &from).await)
}

/// A coin with its entry (contract.rs); `Coin` here is a coin's place.
type Held = crate::contract::Coin;

/// The walk's live coin as our own node holds it (same outpoint, script and covenant id), or the
/// check-failed refusal: a coin only the history shows is never signed on.
fn confirm_live(found: &Held, node: Result<Held, String>) -> Result<Held, String> {
    let same = |c: &Held| {
        c.0 == found.0
            && c.1.script_public_key == found.1.script_public_key
            && c.1.covenant_id == found.1.covenant_id
    };
    match node {
        Ok(c) if same(&c) => Ok(c),
        Err(e) if !e.starts_with("no vault coin") => Err(user_text(e)),
        _ => Err(lineage::CHECK_FAILED.into()),
    }
}

/// Whether the vault's coins hold the stop's coin: output 0 of the stop's tx (do_cancel pays
/// exactly one output, index 0, to the vault).
fn stop_seen(cold: &[TransactionOutpoint], txid: &str) -> bool {
    cold.iter().any(|o| o.index == 0 && o.transaction_id.to_string() == txid)
}

/// Whether our node shows the stop's coin in the account's vault: the same looks as `seen_gone`.
/// False when it is not seen in time (the stop was sent, not yet seen landed) or the node is down.
async fn stop_in_vault(ctx: &Ctx, txid: &str) -> bool {
    let Ok(cold) = ctx.network.address(&ctx.keys.public().cold_address) else { return false };
    for _ in 0..GONE_TRIES {
        tokio::time::sleep(GONE_WAIT).await;
        if let Ok(coins) = ctx.try_utxos(&cold).await {
            let held: Vec<TransactionOutpoint> = coins.iter().map(|(o, _)| *o).collect();
            if stop_seen(&held, txid) {
                return true;
            }
        }
    }
    false
}

/// Whether the stopped coin has left the node's coin list: `GONE_TRIES` looks, `GONE_WAIT` apart
/// (about 10 s in all). False when it cannot be seen gone (still there, or no answer).
async fn seen_gone(ctx: &Ctx, coin: &Option<(Address, TransactionOutpoint)>) -> bool {
    let Some((addr, outpoint)) = coin else { return false };
    for _ in 0..GONE_TRIES {
        tokio::time::sleep(GONE_WAIT).await;
        if let Ok(coins) = ctx.try_utxos(addr).await
            && !coins.iter().any(|(o, _)| o == outpoint)
        {
            return true;
        }
    }
    false
}

/// What the owner compares with the paper: the account's vault (cold) address and the first 8
/// characters of its code (public_sha256).
fn ids_of(keys: &Keys) -> (String, String) {
    let p = keys.public();
    let sha = keys.public_sha256.clone().unwrap_or_else(|| p.sha256());
    (p.cold_address, sha.chars().take(8).collect())
}

fn stamp(mut answer: Value, ids: &(String, String)) -> Value {
    answer["vault_address"] = json!(ids.0);
    answer["code"] = json!(ids.1);
    answer
}

/// The account's stage in watch.status's words: `stopped`, `waiting` (a payment waits), `open`.
fn stage_of(v: &Vault) -> &'static str {
    if is_stopped(v) {
        "stopped"
    } else if v.state.stage == 1 {
        "waiting"
    } else {
        "open"
    }
}

fn fail_check(e: &str) -> Value {
    json!({"ok": false, "kas": null, "hops": null, "live": false, "stage": null,
        "vault_address": null, "code": null, "error": plain(e)})
}

fn fail_stop(e: &str) -> Value {
    json!({"ok": false, "tx": null, "kas": null, "hops": null, "confirmed": false,
        "vault_address": null, "code": null, "error": plain(e)})
}

/// `paper.check {watch_code}` (needs the network, read-only): the account the code names and its
/// coin now. Answers {ok, kas, hops, live, stage, vault_address, code, error}; an account already
/// stopped or ended answers offline with `live` false and that stage.
async fn check(core: &Core, args: &Value) -> (u16, Value) {
    let (scratch, keys) = match read_code(&core.ctx.dir, &core.ctx.network, &args["watch_code"]) {
        Ok(r) => r,
        Err(e) => return (200, fail_check(&e)),
    };
    let ids = ids_of(&keys);
    let ctx = watch_ctx(core, &scratch.0, keys);
    match find_live(core, &ctx).await {
        Ok(l) => {
            let answer = json!({"ok": true, "kas": l.kas, "hops": l.hops, "live": l.live,
                "stage": l.stage, "error": null});
            (200, stamp(answer, &ids))
        }
        Err((status, e)) => (status, stamp(fail_check(&e), &ids)),
    }
}

/// `paper.stop {words, watch_code}` (needs the network; works on a phone with no key set): signs
/// the account's stop with the paper stop the words hold. A stop pays only the owner's own vault
/// (the contract allows nothing else), so these words alone never take funds. Answers {ok, tx, kas,
/// hops, confirmed, vault_address, code, error}: `confirmed` true only when our node shows the
/// stop's coin in the vault (`stop_in_vault`); false = sent, not yet seen landed. The secret is
/// wiped on every path (before the after-stop look), and neither words nor code is ever logged.
async fn stop(core: &Core, args: &Value) -> (u16, Value) {
    let secret = match secret_of(&args["words"]) {
        Ok(s) => s,
        Err(e) => return (200, fail_stop(&e)),
    };
    let (scratch, keys) = match read_code(&core.ctx.dir, &core.ctx.network, &args["watch_code"]) {
        Ok(r) => r,
        Err(e) => return (200, fail_stop(&e)),
    };
    let ids = ids_of(&keys);
    let fail = |status: u16, e: &str| (status, stamp(fail_stop(e), &ids));
    let Some(which) = slot_of(&keys.public(), &secret) else { return fail(200, PAPER_WRONG) };
    let look = watch_ctx(core, &scratch.0, keys.clone());
    let armed = Armed(watch_ctx(core, &scratch.0, armed_keys(keys, which, &secret)));
    drop(secret);
    let live = match find_live(core, &armed.0).await {
        Ok(l) if l.live => l,
        Ok(_) => return fail(200, ALREADY_STOPPED),
        Err((status, e)) => return fail(status, &e),
    };
    let lock = core.money_lock();
    let money = lock.lock().await;
    let done = armed.0.do_cancel(which).await;
    drop(money);
    drop(armed);
    let tx = match done {
        Ok(tx) => tx,
        Err(e) => return fail(200, &user_text(e)),
    };
    let confirmed = stop_in_vault(&look, &tx).await;
    let answer = json!({"ok": true, "tx": tx, "kas": live.kas, "hops": live.hops,
        "confirmed": confirmed, "error": null});
    (200, stamp(answer, &ids))
}

fn fail_takeout(e: &str) -> Value {
    json!({"ok": false, "tx": null, "kas": null, "to": null, "confirmed": false, "error": plain(e)})
}

/// The account's vault (cold) address is the 2-of-3 address its three vault keys make.
fn vault_matches(p: &Public, network: &Network) -> bool {
    p.cold_redeem().is_some_and(|r| p2sh_address(network.prefix, &r).to_string() == p.cold_address)
}

/// Where a take-out pays: `to` when given (an address on this network), else this phone's purse
/// when it has keys; refused in plain words otherwise. A `to` that is present but not a text
/// (null, a number, an object) is TO_WRONG, never the purse: only an absent or empty `to` means
/// "pay my purse".
fn takeout_to(core: &Core, to: Option<&Value>) -> Result<String, &'static str> {
    let given = match to {
        None => None,
        Some(Value::String(s)) => Some(s.trim()).filter(|s| !s.is_empty()),
        Some(_) => return Err(TO_WRONG),
    };
    match given {
        Some(t) if t.len() <= MAX_ADDRESS && core.ctx.network.address(t).is_ok() => Ok(t.into()),
        Some(_) => Err(TO_WRONG),
        None if core.ctx.keys.public.is_some() => {
            core.ctx.funding_addr().map(|a| a.to_string()).map_err(|_| TO_NEEDED)
        }
        None => Err(TO_NEEDED),
    }
}

/// The two vault seeds (`seeds`: an array of two texts, each 24 words apart by spaces) put into
/// `keys` at the slots of the account's vault keys `p`. Refused in plain words, never echoing a
/// word: SEEDS_SHAPE for the wrong shape, SEEDS_WRONG unless they are two different of the three.
/// Each secret lives in a Zeroizing until placed; the caller's Armed wipes `keys` on every path.
fn seeds_into(keys: &mut Keys, p: &Public, seeds: &Value) -> Result<(), String> {
    // Limit: `seeds` is borrowed from the caller's args (Core::call takes &Value), so the typed
    // text cannot be wiped here; only borrowed &str views are made, and each secret is Zeroizing.
    let texts: Vec<&str> = match seeds.as_array() {
        Some(a) if a.len() == 2 => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if texts.len() != 2 || texts.iter().map(|t| t.len()).sum::<usize>() > MAX_SEEDS_TEXT {
        return Err(SEEDS_SHAPE.into());
    }
    let mut placed = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let words: Vec<&str> = text.split_whitespace().take(25).collect();
        if words.len() != 24 {
            return Err(SEEDS_SHAPE.into());
        }
        let secret = sheet_from_words(&words).map_err(|e| format!("Vault seed {}: {e}", i + 1));
        let secret = Zeroizing::new(secret?);
        let slot = cold_slot(p, &secret).ok_or(SEEDS_WRONG)?;
        if placed.contains(&slot) {
            return Err(SEEDS_WRONG.into());
        }
        set_cold(keys, slot, &secret);
        placed.push(slot);
    }
    Ok(())
}

/// The first spendable coin in the account's vault, for the after-take-out look; VAULT_EMPTY when
/// it holds none. Errors carry the status to answer (503 = no network).
type Coin = (Address, TransactionOutpoint);

async fn vault_coin(core: &Core, ctx: &Ctx) -> Result<Coin, (u16, String)> {
    core.connected().await.map_err(|e| (503, e))?;
    let addr = ctx.network.address(&ctx.keys.public().cold_address).map_err(|e| (200, e))?;
    let coins = ctx.try_utxos(&addr).await.map_err(|e| (200, e))?;
    let first = coins.into_iter().find(|(_, u)| !u.is_coinbase).map(|(o, _)| o);
    first.map(|o| (addr, o)).ok_or_else(|| (200, VAULT_EMPTY.to_string()))
}

/// `paper.takeout {seeds, watch_code, to?}` (needs the network; works on a phone with no key set):
/// two of the account's three vault seeds are the authority. The code is checked first (watch
/// code checks, then its vault address against its three vault keys), then `to`, and only then are
/// the seeds read. Spends everything in the vault less the fee to `to` through airgap.rs's
/// take-out (the same build, signing and proof as `takeout`). Answers {ok, tx, kas, to, confirmed,
/// error}: `confirmed` false = sent, not yet seen spent. Seeds are wiped before the after-look
/// and never stored or logged.
async fn takeout(core: &Core, args: &Value) -> (u16, Value) {
    let (scratch, keys) = match read_code(&core.ctx.dir, &core.ctx.network, &args["watch_code"]) {
        Ok(r) => r,
        Err(e) => return (200, fail_takeout(&e)),
    };
    let p = keys.public();
    if !vault_matches(&p, &core.ctx.network) {
        return (200, fail_takeout(CODE_VAULT_DIFFERS));
    }
    let to = match takeout_to(core, args.get("to")) {
        Ok(t) => t,
        Err(e) => return (200, fail_takeout(e)),
    };
    let look = watch_ctx(core, &scratch.0, keys.clone());
    let mut armed = Armed(watch_ctx(core, &scratch.0, keys));
    if let Err(e) = seeds_into(&mut armed.0.keys, &p, &args["seeds"]) {
        return (200, fail_takeout(&e));
    }
    let coin = match vault_coin(core, &look).await {
        Ok(c) => Some(c),
        Err((status, e)) => return (status, fail_takeout(&e)),
    };
    let lock = core.money_lock();
    let money = lock.lock().await;
    let done = crate::airgap::takeout(&armed.0, None, &to, None).await;
    drop(money);
    drop(armed);
    let (tx, paid, to) = match done {
        Ok(d) => d,
        Err(e) if e.starts_with(NO_COLD_COINS) => return (200, fail_takeout(VAULT_EMPTY)),
        Err(e) => return (200, fail_takeout(&e)),
    };
    let confirmed = seen_gone(&look, &coin).await;
    let answer = json!({"ok": true, "tx": tx, "kas": kas(paid), "to": to,
        "confirmed": confirmed, "error": null});
    (200, answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Dials, locked, pin_source, save_vault_anchored};
    use crate::core::guard_export;
    use kaspa_consensus_core::Hash;
    use kaspa_wrpc_client::prelude::*;

    #[test]
    fn a_walked_coin_our_node_does_not_hold_is_refused() {
        use kaspa_consensus_core::tx::{ScriptPublicKey, UtxoEntry};
        let coin = |i: u32| {
            let spk = ScriptPublicKey::from_vec(0, vec![0xaa]);
            let op = TransactionOutpoint::new(Hash::from_bytes([1; 32]), i);
            (op, UtxoEntry::new(5, spk, 9, false, Some(Hash::from_bytes([2; 32]))))
        };
        let found = coin(0);
        let miss = Err("no vault coin with covenant id x at y".to_string());
        assert_eq!(confirm_live(&found, miss).unwrap_err(), lineage::CHECK_FAILED);
        assert_eq!(confirm_live(&found, Ok(coin(1))).unwrap_err(), lineage::CHECK_FAILED);
        let mut other = coin(0);
        other.1.covenant_id = None;
        assert_eq!(confirm_live(&found, Ok(other)).unwrap_err(), lineage::CHECK_FAILED);
        assert_eq!(confirm_live(&found, Ok(coin(0))).unwrap().0, found.0);
        let raw = Err("get_utxos_by_addresses: https://node:16210 status 500".to_string());
        assert_eq!(confirm_live(&found, raw).unwrap_err(), lineage::CHECK_FAILED);
    }

    #[test]
    fn an_ended_history_is_believed_only_when_our_node_shows_the_stop() {
        let end = || Some("ab".to_string());
        let unconfirmed = ended_on_node(3, end(), Ok(None)).err().unwrap();
        assert_eq!(unconfirmed, (200, lineage::CHECK_FAILED.to_string()));
        assert_ne!(unconfirmed.1, ALREADY_STOPPED);
        let down = ended_on_node(3, end(), Err("get_utxos_by_addresses: timeout".into()));
        assert_eq!(down.err().unwrap().1, lineage::CHECK_FAILED);
        // A cold coin from another spend (an old legitimate payout at an earlier state's address)
        // is not this walk's stop: refused, never "stopped".
        let forged = ended_on_node(3, end(), Ok(Some("cd".into()))).err().unwrap();
        assert_eq!(forged, (200, lineage::CHECK_FAILED.to_string()));
        let no_end = ended_on_node(3, None, Ok(Some("ab".into()))).err().unwrap();
        assert_eq!(no_end.1, lineage::CHECK_FAILED);
        let seen = ended_on_node(3, end(), Ok(Some("ab".into()))).unwrap();
        assert!(!seen.live && seen.stage == "stopped" && seen.hops == 3 && seen.coin.is_none());
    }

    /// The printed coin is gone and the safe holds a sibling account's old stop coin (same rules,
    /// same address): the walk's end is a different tx, so never "stopped"; the matching tx is.
    #[test]
    fn stopped_only_when_the_vault_holds_the_stops_own_coin() {
        let id = |b: u8| Hash::from_bytes([b; 32]);
        let ours = id(7).to_string();
        let op = |b: u8, i: u32| TransactionOutpoint::new(id(b), i);
        assert!(stop_seen(&[op(1, 0), op(7, 0)], &ours));
        assert!(!stop_seen(&[], &ours), "coin gone but nothing in the vault yet: not stopped");
        assert!(!stop_seen(&[op(7, 1)], &ours), "only output 0 is the stop's");
        assert!(!stop_seen(&[op(8, 0)], &ours), "a sibling's vault coin is not this stop");
    }

    /// A record ending in a stop this phone sent: never answered from the record alone.
    #[test]
    fn a_sent_stop_is_checked_on_the_node_and_stays_retryable() {
        let mut v = Vault { covenant_id: Hash::from_bytes([9u8; 32]), state: locked(),
            pending_to: None, history: vec!["funded".into()], contract_sha256: None,
            network: String::new(), dials: Dials::testnet() };
        assert_eq!(sent_stop(&v), None);
        let open = open_stage(&v);
        v.history.push("cancel (stop key 1): everything to cold, fee 0.00010 KAS, tx ab12".into());
        assert_eq!(sent_stop(&v), Some("ab12"));
        assert_eq!(ended_stage(&v), None, "a sent stop is not `stopped` offline");
        // coin still live: the stage before the stop, so paper.stop sends it again
        assert_eq!(open_stage(&v), open);
        assert_ne!(open_stage(&v), "stopped");
        // coin gone: the vault coin of that very stop = stopped; else walk / check failed
        assert_eq!(stop_landed("ab12", Ok(Some("ab12".into()))), Ok(true));
        assert_eq!(stop_landed("ab12", Ok(Some("5151".into()))), Ok(false));
        assert_eq!(stop_landed("ab12", Ok(None)), Ok(false));
        assert!(stop_landed("ab12", Err("down".into())).is_err());
    }

    /// The walk's end names a vault coin: it counts only when our node shows the ending tx
    /// spending the walked coin (a forged history naming a sibling's real stop tx fails).
    #[test]
    fn a_walk_end_counts_only_with_the_nodes_proof_of_the_spend() {
        let end = || Some("ab".to_string());
        let proven = spend_proof(Some("ab".into()), Ok(true));
        assert_eq!(ended_on_node(1, end(), proven).unwrap().stage, "stopped");
        let forged = ended_on_node(1, end(), spend_proof(Some("ab".into()), Ok(false)));
        assert_eq!(forged.err().unwrap(), (200, lineage::CHECK_FAILED.to_string()));
        let no_copy = spend_proof(Some("ab".into()), Err("no copy".into()));
        let unknown = ended_on_node(1, end(), no_copy);
        assert_eq!(unknown.err().unwrap().1, lineage::CHECK_FAILED);
        assert_eq!(spend_proof(None, Ok(true)), Ok(None));
    }

    #[test]
    fn a_sibling_stop_coin_never_answers_stopped_for_a_gone_coin() {
        let end = || Some("ab".to_string());
        let sibling = ended_on_node(0, end(), Ok(Some("5151".into()))).err().unwrap();
        assert_eq!(sibling, (200, lineage::CHECK_FAILED.to_string()));
        assert_ne!(sibling.1, ALREADY_STOPPED);
        let filtered_out = ended_on_node(0, end(), Ok(None)).err().unwrap();
        assert_ne!(filtered_out.1, ALREADY_STOPPED);
        let ours = ended_on_node(0, end(), Ok(Some("ab".into()))).unwrap();
        assert_eq!(ours.stage, "stopped");
    }

    #[test]
    fn walk_errors_read_plainly() {
        assert_eq!(walk_error(lineage::TOO_LONG.into()), TOOK_TOO_LONG);
        assert_eq!(walk_error("The account's history is longer than 500 steps".into()), MOVED);
        assert_eq!(walk_error(lineage::SETTLING.into()), lineage::SETTLING);
        assert_eq!(walk_error("bad script at https://x/api 404".into()), lineage::CHECK_FAILED);
        let many = format!("{MANY_COINS} 2. No funds were moved.");
        assert_eq!(user_text(many.clone()), many);
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kv-paper-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ctx(dir: &Path, keys: Keys, rpc: Option<KaspaRpcClient>) -> Ctx {
        let (network, dials) = (Network::testnet(), Dials::testnet());
        Ctx { dir: dir.to_path_buf(), keys, rpc, force: false, network, dials, account: 1 }
    }

    /// An owner folder (full keys on disk) with an account record, its watch code, and an offline
    /// Core over its public block.
    fn owner(tag: &str) -> (PathBuf, Keys, String, Core) {
        let net = Network::testnet();
        let dir = tmp(tag);
        let full = Keys::fresh(&net);
        full.save(&dir.join("keys.json"), true).unwrap();
        let sha = pin_source(&dir).unwrap();
        let v = Vault { covenant_id: Hash::from_bytes([9u8; 32]), state: locked(), pending_to: None,
            history: vec![], contract_sha256: Some(sha), network: net.name.into(),
            dials: Dials::testnet() };
        save_vault_anchored(&dir.join("state.json"), &v, &full.public().sha256()).unwrap();
        let code = guard_export(&dir, &net, None, &[], None).unwrap().to_string();
        let public = Keys::load_public(&dir.join("keys.json")).unwrap();
        let core = Core::open(ctx(&dir, public, None)).unwrap();
        (dir, full, code, core)
    }

    /// A new phone with no key set whose node cannot be reached.
    fn bare_phone(tag: &str) -> (PathBuf, Core) {
        let dir = std::env::temp_dir().join(format!("kv-paper-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let id = Some(Network::testnet().resolver_id());
        let url = Some("ws://127.0.0.1:9");
        let rpc = KaspaRpcClient::new(WrpcEncoding::Borsh, url, None, id, None).unwrap();
        (dir.clone(), Core::open(ctx(&dir, Keys::default(), Some(rpc))).unwrap())
    }

    fn unheld(dir: &Path, keys: &[&str]) {
        let file = json!({"since_setup": true, "keys": keys});
        std::fs::write(dir.join(UNHELD_FILE), file.to_string()).unwrap();
    }

    fn holders(two: &str, three: &str) -> Value {
        json!([{"which": 1, "holder": "you"}, {"which": 2, "holder": two},
            {"which": 3, "holder": three}])
    }

    /// `unknown` ranks like a guardian: with no clearly free slot the owner must choose, and an
    /// asked slot is never a guardian's (nor beside the paper stop it would leave working).
    #[test]
    fn the_paper_stop_takes_slot_two_unless_a_guardian_holds_it() {
        let cases = [
            ("you", "you", None, Ok(2)), ("unheld", "guardian", None, Ok(2)),
            ("unknown", "unknown", None, Err(SLOT_UNSURE)), ("unknown", "unheld", None, Ok(3)),
            ("unknown", "guardian", None, Err(SLOT_UNSURE)), ("unknown", "you", None, Ok(3)),
            ("guardian", "unheld", None, Ok(3)), ("guardian", "you", None, Ok(3)),
            ("paper", "guardian", None, Ok(2)), ("guardian", "paper", None, Ok(3)),
            ("unheld", "paper", None, Ok(3)), ("paper", "paper", None, Ok(2)),
            ("paper", "unknown", None, Ok(2)), ("guardian", "guardian", None, Err(BOTH_GUARDIANS)),
            ("unknown", "unknown", Some(3), Ok(3)), ("unknown", "guardian", Some(2), Ok(2)),
            ("unknown", "guardian", Some(3), Err(GUARDIAN_SLOT)),
            ("guardian", "unknown", Some(2), Err(GUARDIAN_SLOT)),
            ("guardian", "guardian", Some(2), Err(BOTH_GUARDIANS)),
            ("paper", "unknown", Some(3), Err(PAPER_ELSEWHERE)), ("paper", "you", Some(2), Ok(2)),
            ("you", "unheld", Some(3), Ok(3)),
        ];
        for (two, three, asked, want) in cases {
            assert_eq!(pick_slot(&holders(two, three), asked), want, "{two} {three} {asked:?}");
        }
        assert_eq!(slot_arg(&json!({})), Ok(None));
        assert_eq!(slot_arg(&json!({"slot": 3})), Ok(Some(3)));
        for bad in [json!(1), json!(4), json!("2"), json!(2.5), json!(-2), json!([2])] {
            assert_eq!(slot_arg(&json!({ "slot": bad })), Err(BAD_SLOT), "{bad}");
        }
    }

    /// Who holds what decides the slot (as the owner's block will be once waiting changes apply);
    /// a refusal writes nothing.
    #[tokio::test]
    async fn make_picks_the_slot_by_who_holds_it_and_refuses_two_guardians() {
        let (dir, full, _, core) = owner("slots");
        std::fs::remove_file(dir.join("state.json")).unwrap(); // no open account: offline
        let p = full.public();
        let other = Keys::fresh(&Network::testnet()).public().spend_x;
        let waiting_guardian = json!({"cancel3_x": other}).to_string();
        type Case<'a> = (Option<Vec<&'a str>>, Option<&'a str>, Value, Value, &'a str);
        let cases: [Case; 7] = [
            (None, None, json!({}), Value::Null, SLOT_UNSURE),
            (None, None, json!({"slot": 3}), json!(3), ""),
            (Some(vec![&p.cancel2_x]), None, json!({}), json!(2), ""),
            (Some(vec![&p.cancel3_x]), None, json!({}), json!(3), ""),
            (Some(vec![]), None, json!({}), Value::Null, BOTH_GUARDIANS),
            (Some(vec![&p.cancel3_x]), Some(&waiting_guardian), json!({}), Value::Null,
                BOTH_GUARDIANS),
            (Some(vec![&p.cancel3_x]), None, json!({"slot": 2}), Value::Null, GUARDIAN_SLOT),
        ];
        for (list, people, args, slot, error) in cases {
            for f in ["people-next.json", UNHELD_FILE, PENDING_FILE] {
                std::fs::remove_file(dir.join(f)).ok();
            }
            if let Some(l) = list {
                unheld(&dir, &l);
            }
            if let Some(text) = people {
                std::fs::write(dir.join("people-next.json"), text).unwrap();
            }
            let a = make(&core, &args).await;
            assert_eq!(a["slot"], slot, "{a}");
            if slot.is_null() {
                assert_eq!((&a["ok"], &a["error"]), (&json!(false), &json!(error)));
                assert!(a["words"] == json!([]) && read_pending(&dir).is_none(), "nothing made");
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// make stores nothing but the pending public key; only the words typed back (paper.confirm)
    /// make the stop wait like a guardian; a second make replaces a pending one; a confirmed stop
    /// is replaced in its own slot; the secret is in no file and no answer but make's words.
    #[tokio::test]
    async fn make_waits_for_the_words_typed_back_then_like_a_guardian() {
        let (dir, full, _, core) = owner("make");
        std::fs::remove_file(dir.join("state.json")).unwrap(); // no open account: offline
        let p = full.public();
        unheld(&dir, &[&p.cancel3_x]);
        let words_of = |a: &Value| -> Vec<String> {
            serde_json::from_value(a["words"].clone()).unwrap()
        };
        let a = make(&core, &json!({})).await;
        assert_eq!(a, json!({"ok": true, "words": a["words"], "slot": 3, "error": null}));
        let stored = crate::core::read_people_next(&dir).unwrap();
        assert!(stored.is_none() && paper_keys(&dir).is_empty(), "make stores no key");
        let b = make(&core, &json!({})).await;
        let (wa, wb) = (words_of(&a), words_of(&b));
        let secret = sheet_from_words(&wb).unwrap();
        let x = try_xonly_hex(&secret).unwrap();
        assert_eq!(read_pending(&dir), Some((x.clone(), 3)), "the second make replaced the first");
        let other = sheet_words(&Keys::fresh(&Network::testnet()).cancel2.unwrap()).join(" ");
        for w in [json!(wa.join(" ")), json!(other), json!("nope"), Value::Null] {
            let c = confirm(&core, &json!({ "words": w })).await;
            assert_eq!((&c["ok"], &c["error"]), (&json!(false), &json!(WORDS_DIFFER)), "{c}");
        }
        assert!(crate::core::read_people_next(&dir).unwrap().is_none(), "nothing stored yet");
        let c = confirm(&core, &json!({"words": wb})).await;
        assert_eq!((&c["ok"], &c["slot"]), (&json!(true), &json!(3)), "{c}");
        assert!(c["route"].is_string() && c["rule_words"].is_array() && c.get("words").is_none());
        let waiting = crate::core::read_people_next(&dir).unwrap().unwrap();
        assert_eq!(waiting.get("cancel3_x"), Some(&json!(x)));
        assert_eq!(crate::core::read_unheld(&dir), (vec![p.cancel3_x.clone()], true));
        assert!(read_pending(&dir).is_none() && paper_keys(&dir) == vec![x.clone()]);
        let again = confirm(&core, &json!({"words": wb})).await;
        assert_eq!(again["error"], json!(NOT_PENDING));
        let d = make(&core, &json!({})).await;
        assert_eq!(d["slot"], json!(3), "the waiting paper stop is replaced, not doubled");
        let (hex, phrase) = (faster_hex::hex_string(&secret), wb.join(" "));
        assert!(!c.to_string().contains(&hex) && !c.to_string().contains(&wb[0]));
        assert!(!d.to_string().contains(&phrase));
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            assert!(!text.contains(&hex) && !text.contains(&phrase), "{:?}", e.path());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// unheld.json is replaced whole (a temp file renamed over it), never rewritten in place: a
    /// second name for the old file keeps the old text, and no temp file is left.
    #[test]
    fn the_unheld_list_is_written_whole_or_not_at_all() {
        let dir = tmp("atomic");
        unheld(&dir, &["aa"]);
        let witness = dir.join("witness.json");
        std::fs::hard_link(dir.join(UNHELD_FILE), &witness).unwrap();
        let before = std::fs::read_to_string(&witness).unwrap();
        crate::core::record_unheld(&dir, &["bb".into()], false).unwrap();
        assert_eq!(std::fs::read_to_string(&witness).unwrap(), before, "record_unheld");
        std::fs::remove_file(&witness).unwrap();
        std::fs::hard_link(dir.join(UNHELD_FILE), &witness).unwrap();
        let before = std::fs::read_to_string(&witness).unwrap();
        record_paper(&dir, "cc").unwrap();
        assert_eq!(std::fs::read_to_string(&witness).unwrap(), before, "record_paper");
        assert_eq!(crate::core::read_unheld(&dir).0, vec!["aa".to_string(), "bb".into()]);
        assert_eq!(paper_keys(&dir), vec!["cc".to_string()]);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(names.len(), 2, "no temp file left: {names:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A slot repeating stop 1 never matches: the phone's own stop words are not a paper stop.
    #[test]
    fn a_slot_repeating_stop_one_never_matches() {
        let full = Keys::fresh(&Network::testnet());
        let mut p = full.public();
        p.cancel2_x = p.cancel_x.clone();
        assert_eq!(slot_of(&p, &full.cancel.unwrap()), None);
        p.cancel3_x = p.cancel_x.to_uppercase();
        assert_eq!(slot_of(&p, &full.cancel.unwrap()), None);
        let q = full.public();
        assert_eq!(slot_of(&q, &full.cancel3.unwrap()), Some(3));
    }

    /// At most one keyless call per gap: one let through is remembered, one refused is not.
    #[test]
    fn keyless_calls_wait_their_turn() {
        let last = Mutex::new(None);
        let (t0, gap) = (Instant::now(), Duration::from_secs(2));
        assert!(!too_soon(&last, t0, gap));
        assert!(too_soon(&last, t0 + Duration::from_millis(1_999), gap));
        assert!(!too_soon(&last, t0 + gap, gap));
        assert!(too_soon(&last, t0 + gap + Duration::from_secs(1), gap));
        assert!(!too_soon(&last, t0 + gap * 2, gap));
        assert!(!too_soon(&Mutex::new(None), t0, Duration::ZERO));
    }

    #[test]
    fn people_shows_the_paper_slot_and_the_unheld_list_keeps_it() {
        let dir = tmp("mark");
        let mut p = Keys::fresh(&Network::testnet()).public();
        let x = p.cancel3_x.clone();
        unheld(&dir, &[&p.cancel2_x]);
        record_paper(&dir, &x).unwrap();
        let shown = |p: &Public| {
            let held = crate::core::stop_holders(p, &crate::core::read_unheld(&dir));
            let h = mark_paper(held, p, &dir);
            [h[1]["holder"].clone(), h[2]["holder"].clone()]
        };
        assert_eq!(shown(&p), [json!("unheld"), json!("paper")]);
        crate::core::record_unheld(&dir, &["ab".into()], false).unwrap();
        assert_eq!(paper_keys(&dir), vec![x], "record_unheld keeps the paper list");
        p.cancel3_x = String::new();
        assert_eq!(shown(&p), [json!("unheld"), json!("you")]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// paper.sheet is guard.export's own answer as text, with no secret in it.
    #[tokio::test]
    async fn the_sheet_is_the_watch_code_and_holds_no_secret() {
        let (dir, full, code, core) = owner("sheet");
        let a = sheet(&core, &json!({})).await;
        assert_eq!(a["ok"], json!(true), "{a}");
        let text = a["watch_code"].as_str().unwrap();
        let sent: Value = serde_json::from_str(text).unwrap();
        assert_eq!(sent, serde_json::from_str::<Value>(&code).unwrap());
        for s in [full.spend, full.cancel, full.cancel2, full.cancel3, full.cold, full.heir] {
            assert!(!text.contains(&faster_hex::hex_string(&s.unwrap())));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// On a phone with no key set: paper.check and paper.stop with good words reach the network
    /// gate; wrong or hostile words are refused first, without a panic; no file is left behind
    /// and no answer holds the secret.
    #[tokio::test]
    async fn a_phone_with_no_keys_checks_offline_then_meets_the_network() {
        let (owner_dir, full, code, _) = owner("bare-owner");
        let (dir, core) = bare_phone("bare");
        let words = |s: Option<[u8; 32]>| sheet_words(&s.unwrap()).join(" ");
        let good = words(full.cancel2);
        let secret = faster_hex::hex_string(&full.cancel2.unwrap());
        let mut answers = vec![];
        for w in [good.clone(), words(full.cancel3)] {
            let (s, a) = core.call("paper.stop", &json!({"words": w, "watch_code": code})).await;
            assert_eq!((s, &a["ok"], &a["tx"]), (503, &json!(false), &Value::Null), "{a}");
            answers.push(a);
        }
        let (s, a) = core.call("paper.check", &json!({"watch_code": code})).await;
        assert_eq!((s, &a["live"]), (503, &json!(false)), "{a}");
        answers.push(a);
        let other = Keys::fresh(&Network::testnet());
        let short = good.rsplit_once(' ').map(|w| w.0.to_string()).unwrap_or_default();
        let hostile = [json!(""), json!(words(full.spend)), json!(words(other.cancel2)),
            json!(short), json!(format!("{good} abandon")), json!(["zzzz"; 24].join(" ")),
            json!(["abandon"; 24].join(" ")), json!("a".repeat(1 << 20)),
            json!(["ábaco"; 24].join(" ")), json!(5), Value::Null, json!(vec![7; 24]),
            json!(vec!["x".repeat(1 << 20); 24])];
        for w in hostile {
            let (s, a) = core.call("paper.stop", &json!({"words": w, "watch_code": code})).await;
            assert_eq!((s, &a["error"]), (200, &json!(PAPER_WRONG)));
            answers.push(a);
        }
        for a in &answers {
            assert!(!a.to_string().contains(&good) && !a.to_string().contains(&secret), "{a}");
        }
        assert_eq!(std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0), 0, "no file left");
        let made = core.call("paper.make", &json!({})).await;
        assert_eq!(made, (200, crate::core::no_keys()), "paper.make needs the key set");
        let typed = core.call("paper.confirm", &json!({"words": good})).await;
        assert_eq!(typed, (200, crate::core::no_keys()), "paper.confirm needs the key set");
        assert_eq!(core.call("paper.sheet", &json!({})).await, (200, crate::core::no_keys()));
        let (s, st) = core.call("status", &json!({})).await;
        assert_eq!((s, &st["no_keys"]), (200, &json!(true)), "status with no key set still refuses");
        assert!(st.get("daa_now").is_some() && st["network"].is_string(), "carries network time: {st}");
        for d in [&owner_dir, &dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// Every answer after the code is read names the account's vault address and code (the first
    /// 8 characters of public_sha256) for the owner to compare with the paper.
    #[tokio::test]
    async fn answers_name_the_vault_address_and_code_to_compare() {
        let (owner_dir, full, code, _) = owner("ids-owner");
        let (dir, core) = bare_phone("ids");
        let sha = full.public().sha256();
        let want = (json!(full.public().cold_address), json!(sha[..8]));
        let other = sheet_words(&Keys::fresh(&Network::testnet()).cancel2.unwrap()).join(" ");
        let good = sheet_words(&full.cancel2.unwrap()).join(" ");
        let calls = [("paper.stop", other, 200), ("paper.stop", good, 503),
            ("paper.check", String::new(), 503)];
        for (method, words, status) in calls {
            let (s, a) = core.call(method, &json!({"words": words, "watch_code": code})).await;
            assert_eq!(s, status, "{method} {a}");
            assert_eq!((a["vault_address"].clone(), a["code"].clone()), want, "{method} {a}");
        }
        let (_, cut) = core.call("paper.check", &json!({"watch_code": "{}"})).await;
        assert!(cut["vault_address"].is_null() && cut["code"].is_null(), "{cut}");
        for d in [&owner_dir, &dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// A record already ended answers its stage offline, not "moved" (a watch code carries no
    /// history; the record comes from lineage's walk or the stop seen on the node). A record
    /// ending in a stop this phone sent is never `stopped` offline: the node must show it landed.
    #[tokio::test]
    async fn an_ended_account_answers_its_stage_not_moved() {
        let (owner_dir, full, _, _) = owner("ended-owner");
        let (dir, core) = bare_phone("ended");
        let state = owner_dir.join("state.json");
        let ended = [("cancel 5 KAS to the safe, tx ab12", "stopped"), ("inherit 5 KAS", "ended"),
            ("finish 5 KAS; the vault is now empty", "ended")];
        for (last, stage) in ended {
            let text = std::fs::read_to_string(&state).unwrap();
            let mut v: Vault = serde_json::from_str(&text).unwrap();
            v.history.push(last.to_string());
            save_vault_anchored(&state, &v, &full.public().sha256()).unwrap();
            let public = Keys::load_public(&owner_dir.join("keys.json")).unwrap();
            let ctx = watch_ctx(&core, &owner_dir, public);
            if last.starts_with("cancel") {
                let sent = find_live(&core, &ctx).await;
                assert!(sent.is_err_and(|e| e.1 != ALREADY_STOPPED), "{last}: {stage} unchecked");
                continue;
            }
            let l = find_live(&core, &ctx).await.map_err(|e| e.1).unwrap();
            let got = (l.live, l.stage, l.kas, l.coin.is_none());
            assert_eq!(got, (false, stage, 0.0, true), "{last}");
        }
        for d in [&owner_dir, &dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// Each keyless call first removes scratch folders another process left behind; a fresh one of
    /// this process (a call still running) stays.
    #[tokio::test]
    async fn left_over_scratch_folders_are_swept() {
        let (dir, core) = bare_phone("sweep");
        let theirs = dir.join(format!("{SCRATCH_PREFIX}-0-7"));
        let ours = dir.join(format!("{SCRATCH_PREFIX}-{}-999999", std::process::id()));
        let keep = dir.join("paper-pending.json");
        for d in [&theirs, &ours] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("keys.json"), "{}").unwrap();
        }
        std::fs::write(&keep, "{}").unwrap();
        let _ = core.call("paper.check", &json!({"watch_code": 1})).await;
        assert!(!theirs.exists() && ours.exists() && keep.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The after-stop look answers false when the node cannot show the coin gone (no node here),
    /// and false with no coin to look for; it is bounded.
    #[tokio::test]
    async fn an_unseen_stop_is_not_called_confirmed() {
        let (dir, core) = bare_phone("gone");
        let ctx = watch_ctx(&core, &dir, Keys::default());
        let net = Network::testnet();
        let addr = net.address(&Keys::fresh(&net).public().cold_address);
        let outpoint = TransactionOutpoint::new(Hash::from_bytes([3u8; 32]), 0);
        let coin = Some((addr.unwrap(), outpoint));
        let look = tokio::time::timeout(Duration::from_secs(60), seen_gone(&ctx, &coin)).await;
        assert_eq!(look, Ok(false));
        assert!(!seen_gone(&ctx, &None).await);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A watch code that was changed, cut or swapped is refused by paper.check and paper.stop.
    #[tokio::test]
    async fn tampered_watch_codes_are_refused() {
        let (owner_dir, full, code, _) = owner("tamper-owner");
        let (dir, core) = bare_phone("tamper");
        let good = sheet_words(&full.cancel2.unwrap()).join(" ");
        let mut swapped: Value = serde_json::from_str(&code).unwrap();
        swapped["public"]["cancel2_x"] = json!(Keys::fresh(&Network::testnet()).public().spend_x);
        let mut kind: Value = serde_json::from_str(&code).unwrap();
        kind["kind"] = json!("something-else");
        let cut = code[..code.len() / 2].to_string();
        let codes = [json!(swapped.to_string()), json!(kind.to_string()), json!(cut), json!(""),
            json!("x".repeat(1 << 20)), json!(42), Value::Null];
        for c in codes {
            for method in ["paper.check", "paper.stop"] {
                let (s, a) = core.call(method, &json!({"words": good, "watch_code": c})).await;
                assert_eq!((s, &a["ok"]), (200, &json!(false)), "{method} {a}");
                assert!(a["error"].as_str().is_some_and(|e| !e.is_empty()));
            }
        }
        assert_eq!(std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0), 0, "no file left");
        for d in [&owner_dir, &dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    fn seed(s: &Option<[u8; 32]>) -> String {
        sheet_words(&s.unwrap()).join(" ")
    }

    async fn takeout_of(core: &Core, seeds: Value, code: &str, to: Value) -> Value {
        let args = json!({"seeds": seeds, "watch_code": code, "to": to});
        core.call("paper.takeout", &args).await.1
    }

    /// Only two different of the account's own three vault seeds pass; every refusal comes before
    /// the network (the bare phone's node is unreachable) and never echoes a word.
    #[tokio::test]
    async fn takeout_needs_two_different_of_the_three_vault_seeds() {
        let (owner_dir, full, code, _) = owner("takeout-owner");
        let (dir, core) = bare_phone("takeout");
        let other = Keys::fresh(&Network::testnet());
        let to = json!(full.public().cold_address);
        let (a, b) = (seed(&full.cold), seed(&full.cold3));
        let cases = [
            (json!([a, seed(&other.cold)]), SEEDS_WRONG),
            (json!([seed(&other.cold), seed(&other.cold2)]), SEEDS_WRONG),
            (json!([a, a]), SEEDS_WRONG),
            (json!([seed(&full.cancel2), b]), SEEDS_WRONG),
            (json!([a]), SEEDS_SHAPE), (json!(a), SEEDS_SHAPE), (Value::Null, SEEDS_SHAPE),
            (json!([a, "abandon ".repeat(23)]), SEEDS_SHAPE),
            (json!([a, "x".repeat(MAX_SEEDS_TEXT)]), SEEDS_SHAPE),
        ];
        for (seeds, want) in cases {
            let got = takeout_of(&core, seeds, &code, to.clone()).await;
            assert_eq!((&got["ok"], got["error"].as_str()), (&json!(false), Some(want)), "{got}");
        }
        let right = takeout_of(&core, json!([b, a]), &code, to.clone()).await;
        assert_ne!(right["error"].as_str(), Some(SEEDS_WRONG), "the right seeds meet the network");
        assert_eq!((right["ok"].clone(), right["confirmed"].clone()), (json!(false), json!(false)));
        let wrong_to = takeout_of(&core, json!([a, b]), &code, json!("kaspa:nope")).await;
        assert_eq!(wrong_to["error"].as_str(), Some(TO_WRONG));
        let bare = json!({"seeds": [a, b], "watch_code": code});
        let no_to = core.call("paper.takeout", &bare).await.1;
        assert_eq!(no_to["error"].as_str(), Some(TO_NEEDED), "no keys here, no purse");
        for odd in [Value::Null, json!(5), json!({"address": "x"}), json!([])] {
            let got = takeout_of(&core, json!([a, b]), &code, odd.clone()).await;
            assert_eq!(got["error"].as_str(), Some(TO_WRONG), "a present non-text to: {odd}");
        }
        for d in [&owner_dir, &dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// A code whose vault address is not the one its three vault keys make is refused before any
    /// seed is read (garbage seeds would otherwise be the refusal).
    #[tokio::test]
    async fn takeout_checks_the_vault_address_before_the_seeds() {
        let (owner_dir, full, code, _) = owner("takeout-cold-owner");
        let (dir, core) = bare_phone("takeout-cold");
        let mut p = full.public();
        assert!(vault_matches(&p, &Network::testnet()));
        p.cold_address = Keys::fresh(&Network::testnet()).public().cold_address;
        assert!(!vault_matches(&p, &Network::testnet()));
        let mut c: Value = serde_json::from_str(&code).unwrap();
        c["public"]["cold_address"] = json!(p.cold_address);
        c["public_sha256"] = json!(p.sha256());
        let got = takeout_of(&core, json!(["junk", "junk"]), &c.to_string(), Value::Null).await;
        assert_eq!(got["ok"], json!(false));
        let e = got["error"].as_str().unwrap_or("");
        assert!(e == CODE_VAULT_DIFFERS || e == NOT_GUARD_FILE, "{e}");
        for d in [&owner_dir, &dir] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// paper.takeout shares the keyless rate limit and answers WAIT in its own shape.
    #[test]
    fn takeout_waits_like_the_other_keyless_calls() {
        let last = Mutex::new(None);
        let (t0, gap) = (Instant::now(), Duration::from_secs(2));
        assert!(!too_soon(&last, t0, gap));
        assert!(too_soon(&last, t0 + Duration::from_millis(500), gap));
        let w = fail_takeout(WAIT);
        let got = (&w["error"], &w["confirmed"], &w["to"]);
        assert_eq!(got, (&json!(WAIT), &json!(false), &Value::Null));
    }
}
