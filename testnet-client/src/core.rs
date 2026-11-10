//! `core`: the vault service without its transport. `Core` holds the open folder (`Ctx`), the status
//! cache, the remembered sends and the one-at-a-time money lock; `Core::call(method, args)` answers
//! every request the way the laptop's https routes do, so serve.rs (the laptop) and the Android
//! library (the phone, through JNI) drive one body of code. Money actions run one at a time (the
//! emergency stop never waits); status is cached for a moment; every refusal is in bank words.
//! `contract::Vault` is the state record on disk; this is the live service around it.

use crate::airgap::NOTHING_WAITING;
use crate::contract::*;
use crate::keys::*;
use crate::network::Network;
use crate::tx::*;
use kaspa_consensus_core::tx::{Transaction, TransactionOutput, UtxoEntry};
use kaspa_wrpc_client::prelude::*;
use secp256k1::Keypair;
use serde_json::{Value, json};
use silverscript_abi::ArtifactValue as V;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zeroize::{Zeroize, Zeroizing};

const STATUS_TTL: Duration = Duration::from_secs(3);
/// Answers to /api/send remembered by the app's `id`, so a repeated request pays once.
const REMEMBERED_SENDS: usize = 32;
/// A coin this young with no sighting yet is a transaction still landing, not an empty vault.
pub(crate) const STATE_SETTLE_SECONDS: u64 = 60;
/// The watcher's file in the vault folder (watch.rs writes it, this service only reads it).
const ALERTS_FILE: &str = "alerts.json";
/// How many of the watcher's alerts the status carries, newest first.
const ALERTS_SHOWN: usize = 10;
/// A watcher heartbeat older than this is a quiet guard: its view of the coin no longer overrides state.json
/// (the same 180 s after which the phone shows the quiet-guard banner).
const WATCHER_FRESH: u64 = 180;

const NOT_OPEN: &str = "There is no open account.";
const STOPPED: &str = "The account has been stopped; all funds went to your vault.";
const BUSY: &str = "Another payment is still in progress. Try again in a moment.";
const BAD_ADDRESS: &str = "That address doesn't look right";
const BAD_AMOUNT: &str = "That amount doesn't look right";
pub(crate) const HEIR_KEY_HERE: &str = "The heir's key must not be on this device. Move it off and start again.";
const NETWORK_REFUSED: &str = "The network did not accept that. No funds were moved.";
/// A submit whose answer was lost while the funds may have moved (tx::ANSWER_LOST_MARK): never "nothing moved".
const ANSWER_LOST: &str = "The network's answer was lost. Wait a minute and check your balance before trying again.";
pub(crate) const NETWORK_DOWN: &str = "The network can't be reached right now. Try again in a moment.";
const RECORDS_UNREADABLE: &str = "This phone's account records could not be read. No funds were moved.";
/// Any method before `keys.new` has run on this device.
pub(crate) const NO_KEYS: &str = "This account is not set up yet. Complete setup first.";
/// `call_with_secret` with a key block of the wrong length for its slot.
pub(crate) const KEY_SIZE: &str = "The unlocked key is not the right size for this move.";
pub(crate) const SAFE_SHEETS_HERE: &str = "The vault seeds must not be on this device. Move them off and start again.";
/// `call_with_secret` with a slot that is not the one the method signs with.
pub(crate) const WRONG_KEY: &str = "That key does not sign this move.";
/// `takeout` through `call` (no sheets armed), or sheets that did not arm.
const TAKEOUT_NEEDS_SHEETS: &str = "A take-out needs two of the account's three vault seeds.";
const SHEETS_UNREADABLE: &str = "The vault seeds could not be read as words.";
const NOT_A_SAFE_SHEET: &str = "That seed is not one of this account's three vault seeds.";
const SAME_SHEET: &str = "Those are two copies of the same vault seed.";
const LEGACY_SAFE: &str = "This account's vault has a single seed, not three.";
/// The phone's sealed safe keys do not make this account's safe (another account's keys, or a torn store).
const SAFE_KEYS_MISMATCH: &str = "The vault seeds kept on this phone are not this account's. Type the seeds from paper.";
/// `keys.sheets` or a paperless `takeout` after `keys.burn`.
pub(crate) const SHEETS_BURNED: &str = "The vault seeds were deleted from this phone. Only your paper copies open the vault now.";
const RESET_UNCONFIRMED: &str = "Starting over needs a confirmation.";
/// Longest `Slot::Sheets` text accepted (two sheets are about 300 bytes).
const SHEETS_TEXT_MAX: usize = 4096;
/// Flag file in the wallet folder: the phone made the safe itself and keeps its three keys sealed
/// (android.rs keysNew mode "phone" writes it, `keys.burned` removes it; status says `sheets_on_phone`).
pub const SHEETS_ON_PHONE_FILE: &str = "sheets-on-phone";
/// The last status answer (balances, addresses, nothing secret), written on every fresh build: a restart shows it,
/// marked stale, while the node connect runs (android.rs `call_inner`: balances took 23 s to appear).
pub const STATUS_CACHE_FILE: &str = "status-cache.json";
/// The files `keys.reset` carries into the `<network>-old-<seconds>` sibling folder (alerts*.json too).
/// Stop-slot keys nobody holds (phone setup's fillers, removed guardians): People tells them from a guardian by this list.
pub const UNHELD_FILE: &str = "unheld.json";
const RESET_FILES: [&str; 9] = [
    "keys.json", "state.json", "state.times.json", "vault.sil", "vault-public.json", "watch.json",
    SHEETS_ON_PHONE_FILE, STATUS_CACHE_FILE, UNHELD_FILE,
];
/// The per-account files `keys.reset` carries too: `state-N.json`, `status-cache-N.json`, `rules-next-N.json`,
/// the key-set-wide `people-next.json` and the `closed/` folder of old records.
fn is_account_file(name: &str) -> bool {
    (name.starts_with("state-") && name.ends_with(".json")) || name.starts_with("status-cache") || name.starts_with("rules-next") || name.starts_with("carry-") || name == PEOPLE_NEXT_FILE || name == crate::commands::CLOSED_DIR
}
/// The status cache of one account: `status-cache.json` for account 1, `status-cache-N.json` after.
pub fn status_cache_name(account: u32) -> String {
    if account <= 1 { STATUS_CACHE_FILE.to_string() } else { format!("status-cache-{account}.json") }
}
/// The rules an opening starts from: a waiting `rules.change` (unless the call turns dials itself), else this
/// account's record's own, else its newest archived record's (an opening that failed after the archive), else
/// the network preset for a first opening. A reopen after a stop keeps the account's rules.
fn opening_dials(ctx: &Ctx, args: &Value) -> Result<Dials, String> {
    if let Some(d) = read_rules_next(&ctx.dir, ctx.account)?
        && !has_dial_args(args)
    {
        return Ok(d);
    }
    if ctx.try_load_vault()?.is_some() {
        return ctx.dials();
    }
    Ok(last_closed_dials(&ctx.dir, ctx.account).unwrap_or_else(|| ctx.dials.clone()))
}

/// The dials of account N's newest record in `closed/` (`<unix seconds>-state[-N].json`), if any reads.
fn last_closed_dials(dir: &Path, account: u32) -> Option<Dials> {
    let tail = format!("-{}", crate::tx::state_file_name(account));
    let newest = std::fs::read_dir(dir.join(crate::commands::CLOSED_DIR)).ok()?.flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(&tail)?.parse::<u64>().ok().map(|s| (s, n.clone())))
        .max()?;
    let text = std::fs::read_to_string(dir.join(crate::commands::CLOSED_DIR).join(newest.1)).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(v.get("dials")?.clone()).ok()
}

/// The dials a `rules.change` left for the next opening of account N (`rules-next-N.json`).
fn rules_next_name(account: u32) -> String {
    format!("rules-next-{account}.json")
}
fn read_rules_next(dir: &Path, account: u32) -> Result<Option<Dials>, String> {
    match std::fs::read_to_string(dir.join(rules_next_name(account))) {
        Ok(t) => serde_json::from_str(&t).map(Some).map_err(|e| format!("{}: {e}", rules_next_name(account))),
        Err(_) => Ok(None),
    }
}
fn write_rules_next(dir: &Path, account: u32, dials: &Dials) -> Result<(), String> {
    let text = serde_json::to_string_pretty(dials).map_err(|e| format!("{}: {e}", rules_next_name(account)))?;
    std::fs::write(dir.join(rules_next_name(account)), text).map_err(|e| format!("{}: {e}", rules_next_name(account)))
}
fn clear_rules_next(dir: &Path, account: u32) {
    let _ = std::fs::remove_file(dir.join(rules_next_name(account)));
}
/// The guardian and heir a `rules.change` left for the whole key set (docs/LINK_DESIGN.md): only the changed
/// fields of the rule block, `{"cancel2_x"?, "cancel3_x"?, "heir_x"?, "heir_address"?}`. The next `fund` applies
/// them (the heir.set rewrite) before it opens. Key-set wide, so it travels with keys.reset like the account files.
const PEOPLE_NEXT_FILE: &str = "people-next.json";
const PEOPLE_FIELDS: [&str; 4] = ["cancel2_x", "cancel3_x", "heir_x", "heir_address"];
/// Rule-block fields by name: the pending people changes, the shared rewrite's input.
type People = serde_json::Map<String, Value>;
pub(crate) fn read_people_next(dir: &Path) -> Result<Option<People>, String> {
    match std::fs::read_to_string(dir.join(PEOPLE_NEXT_FILE)) {
        Ok(t) => serde_json::from_str::<People>(&t)
            .map(|m| Some(m.into_iter().filter(|(k, v)| PEOPLE_FIELDS.contains(&k.as_str()) && v.is_string()).collect()))
            .map_err(|e| format!("{PEOPLE_NEXT_FILE}: {e}")),
        Err(_) => Ok(None),
    }
}
/// Merges `changes` into the waiting file (a later change of the same field wins) and answers the merged fields.
fn write_people_next(dir: &Path, changes: &People) -> Result<People, String> {
    let mut merged = read_people_next(dir)?.unwrap_or_default();
    merged.extend(changes.clone());
    let text = serde_json::to_string_pretty(&merged).map_err(|e| format!("{PEOPLE_NEXT_FILE}: {e}"))?;
    std::fs::write(dir.join(PEOPLE_NEXT_FILE), text).map_err(|e| format!("{PEOPLE_NEXT_FILE}: {e}"))?;
    Ok(merged)
}
fn clear_people_next(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(PEOPLE_NEXT_FILE));
}
const PEOPLE_OTHERS_OPEN: &str = "A guardian or heir covers every account. Stop your other accounts first.";
const PEOPLE_OWN_KEY: &str = "That link code belongs to this account itself. A guardian or heir makes their link code on their own phone.";
const GUARDIAN_NOT_A_KEY: &str = "That guardian's link code is not right. Read it again.";
const GUARDIAN_WHICH: &str = "Which guardian place? 2 or 3.";
const HEIR_NOT_A_KEY: &str = "That heir's link code is not right. Read it again.";
/// True when `has_dial_args` would turn any dial: fund then ignores a waiting rules-next file.
fn has_dial_args(args: &Value) -> bool {
    ["delay", "window", "cap_kas", "max_fee_kas", "min_remainder_kas", "heir_delay"].iter().any(|k| !args[*k].is_null())
}
/// The `account` argument every money method and status take: absent = 1.
fn account_of(args: &Value) -> Result<u32, &'static str> {
    match args.get("account") {
        None | Some(Value::Null) => Ok(1),
        Some(v) => v.as_u64().filter(|n| (1..=MAX_ACCOUNTS as u64).contains(n)).map(|n| n as u32).ok_or(BAD_ACCOUNT),
    }
}
/// A compile error that means this program cannot read the account's rules (a version it does not carry, a
/// changed rules file or rule block): the account is shown frozen in bank words instead of a panic.
fn rules_unreadable(e: &str) -> bool {
    e.starts_with(UNKNOWN_RULES) || e.starts_with(RULES_CHANGED) || e.starts_with(RULE_BLOCK_CHANGED) || e.contains("pinned to another version")
}
const BAD_ACCOUNT: &str = "Which account? 1 to 8.";
pub(crate) const OLD_RULES: &str = "This account was opened under an older version of the rules that this app no longer carries. Its funds stay where they are, but this app cannot move them: use the version of the app that opened it, or the vault seeds.";
const STEP_PAY_NOW: &str = "Pay the account's funds to your purse.";
const STEP_PAY_NOTICE: &str = "Pay the account's funds to your purse. It waits the notice period, like any large payment, and you can stop it.";
const STEP_FINISH: &str = "When the wait is over, finish the payment.";
const STEP_KEEP: &str = "Close the old account: the small amount it must always keep goes to your vault.";
const STEP_OPEN: &str = "Open the account again under the new rules, from your purse.";
const STEP_NOW_STOP: &str = "Stop the account: its funds go to your vault at once.";
const STEP_NOW_TAKEOUT: &str = "Take those funds back out of the vault with your fingerprint.";
const STEP_NOW_OPEN: &str = "Open the account again under the new rules.";
const NOW_NO_SAFE_KEYS: &str = "Your vault seeds are not kept on this phone, so the instant update is not offered. Use the update with notice.";
const NOW_NOT_STOPPED: &str = "Stop the account first.";
const HEIR_NO_LINK: &str = "This phone has no link yet, so nothing can be passed on to it.";
const NOW_ENDED_NOTHING_IN_SAFE: &str = "This account was emptied from another phone and none of it came to your vault (passed on to your heir, or the stop is still on its way). Open the account again from your purse.";
/// What "Update now" leaves for the reopening's fee: change under the floor folds into the deposit.
const NOW_OPEN_MARGIN: u64 = 1_000_000;

/// The "Update now" choice of `rules.change` on an open account: stop to the safe, take that coin out with the
/// phone's sealed safe keys, reopen. Offered only while the safe keys are sealed on this phone. Fees: the stop
/// (exact now), the take-out and the reopening (about the same each).
fn now_option(ctx: &Ctx, stop_fee: u64) -> Value {
    let on_phone = ctx.dir.join(SHEETS_ON_PHONE_FILE).exists();
    json!({
        "ok": on_phone,
        "why": if on_phone { Value::Null } else { json!(NOW_NO_SAFE_KEYS) },
        "steps": [STEP_NOW_STOP, STEP_NOW_TAKEOUT, STEP_NOW_OPEN],
        "fee_kas": kas(stop_fee),
        "fee_kas_about": kas(stop_fee * 3),
    })
}
const HEIR_NOT_A_PERSON: &str = "An heir must be a person's own address (the plain kind), not a vault or a covenant address.";

/// The service minus the transport: what the https routes and the phone's `call` both drive.
pub struct Core {
    pub(crate) ctx: Ctx,
    // Shared (Arc) so the short-lived armed copy a `call_with_secret` runs on (D3) answers out of the same
    // remembered sends, money lock and status cache as the long-lived Core.
    /// Answers to /api/send by the app's `id`, newest last.
    sends: Arc<Mutex<VecDeque<(String, Value)>>>,
    money: Arc<tokio::sync::Mutex<()>>,
    cache: Arc<Mutex<HashMap<u32, (Instant, Value)>>>,
    /// Set while the watcher file is unreadable, so the log says so once per breakage.
    alerts_warned: Arc<AtomicBool>,
}

/// Which secret a `call_with_secret` arms (D3): the sealed blobs the phone keeps, and the safe's own keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    /// The spend key: `send`.
    Card,
    /// Emergency stop key 1 and the funding key together, [stop 32][funding 32]: `stop` and `decline`,
    /// which need the funding key to carry a vault below the floor out (commands.rs lift_for).
    Stop,
    /// The funding key alone: `fund`, `purse.pay`.
    Funding,
    /// The spend key and the funding key together, [spend 32][funding 32]: `topup` (the contract's topup
    /// entry is signed by the spend key; the funding key pays the extra coin in, commands.rs do_topup).
    Topup,
    /// Two safe sheets as typed from paper, UTF-8 text: 24 words, a newline, 24 words: `takeout`. The
    /// block is text, not a key; `Armed::new` turns it into the two matching cold secrets and nothing else.
    Sheets,
    /// The safe's own secrets as the phone sealed them (keysNew mode "phone"), [cold 32][cold2 32] or all
    /// three: `takeout` without paper (the first two that match the rule block sign) and `keys.sheets` (the
    /// words of each, for the screen). An empty block means the sheets were burned from the phone.
    Safe,
    /// A helper's guardian key (docs/LINK_DESIGN.md), 32 bytes: `watch.stop`, `watch.decline` on an owner's account.
    Guardian,
    /// A helper's heir key, 32 bytes: `watch.claim`, `link.heir_words`, `heir.pay`. Never put into a Core's keys
    /// (`no_heir_key`); `link.heir_words` answers straight from the block.
    Heir,
}

impl Slot {
    /// The slot a money method signs with; None for a method that takes no secret.
    pub fn of_method(method: &str) -> Option<Slot> {
        match method {
            "send" => Some(Slot::Card),
            "stop" | "decline" => Some(Slot::Stop),
            "fund" | "purse.pay" => Some(Slot::Funding),
            "topup" => Some(Slot::Topup),
            "takeout" => Some(Slot::Sheets),
            "keys.sheets" => Some(Slot::Safe),
            "watch.stop" | "watch.decline" => Some(Slot::Guardian),
            "watch.claim" | "link.heir_words" | "heir.pay" => Some(Slot::Heir),
            _ => None,
        }
    }
    /// The slot by method and block length (the phone's callWithSecret): a `takeout` block of 0, 64 or 96
    /// bytes is the phone's own sealed safe keys; two typed sheets are at least ~190 bytes of text.
    pub fn of_method_block(method: &str, len: usize) -> Option<Slot> {
        match (method, len) {
            ("takeout", 0 | 64 | 96) => Some(Slot::Safe),
            _ => Slot::of_method(method),
        }
    }
    /// Whether this slot is one `method` signs with.
    pub fn serves(self, method: &str) -> bool {
        match self {
            Slot::Safe => matches!(method, "takeout" | "keys.sheets"),
            _ => Slot::of_method(method) == Some(self),
        }
    }
    /// How many bytes the slot's key block holds; 0 for the two whose length is not fixed (`accepts`). Not a
    /// container: there is no empty slot to ask about.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(self) -> usize {
        match self {
            Slot::Stop | Slot::Topup => 64,
            Slot::Sheets | Slot::Safe => 0,
            _ => 32,
        }
    }
    /// Whether a block of `n` bytes fits the slot.
    pub fn accepts(self, n: usize) -> bool {
        match self {
            Slot::Sheets => (1..=SHEETS_TEXT_MAX).contains(&n),
            Slot::Safe => matches!(n, 0 | 64 | 96),
            _ => n == self.len(),
        }
    }
    /// The slot's key block out of a full keys.json (the laptop's `call` check of the phone path), or the
    /// name of the key that is not there. The safe's sheets are never in keys.json on a networked device.
    pub fn block_from(self, keys: &Keys) -> Result<Zeroizing<Vec<u8>>, String> {
        let take = |s: &Option<[u8; 32]>, name: &str| s.as_ref().map(|s| Zeroizing::new(s.to_vec())).ok_or_else(|| format!("the {name} key is not on this device"));
        let mut block = Zeroizing::new(Vec::with_capacity(self.len()));
        match self {
            Slot::Card => block.extend_from_slice(&take(&keys.spend, "spend")?),
            Slot::Stop => {
                block.extend_from_slice(&take(&keys.cancel, "cancel")?);
                block.extend_from_slice(&take(&keys.funding, "funding")?);
            }
            Slot::Funding => block.extend_from_slice(&take(&keys.funding, "funding")?),
            Slot::Topup => {
                block.extend_from_slice(&take(&keys.spend, "spend")?);
                block.extend_from_slice(&take(&keys.funding, "funding")?);
            }
            Slot::Sheets | Slot::Safe => return Err("The vault seeds are never kept on this device; they are typed in from paper for each take-out.".into()),
            Slot::Guardian | Slot::Heir => return Err("the guardian and heir keys live only on the helper's phone".into()),
        }
        Ok(block)
    }
}

/// The redeem-script slot (0..3) of the cold sheet `secret` makes, by the rule block's `cold_x`.
pub(crate) fn cold_slot(p: &Public, secret: &[u8; 32]) -> Option<usize> {
    let x = try_xonly_hex(secret)?;
    p.cold_x.iter().position(|k| k.eq_ignore_ascii_case(&x))
}

/// Puts one cold secret into its slot of `keys`.
pub(crate) fn set_cold(keys: &mut Keys, slot: usize, secret: &[u8; 32]) {
    *match slot {
        0 => &mut keys.cold,
        1 => &mut keys.cold2,
        _ => &mut keys.cold3,
    } = Some(*secret);
}

/// The rule block a sheet is matched against, or the refusal (no keys yet; a one-key legacy safe has no sheets).
fn sheets_block(keys: &Keys) -> Result<Public, String> {
    let p = keys.public.clone().ok_or_else(|| NO_KEYS.to_string())?;
    if p.cold_x.len() != 3 {
        return Err(LEGACY_SAFE.into());
    }
    Ok(p)
}

/// `Slot::Sheets`: two sheets typed from paper (24 words, a newline, 24 words) into the two cold secrets at
/// their redeem-script slots. Refused in plain words naming the sheet (and the word position, from
/// `sheet_from_words`), never echoing a word. Each secret lives in a `Zeroizing` until it is placed.
fn arm_sheets(keys: &mut Keys, block: &[u8]) -> Result<(), String> {
    let p = sheets_block(keys)?;
    let text = std::str::from_utf8(block).map_err(|_| SHEETS_UNREADABLE.to_string())?;
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() != 2 {
        return Err(format!("Two vault seeds of 24 words are needed; {} were given.", lines.len()));
    }
    let mut placed = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let secret = Zeroizing::new(sheet_from_words(&words).map_err(|e| format!("Vault seed {}: {e}", i + 1))?);
        let slot = cold_slot(&p, &secret).ok_or_else(|| format!("Vault seed {}: {NOT_A_SAFE_SHEET}", i + 1))?;
        if placed.contains(&slot) {
            return Err(SAME_SHEET.into());
        }
        set_cold(keys, slot, &secret);
        placed.push(slot);
    }
    Ok(())
}

/// `Slot::Safe` for a take-out: the phone's sealed cold secrets, 32 bytes each; the first two that make one of
/// the rule block's three keys go into their slots. Empty = burned. Fewer than two matches = another account's keys.
fn arm_safe(keys: &mut Keys, block: &[u8]) -> Result<(), String> {
    if block.is_empty() {
        return Err(SHEETS_BURNED.into());
    }
    let p = sheets_block(keys)?;
    let mut placed = Vec::new();
    for chunk in block.chunks_exact(32) {
        let secret = Zeroizing::new(<[u8; 32]>::try_from(chunk).expect("32 bytes"));
        if let Some(slot) = cold_slot(&p, &secret)
            && !placed.contains(&slot) {
                set_cold(keys, slot, &secret);
                placed.push(slot);
            }
        if placed.len() == 2 {
            return Ok(());
        }
    }
    Err(SAFE_KEYS_MISMATCH.into())
}

/// `keys.sheets`: the phone's sealed cold secrets as 24 words each, for the screen. The secrets are wiped
/// here; honest limit: the words go out in the answer, which the caller shows and drops.
fn sheets_answer(block: &[u8]) -> Value {
    if block.is_empty() {
        return refusal(SHEETS_BURNED);
    }
    let mut sheets: Vec<[String; 24]> = block
        .chunks_exact(32)
        .map(|c| {
            let secret = Zeroizing::new(<[u8; 32]>::try_from(c).expect("32 bytes"));
            sheet_words(&secret)
        })
        .collect();
    let answer = json!({"ok": true, "sheets": sheets});
    for w in sheets.iter_mut().flatten() {
        w.zeroize();
    }
    answer
}

/// A `Core` holding a secret for one call: the shared Core's `ctx` cloned with the slot's key(s) set, over
/// the same sends, money lock and cache. Dropping it wipes the keys, on the answer path and on a panic's
/// unwind alike. Honest limit: `secp256k1::Keypair` copies made on the stack while signing are not scrubbed.
struct Armed {
    core: Core,
}

const SEALED_UNREADABLE: &str =
    "This phone's sealed records could not be read. No funds were moved.";

impl Armed {
    /// Err = the block did not arm (a sheet refused in plain words); the copy is wiped before the answer.
    fn new(shared: &Core, slot: Slot, block: &[u8]) -> Result<Armed, String> {
        // A sealed block whose word is no secp256k1 secret never reaches a signing path.
        let sealed = matches!(slot, Slot::Card | Slot::Stop | Slot::Funding | Slot::Topup);
        if sealed && !block.chunks(32).all(|c| secp256k1::SecretKey::from_slice(c).is_ok()) {
            return Err(SEALED_UNREADABLE.into());
        }
        let mut ctx = shared.ctx.clone();
        let word = |i: usize| {
            let mut k = [0u8; 32];
            k.copy_from_slice(&block[i * 32..(i + 1) * 32]);
            Some(k)
        };
        let armed = match slot {
            Slot::Card => {
                let _: () = ctx.keys.spend = word(0);
                Ok(())
            },
            Slot::Stop => {
                ctx.keys.cancel = word(0);
                let _: () = ctx.keys.funding = word(1);
                Ok(())
            }
            Slot::Funding => {
                let _: () = ctx.keys.funding = word(0);
                Ok(())
            },
            Slot::Topup => {
                ctx.keys.spend = word(0);
                let _: () = ctx.keys.funding = word(1);
                Ok(())
            }
            Slot::Sheets => arm_sheets(&mut ctx.keys, block),
            Slot::Safe => arm_safe(&mut ctx.keys, block),
            // Never armed into a Core (call_with_secret answers or refuses them first).
            Slot::Guardian | Slot::Heir => Err("Not found".into()),
        };
        if let Err(e) = armed {
            ctx.keys.wipe();
            return Err(e);
        }
        Ok(Armed { core: Core { ctx, sends: shared.sends.clone(), money: shared.money.clone(), cache: shared.cache.clone(), alerts_warned: shared.alerts_warned.clone() } })
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        self.core.ctx.keys.wipe();
    }
}

fn pick_spend(k: &Keys) -> &Option<[u8; 32]> {
    &k.spend
}
fn pick_stop(k: &Keys) -> &Option<[u8; 32]> {
    &k.cancel
}

// Keys: what the service may hold

pub(crate) fn no_safe_sheet(keys: &Keys) -> Result<(), String> {
    if keys.cold.is_some() || keys.cold2.is_some() || keys.cold3.is_some() { Err(SAFE_SHEETS_HERE.into()) } else { Ok(()) }
}

/// The heir's secret sweeps the whole vault after one heir-delay of silence and no stop key can
/// cancel that path, so it may not sit on the networked laptop at all.
pub(crate) fn no_heir_key(keys: &Keys) -> Result<(), String> {
    if keys.heir.is_some() { Err(HEIR_KEY_HERE.into()) } else { Ok(()) }
}

/// A lock that survives a panic on another request: the data behind these mutexes (pairing, cache,
/// sends, refusals, slots) stays usable after a poisoned guard, so one bad request never turns every
/// later one into a 500.
pub(crate) fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An amount from the phone as exact sompi: a decimal string ("0.25") or a JSON number, at most
/// 8 decimals, no sign or exponent, from one sompi to a billion KAS. A string is read digit by digit
/// and is exact; a JSON number has already been through serde_json's f64 (the web app sends a JS
/// Number), so it is exact for any amount under about 90 million KAS and rounded, not refused, above
/// that many significant digits. The digit check still rejects signs, exponents and a 9th decimal.
fn parse_kas(v: &Value) -> Option<u64> {
    let text = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    if (whole.is_empty() && frac.is_empty()) || frac.len() > 8 || !whole.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let frac: u64 = if frac.is_empty() { 0 } else { format!("{frac:0<8}").parse().ok()? };
    let sompi = whole.checked_mul(KAS)?.checked_add(frac)?;
    (1..=1_000_000_000 * KAS).contains(&sompi).then_some(sompi)
}

/// Adds keys nobody holds to `unheld.json` {since_setup, keys} (sorted x-only hex; never shrinks). `at_setup`: the
/// phone made the key set just now, so the list is complete; a list started later (a removal on an older key set)
/// is not, and the slots it does not name stay `unknown`. Nothing to add and not at setup = no file written.
pub(crate) fn record_unheld(dir: &Path, xs: &[String], at_setup: bool) -> Result<(), String> {
    let (mut keys, since_setup) = read_unheld(dir);
    let before = keys.len();
    keys.extend(xs.iter().filter(|x| !x.is_empty()).cloned());
    keys.sort();
    keys.dedup();
    if keys.len() == before && !at_setup {
        return Ok(());
    }
    let mut file = crate::paper::unheld_object(dir);
    file["since_setup"] = json!(since_setup || at_setup);
    file["keys"] = json!(keys);
    crate::paper::write_atomic(&dir.join(UNHELD_FILE), &file.to_string())
}
/// (`keys`, `since_setup`) of `unheld.json`; no file or an unreadable one = (none, false).
pub(crate) fn read_unheld(dir: &Path) -> (Vec<String>, bool) {
    let v: Value = std::fs::read_to_string(dir.join(UNHELD_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let keys = v["keys"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    (keys, v["since_setup"] == true)
}
/// Who holds each stop slot, for People: `you` (stop 1, or a slot repeating it), `unheld` (a key nobody holds),
/// `guardian` (any other key, when the list was kept since setup), `unknown` (a key set made before the list).
pub(crate) fn stop_holders(p: &Public, (unheld, since_setup): &(Vec<String>, bool)) -> Value {
    let holder = |x: &str| {
        if x.is_empty() || x == p.cancel_x {
            "you"
        } else if unheld.iter().any(|h| h == x) {
            "unheld"
        } else if *since_setup {
            "guardian"
        } else {
            "unknown"
        }
    };
    json!([{"which": 1, "holder": "you"}, {"which": 2, "holder": holder(&p.cancel2_x)}, {"which": 3, "holder": holder(&p.cancel3_x)}])
}

pub(crate) fn refusal(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

/// The refusal of a phone that has no keys yet, with a flag the page reads instead of matching words.
pub(crate) fn no_keys() -> Value {
    json!({"ok": false, "no_keys": true, "error": NO_KEYS})
}

// Wording and small rules

/// Bank words for the phone. Text from the node, the RPC link or the laptop's files is replaced
/// by the fixed words of its class and the raw text goes to the laptop's log only; the service's
/// own wording (the constants above, the amount rules) passes through unchanged.
pub(crate) fn plain(e: &str) -> String {
    let words = if e.contains(crate::tx::ANSWER_LOST_MARK) {
        ANSWER_LOST
    } else if e.contains("sequence locks") {
        "Too early: the notice period has not passed yet."
    } else if e.contains("is the last transaction accepted yet") {
        "The last transaction hasn't landed yet. Try again in a few seconds."
    } else if e.contains("rejected by node") || e.starts_with("preflight") || e.contains("covenant context") || e.contains("genesis covenant") {
        NETWORK_REFUSED
    } else if e.contains("Can't reach the network") || e.starts_with("get_utxos_by_addresses") || e.starts_with("get_block_dag_info") || e.contains("RPC") || e.contains("rpc") || e.contains("WebSocket") || e.contains("websocket") || e.contains("connect") {
        NETWORK_DOWN
    } else if e.contains("state.json") || e.contains("keys.json") || e.contains("os error") {
        RECORDS_UNREADABLE
    } else {
        return e.to_string();
    };
    log_full(e);
    words.into()
}

/// The raw text behind a plain refusal, for whoever reads the logs: stdout on the laptop, logcat on the phone
/// (tag KaspaVault, warning). Error texts carry file names and addresses, never a secret.
pub(crate) fn log_full(e: &str) {
    #[cfg(target_os = "android")]
    {
        #[link(name = "log")]
        unsafe extern "C" {
            fn __android_log_write(prio: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> i32;
        }
        if let Ok(text) = std::ffi::CString::new(format!("engine in full: {e}")) {
            // SAFETY: both pointers are NUL-terminated C strings that outlive the call; liblog copies them.
            unsafe {
                __android_log_write(5, c"KaspaVault".as_ptr(), text.as_ptr());
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    println!("  in full: {e}");
}

/// A money answer: the transaction that went out and the fee it paid (`fee_kas`, from the history line).
fn result(answer: Result<(String, Option<f64>), String>) -> Value {
    match answer {
        Ok((tx, fee)) => json!({"ok": true, "tx": tx, "fee_kas": fee, "error": null}),
        Err(e) => json!({"ok": false, "tx": null, "fee_kas": null, "error": plain(&e)}),
    }
}

fn send_result(answer: Result<(String, &str, Option<f64>), String>) -> Value {
    match answer {
        Ok((tx, kind, fee)) => json!({"ok": true, "tx": tx, "kind": kind, "fee_kas": fee, "error": null}),
        Err(e) => json!({"ok": false, "tx": null, "kind": null, "fee_kas": null, "error": plain(&e)}),
    }
}

/// A quote: what the move would pay and move, nothing sent. Refusals are the client's own words.
fn quote_result(answer: Result<(u64, &str, u64), String>) -> Value {
    match answer {
        Ok((fee, kind, total)) => json!({"ok": true, "kind": kind, "fee_kas": kas(fee), "fee_sompi": fee, "total_kas": kas(total), "error": null}),
        Err(e) => json!({"ok": false, "kind": null, "fee_kas": null, "fee_sompi": null, "total_kas": null, "error": plain(&e)}),
    }
}

/// Most coin amounts `coins.info` lists; its count and total still cover every coin.
const COINS_LISTED: usize = 200;
/// Said when this phone has no purse to read yet (words the page's plain() filter lets through).
const COINS_NO_PURSE: &str = "The purse isn't set up on this phone yet.";

/// The `coins.info` answer for the purse's coins: how many, their total, the amounts largest
/// first (at most COINS_LISTED) and what combining them all into one coin would cost (0 for an
/// empty purse, null when the network would not take that combination, e.g. too little to pay
/// its own fee).
fn coins_answer(ctx: &Ctx, coins: &[Coin]) -> Value {
    let total: u64 = coins.iter().map(|(_, u)| u.amount).sum();
    let mut amounts: Vec<u64> = coins.iter().map(|(_, u)| u.amount).collect();
    amounts.sort_unstable_by(|a, b| b.cmp(a));
    amounts.truncate(COINS_LISTED);
    let fee = if coins.is_empty() { Some(0) } else { combine_fee(ctx, coins, total) };
    json!({"ok": true, "count": coins.len(), "total_sompi": total, "amounts_sompi": amounts,
        "fee_sompi": fee, "error": null})
}

/// The fee of an N-in, 1-out spend of every purse coin back to the purse, from the client's one
/// fee loop (`quote`, no ceiling = the plain-send ceiling, as purse.pay sends). Priced without the
/// purse's secret: the coins are re-addressed to a throwaway key's script and signed with it; an
/// x-only key and a Schnorr signature are fixed-size, so the mass and the fee are the real spend's.
fn combine_fee(ctx: &Ctx, coins: &[Coin], total: u64) -> Option<u64> {
    let k = keypair(&[7u8; 32]);
    let spk = kaspa_txscript::pay_to_address_script(&p2pk_address(ctx.network.prefix, &k));
    let entry = |u: &UtxoEntry| {
        UtxoEntry::new(u.amount, spk.clone(), u.block_daa_score, u.is_coinbase, None)
    };
    let moved = |(op, u): &Coin| (*op, entry(u));
    let sample: Vec<Coin> = coins.iter().map(moved).collect();
    let entries: Vec<UtxoEntry> = sample.iter().map(|(_, u)| u.clone()).collect();
    let quoted = ctx.quote(&entries, None, &|fee| {
        let left = total.checked_sub(fee).filter(|l| *l > 0);
        let left = left.ok_or_else(|| "too little to combine".to_string())?;
        ctx.plain_tx(&sample, vec![TransactionOutput::new(left, spk.clone())], &k, false)
    });
    quoted.ok().map(|q| q.fee)
}

/// The fee a command's history line records ("..., fee 0.00250 KAS, ...").
fn fee_of(line: &str) -> Option<f64> {
    let (_, rest) = line.split_once("fee ")?;
    rest.split_whitespace().next()?.parse().ok()
}

/// `a=1&b=2` into pairs, percent-decoded. The query never reaches the log.
#[cfg(feature = "laptop")]
pub(crate) fn query_pairs(query: &str) -> HashMap<String, String> {
    query.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), percent_decode(v))).collect()
}

#[cfg(feature = "laptop")]
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len()).then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `<dir>/alerts.json` as the watcher wrote it; Ok(None) when there is no watcher file.
fn read_alerts(dir: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(dir.join(ALERTS_FILE)) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if v.is_object() => Ok(Some(v)),
            Ok(_) => Err(format!("{ALERTS_FILE}: not a JSON object")),
            Err(e) => Err(format!("{ALERTS_FILE}: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{ALERTS_FILE}: {e}")),
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ---- The guard inside the app (D5) and the second keyless guard (D6) ----

/// The running guard (`watch::run_guard` on this process's runtime): one per process, kept in a static so
/// `guard.stop`, `guard.status`, `guard.wait` and `guard.poll` find it from any call.
pub struct GuardHandle {
    task: tokio::task::JoinHandle<()>,
    stop: tokio::sync::watch::Sender<bool>,
    woke: Arc<tokio::sync::Notify>,
    /// Seconds between ticks, read by the guard every tick (`guard.poll`).
    poll: Arc<AtomicU64>,
    started: u64,
    /// Why the guard ended, once it has: "finished" (the vault closed or it was stopped) or its refusal.
    exited: Arc<Mutex<Option<String>>>,
}

static GUARD: OnceLock<Mutex<Option<GuardHandle>>> = OnceLock::new();

fn guard_slot() -> &'static Mutex<Option<GuardHandle>> {
    GUARD.get_or_init(|| Mutex::new(None))
}

/// A follow-only guard on one watched account's folder: the task and its stop (kept, since a dropped stop ends it).
type Follower = (tokio::task::JoinHandle<()>, tokio::sync::watch::Sender<bool>);

/// Watched-follow guards (watched.rs): one follow-only `watch::run_guard` per watched folder, keyed by the folder
/// path, beside the wallet's own guard slot, which they never touch. Process-global, so an engine reopened in the
/// same process finds the ones already running instead of starting a second.
static FOLLOWERS: OnceLock<Mutex<HashMap<std::path::PathBuf, Follower>>> = OnceLock::new();
/// Seconds between a follow-only guard's ticks.
const FOLLOW_POLL_SECS: u64 = 5;

fn followers() -> &'static Mutex<HashMap<std::path::PathBuf, Follower>> {
    FOLLOWERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Starts the follow-only guard on `ctx` (a watched folder's Ctx, on the wallet's own node connection; the
/// two-node rule of `guard.start` is not asked: alarms come from the phone polling `watch.status`). A folder that
/// already has one, running or ended, is left alone unless `restart` (a fresh import), which replaces it.
pub(crate) fn follow_start(ctx: Ctx, restart: bool) -> bool {
    let key = ctx.dir.clone();
    let mut map = guard(followers());
    if let Some((task, _)) = map.get(&key) {
        if !restart {
            return false;
        }
        task.abort();
    }
    let (stop, rx) = tokio::sync::watch::channel(false);
    let poll = Arc::new(AtomicU64::new(FOLLOW_POLL_SECS));
    let task = tokio::spawn(async move {
        let dir = ctx.dir.clone();
        if let Err(e) = crate::watch::run_guard(ctx, vec![], false, poll, rx, Arc::new(tokio::sync::Notify::new())).await {
            println!("the follow-only guard on {} ended: {e}", dir.display());
        }
    });
    map.insert(key, (task, stop));
    true
}

/// Ends the follow-only guard on a watched folder, if it has one (`watch.forget`, before the folder goes).
pub(crate) fn follow_stop(dir: &Path) {
    if let Some((task, _)) = guard(followers()).remove(dir) {
        task.abort();
    }
}

/// What `guard.export` / `guard-export` write in the vault folder: everything a second, keyless guard needs.
pub const GUARD_FILE: &str = "guard-public.json";
pub const GUARD_KIND: &str = "guard-public";
const GUARD_RUNNING: &str = "The guard is already running.";
const GUARD_NOT_RUNNING: &str = "The guard is not running.";
const FOLDER_HAS_KEYS: &str = "This folder already holds an account. A watch-only copy goes in a folder of its own.";
pub(crate) const NOT_GUARD_FILE: &str = "That is not a watch code this account can read.";
/// The longest `guard.wait` may hold a call.
const GUARD_WAIT_MAX_MS: u64 = 600_000;

/// `guard.export` (D6): `<dir>/guard-public.json` = the public block and its code, the vault record without its
/// history and pending destination, the rules hash and the node hint. No secret is read: the public block comes
/// from disk through `Keys::load_public`. `state_file` replaces `<dir>/state.json` (the laptop's `--state`).
/// `owner_name` (guard.export arg `name`) rides along as `owner_name` for the helper's list (watch.import).
pub fn guard_export(dir: &Path, network: &Network, state_file: Option<&Path>, nodes: &[String], owner_name: Option<&str>) -> Result<Value, String> {
    let out = guard_build(dir, network, state_file, nodes, owner_name)?;
    let path = dir.join(GUARD_FILE);
    let text = serde_json::to_string_pretty(&out).map_err(|e| format!("{GUARD_FILE}: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(out)
}

/// `guard_export`'s file content, built without writing anything (watch.code).
pub fn guard_build(
    dir: &Path,
    network: &Network,
    state_file: Option<&Path>,
    nodes: &[String],
    owner_name: Option<&str>,
) -> Result<Value, String> {
    let keys = Keys::load_public(&dir.join("keys.json"))?;
    let public = keys.public();
    let public_sha256 = keys.public_sha256.clone().unwrap_or_else(|| public.sha256());
    let state_path = state_file.map(Path::to_path_buf).unwrap_or_else(|| dir.join("state.json"));
    let text = std::fs::read_to_string(&state_path).map_err(|e| format!("{}: {e}", state_path.display()))?;
    let v: Vault = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", state_path.display()))?;
    network.check_recorded(&v.network, "state.json")?;
    // A vault from before the pin runs on the folder's vault.sil, else on the rules built in here.
    let contract_sha256 = v
        .contract_sha256
        .clone()
        .or_else(|| std::fs::read(dir.join(SOURCE_FILE)).ok().map(|b| sha256_hex(&b)))
        .unwrap_or_else(|| embedded_sha256().to_string());
    let mut vault = serde_json::to_value(&v).map_err(|e| format!("state.json: {e}"))?;
    if let Some(o) = vault.as_object_mut() {
        o.remove("history");
        o.remove("pending_to");
    }
    let mut out = json!({"kind": GUARD_KIND, "v": 1, "network": network.name, "public": public, "public_sha256": public_sha256, "vault": vault, "contract_sha256": contract_sha256, "nodes": nodes});
    if let Some(name) = owner_name.map(str::trim).filter(|n| !n.is_empty()) {
        out["owner_name"] = json!(name);
    }
    Ok(out)
}

/// `guard.import` / `guard-import` (D6): makes `dir` a watch-only folder from the text `guard_export` wrote: a
/// public-only keys.json, state.json (anchored to the rule block code) and the pinned vault.sil restored from the
/// rules built into this program by `contract_sha256`. Refused into a folder whose keys.json holds any secret:
/// a second guard never shares the wallet's own folder.
pub fn guard_import(dir: &Path, network: &Network, text: &str) -> Result<Value, String> {
    let file: Value = serde_json::from_str(text.trim()).map_err(|_| NOT_GUARD_FILE.to_string())?;
    if file["kind"] != GUARD_KIND {
        return Err(NOT_GUARD_FILE.into());
    }
    network.check_recorded(file["network"].as_str().unwrap_or(""), GUARD_FILE)?;
    let public: Public = serde_json::from_value(file["public"].clone()).map_err(|_| NOT_GUARD_FILE.to_string())?;
    let public_sha256 = file["public_sha256"].as_str().ok_or_else(|| NOT_GUARD_FILE.to_string())?.to_string();
    if public.sha256() != public_sha256 {
        return Err("The covenant in this file does not match the code written in it. Export it again from the account.".into());
    }
    // Anyone can recompute public_sha256: the block itself must hold real keys and addresses.
    public.check_foreign(network).map_err(|_| NOT_GUARD_FILE.to_string())?;
    let sha = file["contract_sha256"].as_str().ok_or_else(|| NOT_GUARD_FILE.to_string())?.to_string();
    let mut vault = file["vault"].clone();
    let o = vault.as_object_mut().ok_or_else(|| NOT_GUARD_FILE.to_string())?;
    o.entry("history").or_insert_with(|| json!([]));
    o.entry("pending_to").or_insert(Value::Null);
    o.insert("contract_sha256".into(), json!(sha));
    o.insert("network".into(), json!(network.name));
    let v: Vault = serde_json::from_value(vault).map_err(|_| NOT_GUARD_FILE.to_string())?;
    let keys_path = dir.join("keys.json");
    if keys_path.exists() {
        if Keys::is_locked(&keys_path) {
            return Err(FOLDER_HAS_KEYS.into());
        }
        let here = std::fs::read_to_string(&keys_path).map_err(|e| format!("{}: {e}", keys_path.display()))?;
        let here: Keys = serde_json::from_str(&here).map_err(|_| format!("{} is not a keys.json this program can read.", keys_path.display()))?;
        if here.has_secret() {
            return Err(FOLDER_HAS_KEYS.into());
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    restore_source(dir, &sha)?;
    let keys = Keys { public: Some(public), public_sha256: Some(public_sha256.clone()), ..Default::default() };
    keys.save(&keys_path, true)?;
    save_vault_anchored(&dir.join("state.json"), &v, &public_sha256)?;
    let ctx = Ctx { dir: dir.to_path_buf(), keys, rpc: None, force: false, network: network.clone(), dials: v.dials.clone(), account: 1 };
    let address = ctx.try_vault_address(&v.state)?.to_string();
    Ok(json!({"ok": true, "address": address, "covenant_id": v.covenant_id.to_string(), "contract_sha256": sha}))
}

/// `guard.status`: the handle's view (running, why it ended, when it started) and alerts.json as the guard
/// wrote it (`seen_at` null and no alerts before the first good answer or without a file).
pub fn guard_status(dir: &Path) -> Value {
    let (running, exited, started) = match guard(guard_slot()).as_ref() {
        Some(h) => (!h.task.is_finished(), guard(&h.exited).clone(), Some(h.started)),
        None => (false, None, None),
    };
    let file = read_alerts(dir).ok().flatten();
    let seen_at = file.as_ref().map_or(Value::Null, |f| f["seen_at"].clone());
    let alerts = file.as_ref().and_then(|f| f["alerts"].as_array().cloned()).unwrap_or_default();
    json!({"ok": true, "running": running, "exited": exited, "seen_at": seen_at, "alerts": alerts, "started": started})
}

/// `guard.stop {}`: pulls the stop, waits a moment for the task, and lets the handle go.
async fn guard_stop() -> Result<Value, String> {
    let taken = guard(guard_slot()).take();
    let Some(h) = taken else { return Ok(json!({"ok": true, "stopped": false})) };
    let _ = h.stop.send(true);
    let stopped = tokio::time::timeout(Duration::from_secs(5), h.task).await.is_ok();
    Ok(json!({"ok": true, "stopped": stopped}))
}

/// `guard.wait {timeout_ms}`: holds the call until the guard saves an alert or gets a good answer (or the
/// timeout), then answers `guard.status`. With no guard running it answers at once.
async fn guard_wait(dir: &Path, timeout: Duration) -> Value {
    let woke = guard(guard_slot()).as_ref().map(|h| h.woke.clone());
    if let Some(w) = woke {
        let _ = tokio::time::timeout(timeout, w.notified()).await;
    }
    guard_status(dir)
}

/// `guard.poll {seconds}`: the seconds between ticks, from the next tick on.
fn guard_poll(seconds: Option<u64>) -> Result<Value, String> {
    let secs = seconds.ok_or_else(|| "guard.poll needs {\"seconds\": <n>}".to_string())?.clamp(1, 3600);
    match guard(guard_slot()).as_ref() {
        Some(h) => {
            h.poll.store(secs, Ordering::Relaxed);
            Ok(json!({"ok": true, "poll_seconds": secs}))
        }
        None => Err(GUARD_NOT_RUNNING.into()),
    }
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

/// What the watcher adds to a status: `watcher_seen_at`, its newest alerts verbatim, and its view of
/// `pending`. state.json only knows what this laptop signed; the watcher reads the chain. So a
/// withdrawal started elsewhere (a thief holding the spend key; review 8, High 1) exists only in
/// alerts.json: while the guard is fresh, a `withdrawal_pending` newer than any completed/stopped
/// alert shows as pending even when state.json says nothing waits, and a `withdrawal_completed`,
/// `withdrawal_declined` (v5) or `stopped` newer than that clears a pending state.json still holds. A quiet
/// guard (heartbeat older than WATCHER_FRESH, or none) changes nothing: the phone shows its banner.
fn with_watcher(mut status: Value, file: &Value, now: u64) -> Value {
    let mut alerts: Vec<Value> = file["alerts"].as_array().cloned().unwrap_or_default();
    alerts.sort_by_key(|a| std::cmp::Reverse(a["at"].as_u64().unwrap_or(0)));
    alerts.truncate(ALERTS_SHOWN);
    let fresh = file["seen_at"].as_u64().is_some_and(|t| now.saturating_sub(t) <= WATCHER_FRESH);
    let stage = alerts.iter().find(|a| matches!(a["kind"].as_str(), Some("withdrawal_pending" | "withdrawal_completed" | "withdrawal_declined" | "stopped")));
    if let (true, Some(a)) = (fresh, stage) {
        match a["kind"].as_str() {
            Some("withdrawal_pending") if status["pending"].is_null() => {
                status["pending"] = json!({"to": null, "amount_kas": null, "seconds_left": null, "tx": null, "watcher": true, "text": a["text"]});
            }
            Some("withdrawal_completed" | "withdrawal_declined" | "stopped") => status["pending"] = Value::Null,
            _ => {}
        }
    }
    status["watcher_seen_at"] = file["seen_at"].clone();
    status["alerts"] = Value::Array(alerts);
    status
}

fn tx_of(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once(" tx ")?;
    rest.split([' ', ';']).next().filter(|id| !id.is_empty())
}

/// What a history line was, for Statements: paid (left to someone), received (top-up),
/// stopped (cancel, decline, an heir claim, ended elsewhere), rules (Update now),
/// fund (account opened), other (a request).
fn history_kind(h: &str) -> &'static str {
    let first = h.split([' ', ':']).next().unwrap_or("");
    match first {
        "spend" | "finish" => "paid",
        "topup" => "received",
        "cancel" | "decline" | "inherit" | "inheritOpen" => "stopped",
        "fund" => "fund",
        _ if h == crate::commands::ENDED_ELSEWHERE => "stopped",
        _ if h == "Rules updated" => "rules",
        _ => "other",
    }
}

/// The KAS amount a line names (the number after its first word: "spend 5 KAS",
/// "finish 4.9 KAS", "topup 3", "fund 10 KAS"); None for lines without one (stops, rules).
fn history_kas(h: &str, kind: &str) -> Option<f64> {
    if !matches!(kind, "paid" | "received" | "fund") {
        return None;
    }
    let n = h.split(' ').nth(1)?.parse::<f64>().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n)
}

fn history_json(v: &Vault, state: &Path) -> Vec<Value> {
    let times = history_times(state);
    v.history
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let mut item = match tx_of(h) {
                Some(tx) => {
                    let text = h.replacen(&format!(" tx {tx}"), "", 1);
                    json!({"text": text.trim_end_matches([',', ':']), "tx": tx})
                }
                None => json!({"text": h, "tx": null}),
            };
            let kind = history_kind(h);
            item["kind"] = json!(kind);
            if let Some(k) = history_kas(h, kind) {
                item["kas"] = json!(k);
            }
            if let Some((at, _)) = times.get(i).filter(|(at, t)| *at > 0 && t == h) {
                item["at"] = json!(at);
            }
            item
        })
        .collect()
}

fn seconds(net: &Network, daa: i64) -> i64 {
    let rate = net.daa_per_sec.round().max(1.0) as i64;
    (daa.max(0) + rate - 1) / rate
}

/// "Protected by" pips for the phone: distinct stop keys configured, whether an heir is set,
/// and how many safe sheets guard the cold address (3 for the 2-of-3 safe, else 1).
/// (emergency stop buttons someone holds, heir set, vault seeds). A button counts when this phone holds it (slot 1)
/// or a guardian does (`stop_holders`); a key nobody holds, or one whose holder is not known (a key set made before
/// the unheld list), is not a button.
fn protection(p: &Public, unheld: &(Vec<String>, bool), paper: &[String]) -> (usize, bool, usize) {
    let holders = stop_holders(p, unheld);
    let mut held: Vec<&str> = vec![p.cancel_x.as_str()];
    for (i, x) in [(1usize, p.cancel2_x.as_str()), (2, p.cancel3_x.as_str())] {
        let paper_stop = paper.iter().any(|k| k == x); // a paper stop is a button the owner holds
        if (holders[i]["holder"] == "guardian" || paper_stop) && !held.contains(&x) {
            held.push(x);
        }
    }
    (held.len(), !p.heir_x.is_empty(), if p.cold_x.len() == 3 { 3 } else { 1 })
}

fn stopped_status(net: &Network, dials: &Dials, p: &Public, unheld: &(Vec<String>, bool), paper: &[String], safe: u64, history: Vec<Value>) -> Value {
    let (stop_keys, heir, sheets) = protection(p, unheld, paper);
    json!({
        "network": net.name,
        "balance_kas": 0.0,
        "limit_kas": kas(dials.cap as u64),
        "spent_kas": 0.0,
        "left_kas": 0.0,
        "refresh_seconds": 0,
        "notice_seconds": seconds(net, dials.delay),
        "pending": null,
        "stopped": true,
        "safe_kas": kas(safe),
        "stop_keys": stop_keys,
        "heir": heir,
        "sheets": sheets,
        "hard_limit": p.hard_limit,
        "heir_seconds_left": null,
        "heir_delay_seconds": null,
        "heir_warn_share": HEIR_WARN_SHARE,
        "history": history,
    })
}

impl Core {
    /// The key checks the service makes before it answers anything: no safe sheet and no heir key on
    /// this device. The caller has already put the keys it wants held into `ctx.keys` (serve.rs merges
    /// the first stop key from the offline folder first).
    pub fn open(ctx: Ctx) -> Result<Core, String> {
        no_safe_sheet(&ctx.keys)?;
        no_heir_key(&ctx.keys)?;
        Ok(Core { ctx, sends: Arc::new(Mutex::new(VecDeque::new())), money: Arc::new(tokio::sync::Mutex::new(())), cache: Arc::new(Mutex::new(HashMap::new())), alerts_warned: Arc::new(AtomicBool::new(false)) })
    }

    /// One entry for every caller: the https route (serve.rs) and the phone's direct call answer
    /// with the same (status code, body). `args` is the request's JSON object (`{}` when the method
    /// takes none); anything else is a 400. `keys.public`, `keys.burned`, `link.*`, `guard.*` and
    /// `watch.*` answer before the gates (an unknown `guard.`/`watch.` name is a 404 offline). Every
    /// other name meets the gates first, known or not: before `keys.new` has run (no rule block) it
    /// gets the no-keys answer, then the node must be reachable (503 with the network-down words, as
    /// the route always answered); only past both is a name that is not known a 404.
    pub async fn call(&self, method: &str, args: &Value) -> (u16, Value) {
        if !args.is_object() {
            return (400, refusal("Bad request"));
        }
        if method == "keys.public" {
            return (200, self.keys_public());
        }
        // The guard (D5/D6) answers offline, before the key and network gates: guard.import runs on a folder
        // with no keys yet, and the others read their public block from disk, not from this Core.
        if method.starts_with("guard.") {
            return self.guard_call(method, args).await;
        }
        // The burn marker: a file in the folder, no keys and no network needed.
        if method == "keys.burned" {
            return (200, self.keys_burned());
        }
        // The helper's link (docs/LINK_DESIGN.md): link.json and the code's own text, no keys and no network.
        if method == "link.code" || method == "link.read" {
            let answer = if method == "link.code" {
                crate::link::link_code(&self.ctx.dir, &self.ctx.network)
            } else {
                let text = args["text"].as_str().unwrap_or("");
                crate::link::read_link(&self.ctx.network, text, self.ctx.keys.public.as_ref())
                    .and_then(|l| crate::link::not_this_phone(&self.ctx.dir, &l)
                        .map(|_| l.answer()))
            };
            return (200, answer.unwrap_or_else(|e| refusal(&e)));
        }
        // Accounts this phone watches for others (watched.rs): they live in their own folders, so they need no
        // key set here; watch.status checks the network itself.
        if method.starts_with("watch.") {
            return crate::watched::call(self, method, args).await;
        }
        // The heir's purse (what claims paid this phone's link): a helper phone may have no key set of its own.
        if method == "heir.purse" {
            if let Err(e) = self.connected().await {
                return (200, json!({"ok": false, "address": null, "kas": null, "error": plain(&e)}));
            }
            return (200, self.heir_purse().await);
        }
        // The paper stop (paper.rs): check and stop work from a watch code and typed words, so a
        // new phone with no key set (after a loss) can use them; they never touch its key set.
        if method == "paper.check" || method == "paper.stop" || method == "paper.takeout" {
            return crate::paper::call_keyless(self, method, args).await;
        }
        // Public supply figures (market.rs): no keys, no node; the public service, cached.
        if method == "market.supply" {
            return (200, crate::market::supply(&self.ctx.network).await);
        }
        self.gated(method, args).await
    }

    /// `call` past the methods that answer before the gates: the no-keys answer, then the node
    /// gate, then the method on the account `account` names. `paper.sheet` answers between the
    /// two gates (offline).
    async fn gated(&self, method: &str, args: &Value) -> (u16, Value) {
        if self.ctx.keys.public.is_none() {
            return (200, if method == "status" { self.no_keys_status().await } else { no_keys() });
        }
        if method == "paper.sheet" {
            return (200, crate::paper::sheet(self, args).await);
        }
        if let Err(e) = self.connected().await {
            return (503, json!({"ok": false, "tx": null, "kind": null, "error": plain(&e)}));
        }
        // `account` picks which record of this key set the method works on (1 = state.json). The same
        // sends, money lock and status cache are shared across accounts; only the state path differs.
        let account = match account_of(args) {
            Ok(n) => n,
            Err(e) => return (200, refusal(e)),
        };
        let other = (account != self.ctx.account).then(|| self.at(account));
        let core = other.as_ref().unwrap_or(self);
        match method {
            "status" => core.status().await,
            "quote" => (200, core.quote(args).await),
            "send" => (200, core.send(args).await),
            "finish" => (200, core.finish().await),
            "stop" => (200, core.stop().await),
            "decline" => (200, core.decline(args).await),
            "funding" => (200, core.funding().await),
            "coins.info" => (200, core.coins_info().await),
            "fund" => (200, core.fund(args).await),
            "topup" => (200, core.topup(args).await),
            "takeout" => (200, core.takeout(args).await),
            "purse.pay" => (200, core.purse_pay(args).await),
            "keys.reset" => (200, core.keys_reset(args).await),
            "rules.change" => (200, core.rules_change(args).await),
            "heir.set" => (200, core.heir_set(args).await),
            "guardian.set" => (200, core.guardian_set(args).await),
            "paper.make" => (200, crate::paper::make(core, args).await),
            "paper.confirm" => (200, crate::paper::confirm(core, args).await),
            _ => (404, refusal("Not found")),
        }
    }

    /// A Core on another folder (a watched account, watched.rs): the same money lock and sends, its own cache.
    pub(crate) fn beside(&self, ctx: Ctx) -> Core {
        Core { ctx, sends: self.sends.clone(), money: self.money.clone(), cache: Arc::new(Mutex::new(HashMap::new())), alerts_warned: Arc::new(AtomicBool::new(false)) }
    }

    /// The one-at-a-time money lock, for a move run on a Ctx outside any Core (watch.stop/decline/claim).
    pub(crate) fn money_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.money.clone()
    }

    /// `rules.change` for paper.rs: paper.make leaves its stop waiting exactly as a guardian's.
    pub(crate) async fn rules_change_paper(&self, args: &Value) -> Value {
        self.rules_change(args).await
    }

    /// This Core on another account of the key set: the same shared sends, money lock and cache.
    fn at(&self, account: u32) -> Core {
        Core { ctx: self.ctx.with_account(account), sends: self.sends.clone(), money: self.money.clone(), cache: self.cache.clone(), alerts_warned: self.alerts_warned.clone() }
    }

    /// `call` for the money methods on the phone (D3): the shared Core holds the public block only, and
    /// the one secret the move signs with rides in as `block` for exactly this call: [spend 32] for
    /// `Slot::Card`, [stop 32][funding 32] for `Slot::Stop`, [funding 32] for `Slot::Funding`. The handler
    /// runs on a short-lived armed copy of this Core (same sends, money lock, cache); its keys are wiped on
    /// every path, the block with them. Same gate, words and answer shapes as `call`.
    /// `Slot::Topup` is [spend 32][funding 32]; `Slot::Sheets` is the text of two sheets (`takeout`);
    /// `Slot::Safe` is the phone's sealed cold secrets (`takeout` without paper, `keys.sheets`). A slot that is
    /// not the method's own is refused (400, WRONG_KEY) before anything is armed. `keys.sheets` reads its block
    /// and answers here, offline; it never arms a Core.
    pub async fn call_with_secret(&self, method: &str, args: &Value, slot: Slot, block: Zeroizing<Vec<u8>>) -> (u16, Value) {
        if !args.is_object() {
            return (400, refusal("Bad request"));
        }
        if !slot.serves(method) {
            return (400, refusal(WRONG_KEY));
        }
        if !slot.accepts(block.len()) {
            return (400, refusal(KEY_SIZE));
        }
        if method == "keys.sheets" {
            return (200, sheets_answer(&block));
        }
        // The heir key never goes into a Core (no_heir_key): the words answer straight from the block.
        if method == "link.heir_words" {
            let secret = Zeroizing::new(<[u8; 32]>::try_from(&block[..]).expect("Heir accepts 32 bytes"));
            drop(block);
            return (200, crate::link::heir_words(&self.ctx.dir, &secret).unwrap_or_else(|e| refusal(&e)));
        }
        // watch.stop / watch.decline / watch.claim (watched.rs) run on the watched account's own folder; the
        // secret never enters this Core. heir.pay spends the heir address (this phone's own link) here.
        if matches!(slot, Slot::Guardian | Slot::Heir) {
            if method == "heir.pay" {
                let secret = Zeroizing::new(<[u8; 32]>::try_from(&block[..]).expect("Heir accepts 32 bytes"));
                drop(block);
                return (200, self.heir_pay(args, &secret).await);
            }
            let secret = Zeroizing::new(<[u8; 32]>::try_from(&block[..]).expect("Guardian and Heir accept 32 bytes"));
            drop(block);
            return crate::watched::call_with_secret(self, method, args, secret).await;
        }
        let armed = match Armed::new(self, slot, &block) {
            Ok(a) => a,
            Err(e) => {
                drop(block);
                return (200, refusal(&e));
            }
        };
        drop(block);
        let answer = armed.core.call(method, args).await;
        drop(armed);
        answer
    }

    /// `guard.*` (D5/D6): the guard inside the app. Every answer is 200 with `ok` true, or the refusal words.
    pub(crate) async fn guard_call(&self, method: &str, args: &Value) -> (u16, Value) {
        let dir = &self.ctx.dir;
        let answer = match method {
            "guard.start" => self.guard_start(args).await,
            "guard.stop" => guard_stop().await,
            "guard.status" => Ok(guard_status(dir)),
            "guard.wait" => Ok(guard_wait(dir, Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(30_000).min(GUARD_WAIT_MAX_MS))).await),
            "guard.poll" => guard_poll(args["seconds"].as_u64()),
            // `account` picks the record that travels (state-N.json), absent = 1.
            "guard.export" => match account_of(args) {
                Ok(n) => guard_export(dir, &self.ctx.network, Some(&dir.join(state_file_name(n))), &string_list(&args["nodes"]), args["name"].as_str()),
                Err(e) => Err(e.into()),
            },
            "guard.import" => match args["text"].as_str() {
                Some(text) => guard_import(dir, &self.ctx.network, text),
                None => Err(NOT_GUARD_FILE.into()),
            },
            _ => return (404, refusal("Not found")),
        };
        match answer {
            Ok(v) => (200, v),
            Err(e) => (200, refusal(&e)),
        }
    }

    /// `guard.start {nodes: [..], auto_heir?, poll_seconds?}`: spawns `watch::run_guard` on this runtime. Two
    /// different node urls are required (check_nodes' words); the Core's own node may be `nodes[0]`. Keyless: the
    /// public block is read from disk and the guard gets its own connection to `nodes[0]`.
    /// Fewer than two different hosts typed: the rest come from the secure resolver
    /// (`fill_guard_nodes`), answered as `filled: [url..]` so the page can label them.
    async fn guard_start(&self, args: &Value) -> Result<Value, String> {
        let given = string_list(&args["nodes"]);
        let auto_heir = args["auto_heir"].as_bool().unwrap_or(false);
        let poll_seconds = args["poll_seconds"].as_u64().unwrap_or(3).clamp(1, 3600);
        if guard(guard_slot()).as_ref().is_some_and(|h| !h.task.is_finished()) {
            return Err(GUARD_RUNNING.into());
        }
        let (resolver, net) = (secure_resolver(), &self.ctx.network);
        let fill = fill_guard_nodes(given, || resolver_node(&resolver, net));
        let (nodes, filled) = tokio::time::timeout(GUARD_FILL_TOTAL, fill)
            .await
            .map_err(|_| GUARD_NO_SECOND_NODE.to_string())??;
        crate::watch::check_nodes(&nodes)?;
        let keys = Keys::load_public(&self.ctx.dir.join("keys.json"))?;
        let rpc = connect(Some(&nodes[0]), &self.ctx.network).await?;
        let ctx = Ctx { dir: self.ctx.dir.clone(), keys, rpc: Some(rpc), force: false, network: self.ctx.network.clone(), dials: self.ctx.dials.clone(), account: 1 };
        let poll = Arc::new(AtomicU64::new(poll_seconds));
        let (stop, rx) = tokio::sync::watch::channel(false);
        let woke = Arc::new(tokio::sync::Notify::new());
        let exited = Arc::new(Mutex::new(None));
        let started = now_secs();
        let task = {
            let (poll, woke, exited) = (poll.clone(), woke.clone(), exited.clone());
            tokio::spawn(async move {
                let ended = crate::watch::run_guard(ctx, nodes, auto_heir, poll, rx, woke.clone()).await;
                *guard(&exited) = Some(ended.err().unwrap_or_else(|| "finished".into()));
                woke.notify_waiters();
            })
        };
        *guard(guard_slot()) = Some(GuardHandle { task, stop, woke, poll, started, exited });
        Ok(json!({"ok": true, "started": started, "filled": filled}))
    }

    /// `keys.public {}`: the rule block, its code, the top-up address and the carry file (`vault-public.json`)
    /// for the drawer device, and `stops` (who holds each stop slot, `stop_holders`). Answers offline (no gate).
    fn keys_public(&self) -> Value {
        let Some(p) = &self.ctx.keys.public else { return no_keys() };
        let code = self.ctx.keys.public_sha256.clone().unwrap_or_else(|| p.sha256());
        let export = PublicExport { kind: PUBLIC_EXPORT_KIND.into(), public: p.clone(), public_sha256: code.clone() };
        let stops = stop_holders(p, &read_unheld(&self.ctx.dir));
        let stops = crate::paper::mark_paper(stops, p, &self.ctx.dir);
        json!({"ok": true, "public": p, "public_sha256": code, "funding_address": self.ctx.funding_addr().ok().map(|a| a.to_string()), "export": export, "stops": stops})
    }

    /// `funding {}`: the top-up purse, the address a faucet or an exchange pays before `fund`, and what it holds.
    async fn funding(&self) -> Value {
        let Ok(address) = self.ctx.funding_addr() else { return no_keys() };
        match self.ctx.try_utxos(&address).await {
            Ok(coins) => json!({"ok": true, "address": address.to_string(), "kas": kas(coins.iter().map(|(_, u)| u.amount).sum())}),
            Err(e) => refusal(&plain(&e)),
        }
    }

    /// `coins.info {}` (keyless, moves nothing): the purse's coins as `coins_answer` reads them.
    /// Answers
    /// {ok, count, total_sompi, amounts_sompi, fee_sompi, error}.
    async fn coins_info(&self) -> Value {
        let fail = |e: &str| json!({"ok": false, "count": null, "total_sompi": null,
            "amounts_sompi": null, "fee_sompi": null, "error": e});
        let Ok(address) = self.ctx.funding_addr() else { return fail(COINS_NO_PURSE) };
        match self.ctx.try_utxos(&address).await {
            Ok(coins) => coins_answer(&self.ctx, &coins),
            Err(e) => fail(&plain(&e)),
        }
    }

    /// `fund {kas, delay?, window?, cap_kas?, max_fee_kas?, min_remainder_kas?, heir_delay?}`: opens the
    /// account from the top-up purse with this Core's dials turned by the request (commands.rs do_fund).
    /// Needs the funding key: `Slot::Funding` on the phone, keys.json on the laptop.
    async fn fund(&self, args: &Value) -> Value {
        let Some(amount_kas) = parse_kas(&args["kas"]).map(kas) else { return result(Err(BAD_AMOUNT.into())) };
        let base = match opening_dials(&self.ctx, args) {
            Ok(d) => d,
            Err(e) => return result(Err(e)),
        };
        let dials = match base.turned(args) {
            Ok(d) => d,
            Err(e) => return result(Err(e)),
        };
        let Ok(_money) = self.money.try_lock() else { return result(Err(BUSY.into())) };
        let mut ctx = self.ctx.clone();
        ctx.dials = dials;
        // A guardian or heir a `rules.change` left waiting: the rule block is rewritten first (every record closed),
        // and the opening runs under the NEW block, the armed funding secret kept.
        let applied = match self.apply_people_next().await {
            Ok(Some(fresh)) => {
                ctx.keys.public = fresh.public;
                ctx.keys.public_sha256 = fresh.public_sha256;
                true
            }
            Ok(None) => false,
            Err(e) => {
                ctx.keys.wipe();
                return result(Err(e));
            }
        };
        let funded = ctx.do_fund(amount_kas).await;
        ctx.keys.wipe();
        if funded.is_ok() {
            clear_rules_next(&self.ctx.dir, self.ctx.account);
            self.settle().await;
        } else {
            guard(&self.cache).clear();
        }
        let mut answer = result(funded.map(|tx| (tx, self.paid_fee())));
        // Set on a failed opening too: the block on disk changed either way and the caller must reopen on it.
        if applied {
            answer["rules_block_changed"] = json!(true);
        }
        answer
    }

    /// Applies `people-next.json` with the heir.set rewrite; the caller holds the money lock. Answers the new
    /// public-only keys, or None when nothing waits. The file is removed as soon as the rewrite landed: the rewrite
    /// stays whether or not the opening that follows succeeds (the records are closed, the block is new), so the
    /// file is applied either way, and a crash mid-opening must not leave it to apply a second time.
    async fn apply_people_next(&self) -> Result<Option<Keys>, String> {
        let changes = match read_people_next(&self.ctx.dir)? {
            Some(c) => c,
            // Builds before 62 wrote a removed guardian as an empty stop slot, which no opening accepts: fill it now.
            None => {
                let p = Keys::load_public(&self.ctx.dir.join("keys.json")).map(|k| k.public()).ok();
                let empty: People = p.iter().flat_map(|p| [("cancel2_x", &p.cancel2_x), ("cancel3_x", &p.cancel3_x)]).filter(|(_, x)| x.is_empty()).map(|(k, _)| (k.to_string(), json!(""))).collect();
                if empty.is_empty() {
                    return Ok(None);
                }
                empty
            }
        };
        // This account's lines carry into the reopened record (commands.rs do_fund reads carry-N.json).
        if let Some(v) = self.ctx.try_load_vault()? {
            crate::commands::write_carry(&self.ctx.dir, self.ctx.account, &v.history)?;
        }
        let fresh = match self.rewrite_public(&changes, None).await {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_file(self.ctx.dir.join(crate::commands::carry_file_name(self.ctx.account)));
                return Err(e);
            }
        };
        clear_people_next(&self.ctx.dir);
        Ok(Some(fresh))
    }

    /// `topup {kas}`: adds to the open account from the purse (commands.rs do_topup: stage=0 only, the state
    /// unchanged). Needs the spend key and the funding key: `Slot::Topup` on the phone, keys.json on the laptop.
    /// Answers {ok, tx, kas, fee_kas, error}.
    async fn topup(&self, args: &Value) -> Value {
        let fail = |e: &str| json!({"ok": false, "tx": null, "kas": null, "fee_kas": null, "error": plain(e)});
        let Some(amount) = parse_kas(&args["kas"]) else { return fail(BAD_AMOUNT) };
        let Ok(_money) = self.money.try_lock() else { return fail(BUSY) };
        let mut ctx = self.ctx.clone();
        let added = ctx.do_topup(kas(amount)).await;
        ctx.keys.wipe();
        if added.is_ok() {
            self.settle().await;
        } else {
            guard(&self.cache).clear();
        }
        match added {
            Ok(tx) => json!({"ok": true, "tx": tx, "kas": kas(amount), "fee_kas": self.paid_fee(), "error": null}),
            Err(e) => fail(&e),
        }
    }

    /// `takeout {to?, kas?}` (mainnet): spend from the safe with two of its three sheets, armed for
    /// this call alone (`Slot::Sheets` typed from paper, or `Slot::Safe` from the phone's sealed keys); through
    /// `call` with no sheets armed it refuses. `to` default = this device's purse (the top-up address); `kas`
    /// absent = everything less the fee. The build, signing and proof are airgap.rs's cold-send path
    /// (`airgap::takeout`). Answers {ok, tx, kas (what reached `to`), to, error}. One money action at a time.
    /// `takeout`'s `from_stop`: the tx of the coin this account's stop put in the safe, or the
    /// refusal.
    async fn stop_tx(&self) -> Result<String, String> {
        let v = match self.ctx.try_load_vault() {
            Ok(Some(v)) => v,
            Ok(None) => return Err(NOW_NOT_STOPPED.into()),
            Err(e) => return Err(e),
        };
        if is_stopped(&v) {
            let tx = v.history.last().and_then(|h| tx_of(h));
            return tx.map(str::to_string).ok_or_else(|| NOW_NOT_STOPPED.into());
        }
        // Stopped from another phone (a guardian): this record never saw the stop; the node names
        // its coin. No stop tx is known here (None): this only picks which of the owner's own safe
        // coins to take out, so a sibling's coin cannot cost money.
        if !self.ctx.record_ended(&v).await? {
            return Err(NOW_NOT_STOPPED.into());
        }
        let found = self.ctx.stop_from_elsewhere(&v, None).await?;
        found.ok_or_else(|| NOW_ENDED_NOTHING_IN_SAFE.into())
    }

    async fn takeout(&self, args: &Value) -> Value {
        let fail = |e: &str| json!({"ok": false, "tx": null, "kas": null, "to": null, "error": plain(e)});
        if self.ctx.keys.cold_keys().len() != 2 {
            return fail(TAKEOUT_NEEDS_SHEETS);
        }
        let to = match args["to"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            Some(to) => {
                if self.ctx.network.address(to).is_err() {
                    return fail(BAD_ADDRESS);
                }
                to.to_string()
            }
            None => match self.ctx.funding_addr() {
                Ok(a) => a.to_string(),
                Err(_) => return fail(NO_KEYS),
            },
        };
        // `from_stop` ("Update now"): only the coin this account's own stop put in the safe, all of it less the fee.
        let only_tx = if args["from_stop"].as_bool() == Some(true) {
            match self.stop_tx().await {
                Ok(tx) => Some(tx),
                Err(e) => return fail(&e),
            }
        } else {
            None
        };
        let amount = if args["kas"].is_null() || only_tx.is_some() {
            None
        } else {
            match parse_kas(&args["kas"]) {
                Some(s) => Some(s),
                None => return fail(BAD_AMOUNT),
            }
        };
        let Ok(_money) = self.money.try_lock() else { return fail(BUSY) };
        let out = crate::airgap::takeout(&self.ctx, amount, &to, only_tx.as_deref()).await;
        guard(&self.cache).clear();
        match out {
            // open_kas: what the reopening can lock from this take-out (a margin for its fee; the rest folds in).
            Ok((tx, paid, to)) => json!({"ok": true, "tx": tx, "kas": kas(paid), "open_kas": kas(paid.saturating_sub(NOW_OPEN_MARGIN)), "to": to, "error": null}),
            Err(e) => fail(&e),
        }
    }

    /// `purse.pay {to, kas?}`: a plain payment from the purse (the funding key) to any address; `kas` absent =
    /// everything less the fee (commands.rs do_pay_sompi). Needs the funding key (`Slot::Funding`).
    /// Answers {ok, tx, kas (what reached `to`), to, error}.
    async fn purse_pay(&self, args: &Value) -> Value {
        let fail = |e: &str| json!({"ok": false, "tx": null, "kas": null, "to": null, "error": plain(e)});
        let to = args["to"].as_str().unwrap_or("").trim().to_string();
        if self.ctx.network.address(&to).is_err() {
            return fail(BAD_ADDRESS);
        }
        let amount = if args["kas"].is_null() {
            None
        } else {
            match parse_kas(&args["kas"]) {
                Some(s) => Some(s),
                None => return fail(BAD_AMOUNT),
            }
        };
        let Ok(_money) = self.money.try_lock() else { return fail(BUSY) };
        let mut ctx = self.ctx.clone();
        let paid = ctx.do_pay_sompi(amount, &to).await;
        ctx.keys.wipe();
        guard(&self.cache).clear();
        match paid {
            Ok((tx, sompi)) => json!({"ok": true, "tx": tx, "kas": kas(sompi), "to": to, "error": null}),
            Err(e) => fail(&e),
        }
    }

    /// `heir.purse` (keyless): what heir claims paid this phone, at its link's heir address. Answers
    /// {ok, address, kas, error}; no link = ok false.
    async fn heir_purse(&self) -> Value {
        let address = match crate::link::load(&self.ctx.dir) {
            Ok(l) => l.heir_address,
            Err(_) => return json!({"ok": false, "address": null, "kas": null, "error": HEIR_NO_LINK}),
        };
        let held = match self.ctx.network.address(&address) {
            Ok(a) => self.ctx.try_utxos(&a).await.map(|c| c.iter().map(|(_, u)| u.amount).sum::<u64>()),
            Err(e) => Err(e),
        };
        match held {
            Ok(s) => json!({"ok": true, "address": address, "kas": kas(s), "error": null}),
            Err(e) => json!({"ok": false, "address": address, "kas": null, "error": plain(&e)}),
        }
    }

    /// `heir.pay {to, kas?}` (Slot Heir): a plain payment from this phone's heir address (what heir claims paid
    /// it), as `purse.pay` is for the purse; `kas` absent = all of it less the fee. The key must be this phone's
    /// own heir key (link.json heir_x). Answers {ok, tx, kas (what reached `to`), to, error}.
    async fn heir_pay(&self, args: &Value, secret: &[u8; 32]) -> Value {
        let fail = |e: &str| json!({"ok": false, "tx": null, "kas": null, "to": null, "error": plain(e)});
        match crate::link::load(&self.ctx.dir) {
            Ok(l) if try_xonly_hex(secret).is_some_and(|x| x == l.heir_x) => {}
            Ok(_) => return fail(crate::link::WRONG_HEIR),
            Err(_) => return fail(HEIR_NO_LINK),
        }
        let to = args["to"].as_str().unwrap_or("").trim().to_string();
        if self.ctx.network.address(&to).is_err() {
            return fail(BAD_ADDRESS);
        }
        let amount = if args["kas"].is_null() {
            None
        } else {
            match parse_kas(&args["kas"]) {
                Some(s) => Some(s),
                None => return fail(BAD_AMOUNT),
            }
        };
        if let Err(e) = self.connected().await {
            return fail(&e);
        }
        let Ok(_money) = self.money.try_lock() else { return fail(BUSY) };
        let mut ctx = self.ctx.clone();
        ctx.keys = Keys { funding: Some(*secret), ..Keys::default() };
        let paid = ctx.do_pay_sompi(amount, &to).await;
        ctx.keys.wipe();
        match paid {
            Ok((tx, sompi)) => json!({"ok": true, "tx": tx, "kas": kas(sompi), "to": to, "error": null}),
            Err(e) => fail(&e),
        }
    }

    /// `keys.reset {confirm: true}` (keyless): starts this device over. Refused unless confirmed and nothing of
    /// this key set holds money: the purse, the account (no state.json, or stopped with no coin at the vault
    /// address) and the safe (its keys are the sheets this rule block names; a new block could not spend it from
    /// here). Then keys.json, state.json, vault.sil, vault-public.json, watch.json, alerts*.json and the burn marker
    /// move into the sibling folder `<network>-old-<unix seconds>`, so `keysNew` can run again. Answers
    /// {ok:true, archived: <folder name>} or {ok:false, error}.
    async fn keys_reset(&self, args: &Value) -> Value {
        match self.try_keys_reset(args).await {
            Ok(name) => json!({"ok": true, "archived": name}),
            Err(e) => refusal(&plain(&e)),
        }
    }

    async fn try_keys_reset(&self, args: &Value) -> Result<String, String> {
        if args["confirm"] != json!(true) {
            return Err(RESET_UNCONFIRMED.into());
        }
        let _money = self.money.try_lock().map_err(|_| BUSY.to_string())?;
        let ctx = &self.ctx;
        let sum = |coins: Vec<Coin>| coins.iter().map(|(_, u)| u.amount).sum::<u64>();
        let purse = sum(ctx.try_utxos(&ctx.funding_addr()?).await?);
        if purse > 0 {
            return Err(format!("The purse still holds {:.8} KAS. Move it out first.", kas(purse)));
        }
        let public = ctx.keys.public();
        let safe = sum(ctx.try_utxos(&ctx.network.address(&public.cold_address)?).await?);
        // A safe whose paper sheets are lost can never be emptied from this phone: with leave_safe the owner
        // says so in words and starts over without it.
        if safe > 0 && !args["leave_safe"].as_bool().unwrap_or(false) {
            return Err(format!("The vault still holds {:.8} KAS. Take it out first, or say that you leave it behind.", kas(safe)));
        }
        for n in accounts_in(&ctx.dir) {
            let ctx = &ctx.with_account(n);
            let Some(v) = ctx.try_load_vault()? else { continue };
            let address = ctx.try_vault_address(&v.state).map_err(|e| if rules_unreadable(&e) { OLD_RULES.to_string() } else { e })?;
            let held: u64 = ctx.try_utxos(&address).await?.into_iter().filter(|(_, u)| u.covenant_id == Some(v.covenant_id)).map(|(_, u)| u.amount).sum();
            if held > 0 {
                return Err(if is_stopped(&v) {
                    format!("Account {n} still holds {:.8} KAS.", kas(held))
                } else {
                    format!("Account {n} is still open with {:.8} KAS. Stop it first.", kas(held))
                });
            }
            if !is_stopped(&v) {
                let age = std::fs::metadata(ctx.state_path()).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
                if age.is_none_or(|a| a.as_secs() <= STATE_SETTLE_SECONDS) {
                    return Err("is the last transaction accepted yet".into());
                }
            }
        }
        let name = format!("{}-old-{}", ctx.network.name, now_secs());
        let parent = ctx.dir.parent().ok_or("the wallet folder has no parent folder")?;
        let dest = parent.join(&name);
        std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        for entry in std::fs::read_dir(&ctx.dir).map_err(|e| format!("{}: {e}", ctx.dir.display()))?.flatten() {
            let file = entry.file_name();
            let Some(fname) = file.to_str() else { continue };
            if RESET_FILES.contains(&fname) || is_account_file(fname) || (fname.starts_with("alerts") && fname.ends_with(".json")) {
                std::fs::rename(entry.path(), dest.join(fname)).map_err(|e| format!("{fname}: {e}"))?;
            }
        }
        guard(&self.cache).clear();
        Ok(name)
    }

    /// `keys.burned {}` (keyless, offline): the sealed sheets left this phone (the app wiped them); the marker
    /// file goes, so status says `sheets_on_phone: false`. Answers {ok:true}.
    fn keys_burned(&self) -> Value {
        match std::fs::remove_file(self.ctx.dir.join(SHEETS_ON_PHONE_FILE)) {
            Ok(()) => json!({"ok": true}),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"ok": true}),
            Err(e) => refusal(&plain(&format!("{}: {e}", SHEETS_ON_PHONE_FILE))),
        }
    }

    /// The context, coin and signing key a quote is built with: the real ones when the secret is here (the
    /// laptop), else the shadow vault's (the phone quotes before the fingerprint, and serve never holds a
    /// stop key it did not load). `pick` names the secret the move signs with.
    fn sized(&self, coin: &Coin, state: &State, pick: fn(&Keys) -> &Option<[u8; 32]>) -> (Ctx, Coin, Keypair) {
        if let Some(s) = pick(&self.ctx.keys) {
            return (self.ctx.clone(), coin.clone(), keypair(s));
        }
        let (ctx, coin) = self.shadow(coin, state);
        let key = keypair(pick(&ctx.keys).as_ref().expect("the shadow holds all nine keys"));
        (ctx, coin, key)
    }

    /// A shadow of this vault for pricing without a secret: nine keys this process just made, under the
    /// real block's version, dials, flags and shape (heir or none, three stop keys or one, 2-of-3 safe or a
    /// legacy single sheet), and the real coin re-addressed to the shadow's script. Every signature is then
    /// a real one and the engine preflight passes (a throwaway key alone fails checkSig there); an x-only
    /// key, a Schnorr signature and a script hash are fixed-size, so the shadow's fee is the real move's fee.
    fn shadow(&self, coin: &Coin, state: &State) -> (Ctx, Coin) {
        let real = self.ctx.keys.public();
        let net = &self.ctx.network;
        let mut keys = Keys::fresh(net);
        let mut p = keys.public();
        if real.heir_x.is_empty() {
            keys.heir = None;
            p.heir_x.clear();
            p.heir_address.clear();
        }
        if real.cancel2_x.is_empty() {
            keys.cancel2 = None;
            p.cancel2_x.clear();
        }
        if real.cancel3_x.is_empty() {
            keys.cancel3 = None;
            p.cancel3_x.clear();
        }
        if real.cold_x.is_empty() {
            (keys.cold2, keys.cold3) = (None, None);
            p.cold_x.clear();
            // An outside safe (`set-cold-to`) is priced to its own address; legacy to a sheet.
            let sheet = || p2pk_address(net.prefix, &keys.cold_key()).to_string();
            p.cold_address = if real.cold_external { real.cold_address.clone() } else { sheet() };
            p.cold_external = real.cold_external;
        }
        (p.v, p.auto_heir, p.hard_limit, p.network) = (real.v, real.auto_heir, real.hard_limit, real.network.clone());
        keys.public = Some(p);
        let ctx = Ctx { keys, ..self.ctx.clone() };
        let entry = UtxoEntry::new(coin.1.amount, ctx.vault_spk(state), coin.1.block_daa_score, coin.1.is_coinbase, coin.1.covenant_id);
        (ctx, (coin.0, entry))
    }

    pub(crate) async fn connected(&self) -> Result<(), String> {
        let rpc = self.ctx.rpc();
        if rpc.is_connected() {
            return Ok(());
        }
        println!("node connection lost; reconnecting");
        let options = ConnectOptions { block_async_connect: true, strategy: ConnectStrategy::Fallback, ..Default::default() };
        rpc.connect(Some(options)).await.map_err(|e| format!("Can't reach the network right now: {e}"))?;
        crate::tx::ensure_secure(&rpc, "after a reconnect").await.map(|_| ())
    }

    /// A guardian/heir-only phone (no key set) still needs network time to end its fingerprint-change
    /// wait, so its status refusal carries a daa_now asked now (fresh by construction, never cached;
    /// null when the node does not answer).
    async fn no_keys_status(&self) -> Value {
        let daa_now = match self.connected().await {
            Ok(()) => self.ctx.try_daa().await.ok(),
            Err(_) => None,
        };
        let mut v = no_keys();
        v["daa_now"] = json!(daa_now);
        v["network"] = json!(self.ctx.network.name);
        v
    }

    pub(crate) async fn status(&self) -> (u16, Value) {
        if let Some((at, v)) = guard(&self.cache).get(&self.ctx.account)
            && at.elapsed() < STATUS_TTL {
                return (200, v.clone());
            }
        match self.build_status().await {
            Ok(v) => {
                guard(&self.cache).insert(self.ctx.account, (Instant::now(), v.clone()));
                let _ = std::fs::write(self.ctx.dir.join(status_cache_name(self.ctx.account)), v.to_string());
                (200, v)
            }
            // A state file mid-write or a coin still landing: the last answer beats an error.
            // Never with daa_now: an old network time would shorten the phone's fingerprint-change wait.
            Err(e) => {
                let cached = guard(&self.cache).get(&self.ctx.account).map(|(_, v)| v.clone()); // lock dropped here
                match cached {
                    Some(v) => (200, without_daa_now(v)),
                    // A network time asked now (fresh by construction): a guardian-only phone still measures its wait.
                    None => (503, json!({"error": plain(&e), "network": self.ctx.network.name,
                        "daa_now": self.ctx.try_daa().await.ok()})),
                }
            }
        }
    }

    async fn build_status(&self) -> Result<Value, String> {
        let mut status = self.state_status().await?;
        // The network's DAA score right now (null when the node did not answer): the phone measures its
        // fingerprint-change wait in network time too, so a moved phone clock cannot shorten it.
        let daa_now = self.ctx.try_daa().await.ok();
        if let Some(o) = status.as_object_mut() {
            o.insert("daa_now".into(), json!(daa_now));
            // The safe's address (the 2-of-3 sheets' P2SH) and whether this phone still keeps the sheets.
            o.insert("safe_address".into(), json!(self.ctx.keys.public().cold_address));
            o.insert("sheets_on_phone".into(), json!(self.ctx.dir.join(SHEETS_ON_PHONE_FILE).exists()));
            // Which account this is, which exist (have a record), the next free id, and the rules a
            // `rules.change` left waiting for this account's next opening (dials + their words), else null.
            o.insert("account".into(), json!(self.ctx.account));
            o.insert("accounts".into(), json!(accounts_in(&self.ctx.dir)));
            o.insert("next_account".into(), json!(next_account(&self.ctx.dir)));
            let has_heir = !self.ctx.keys.public().heir_x.is_empty();
            let next = read_rules_next(&self.ctx.dir, self.ctx.account).ok().flatten().map(|d| json!({"dials": d, "words": d.describe(self.ctx.network.daa_per_sec, has_heir)}));
            o.insert("rules_next".into(), next.unwrap_or(Value::Null));
            // The guardian and heir a `rules.change` left for the next opening (key-set wide), else null.
            o.insert("people_next".into(), read_people_next(&self.ctx.dir).ok().flatten().map(Value::Object).unwrap_or(Value::Null));
        }
        // The purse (the top-up funding address) is a balance of its own: Home shows every balance.
        // Left out, not failed, when the purse cannot be read.
        if let Ok(address) = self.ctx.funding_addr()
            && let Ok(coins) = self.ctx.try_utxos(&address).await
                && let Some(o) = status.as_object_mut() {
                    o.insert("purse_kas".into(), json!(kas(coins.iter().map(|(_, u)| u.amount).sum::<u64>())));
                    o.insert("purse_address".into(), json!(address.to_string()));
                }
        Ok(match read_alerts(&self.ctx.dir) {
            Ok(Some(file)) => {
                self.alerts_warned.store(false, Ordering::Relaxed);
                with_watcher(status, &file, now_secs())
            }
            Ok(None) => status,
            // Missing or broken: the status goes out without the watcher's fields, said once in the log.
            Err(e) => {
                if !self.alerts_warned.swap(true, Ordering::Relaxed) {
                    println!("watcher file unreadable; status carries no alerts: {e}");
                }
                status
            }
        })
    }

    /// The status from state.json and the chain alone; `build_status` adds the watcher's view.
    async fn state_status(&self) -> Result<Value, String> {
        let ctx = &self.ctx;
        let public = ctx.keys.public();
        let cold = ctx.network.address(&public.cold_address)?;
        let safe: u64 = ctx.try_utxos(&cold).await?.iter().map(|(_, u)| u.amount).sum();
        let unheld = read_unheld(&ctx.dir);
        let paper = crate::paper::paper_keys(&ctx.dir);
        let Some(v) = ctx.try_load_vault()? else { return Ok(stopped_status(&ctx.network, &ctx.dials()?, &public, &unheld, &paper, safe, vec![])) };
        let history = || history_json(&v, &ctx.state_path());
        if is_stopped(&v) {
            return Ok(stopped_status(&ctx.network, &v.dials, &public, &unheld, &paper, safe, history()));
        }
        // An account this program cannot compile (rules it does not carry) is shown frozen, in words, not a panic.
        let address = match ctx.try_vault_address(&v.state) {
            Ok(a) => a,
            Err(e) if rules_unreadable(&e) => {
                let mut s = stopped_status(&ctx.network, &v.dials, &public, &unheld, &paper, safe, history());
                s["stopped"] = json!(false);
                s["old_rules"] = json!(true);
                s["notice"] = json!(OLD_RULES);
                return Ok(s);
            }
            Err(e) => return Err(e),
        };
        let coins = ctx.try_utxos(&address).await?;
        let Some((_, coin)) = coins.into_iter().find(|(_, u)| u.covenant_id == Some(v.covenant_id)) else {
            let age = std::fs::metadata(ctx.state_path()).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            return match age {
                Some(age) if age.as_secs() > STATE_SETTLE_SECONDS => Ok(stopped_status(&ctx.network, &v.dials, &public, &unheld, &paper, safe, history())),
                _ => Err("is the last transaction accepted yet".into()),
            };
        };
        let daa = ctx.try_daa().await? as i64;
        let lock_time = daa.saturating_sub(20); // the same value a spend would use now
        let coin_daa = coin.block_daa_score as i64;
        let resets = lock_time >= v.state.window_start + v.dials.window && lock_time >= coin_daa + v.dials.window;
        let spent = if resets { 0 } else { v.state.spent };
        let refresh = if resets { 0 } else { (v.state.window_start + v.dials.window).max(coin_daa + v.dials.window) + 20 - daa };
        let pending = (v.state.stage == 1).then(|| {
            json!({
                "to": v.pending_to,
                "amount_kas": kas(v.state.amount as u64),
                "seconds_left": seconds(&ctx.network, coin_daa + v.dials.delay - daa),
                "tx": v.history.iter().rev().find(|h| h.starts_with("request")).and_then(|h| tx_of(h)),
            })
        });
        let (stop_keys, heir, sheets) = protection(&public, &unheld, &paper);
        // v6: seconds until the heir may claim this coin (the watcher's heir clock, one
        // function in contract.rs); null without an heir. The phone warns at `heir_warn_share` like the watcher.
        let heir_seconds_left = heir.then(|| seconds(&ctx.network, heir_daa_left(daa - coin_daa, public.inherit_delay(&v.dials))));
        // The whole heir delay in seconds (same conversion), so the phone can draw the elapsed share and its
        // amber/red warnings; null without an heir.
        let heir_delay_seconds = heir.then(|| seconds(&ctx.network, public.inherit_delay(&v.dials)));
        Ok(json!({
            "network": ctx.network.name,
            "balance_kas": kas(coin.amount),
            "limit_kas": kas(v.dials.cap as u64),
            "spent_kas": kas(spent as u64),
            "left_kas": kas((v.dials.cap - spent).max(0) as u64),
            "refresh_seconds": seconds(&ctx.network, refresh),
            "notice_seconds": seconds(&ctx.network, v.dials.delay),
            "pending": pending,
            "stopped": false,
            "safe_kas": kas(safe),
            "stop_keys": stop_keys,
            "heir": heir,
            "sheets": sheets,
            "hard_limit": public.hard_limit,
            "heir_seconds_left": heir_seconds_left,
            "heir_delay_seconds": heir_delay_seconds,
            "heir_warn_share": HEIR_WARN_SHARE,
            "history": history(),
        }))
    }

    /// The vault if it can act, else the plain reason it can't.
    fn open_vault(&self) -> Result<Vault, String> {
        let v = self.ctx.try_load_vault()?.ok_or(NOT_OPEN)?;
        if is_stopped(&v) {
            return Err(STOPPED.into());
        }
        // Rules this program cannot compile: every money move refuses in words (the handlers below compile freely).
        self.ctx.try_vault_spk(&v.state).map_err(|e| if rules_unreadable(&e) { OLD_RULES.to_string() } else { e })?;
        Ok(v)
    }

    /// `rules.change {account?, <the fund dials>}`: new rules for an account. The rules are
    /// part of an account's address, so an open account cannot be re-tuned in place: the money moves to a fresh
    /// opening with the new rules (stop -> the safe -> take out to the purse -> open). This method moves nothing:
    /// it checks the dials against the account's own, keeps them beside the account as `rules-next-N.json`, and
    /// says the route and the cost first (the stop's fee now, exactly; the take-out and the opening pay about the
    /// same each). The next `fund` on this account that turns no dial itself opens under them. On a stopped or
    /// never-opened account the route is just "open". Beside the dials it takes a guardian (`guardian_x`, 64 hex
    /// or "" = the slot gets a key nobody holds (unheld.json); `guardian_which` 2|3, default 2) and an heir (`heir_x` or
    /// `heir_address`, "" = no heir): those cover every account, so they wait key-set wide in `people-next.json`
    /// for the next `fund`, and only while no other account is open. Answers {ok, route, steps, fee_kas,
    /// fee_kas_about, dials, words, people, error}; `people` = the waiting guardian/heir fields or null.
    async fn rules_change(&self, args: &Value) -> Value {
        let fail = |e: &str| json!({"ok": false, "route": null, "steps": [], "fee_kas": null, "fee_kas_about": null, "move_kas": null, "to": null, "kind": null, "now": null, "dials": null, "words": [], "people": null, "error": plain(e)});
        let people = match self.people_changes(args) {
            Ok(p) => p,
            Err(e) => return fail(&e),
        };
        if !people.is_empty()
            && let Err(e) = self.others_stopped().await {
                return fail(&e);
            }
        // A people-only change keeps the dials already waiting for this account (else its own).
        let base = match (people.is_empty() || has_dial_args(args), read_rules_next(&self.ctx.dir, self.ctx.account)) {
            (false, Ok(Some(d))) => Ok(d),
            (false, Err(e)) => Err(e),
            _ => self.ctx.dials(),
        };
        let dials = match base.and_then(|base| base.turned(args)) {
            Ok(d) => d,
            Err(e) => return fail(&e),
        };
        let waiting = match read_people_next(&self.ctx.dir) {
            Ok(w) => w,
            Err(e) => return fail(&e),
        };
        // The dials are checked against the heir as it WILL be once the waiting changes apply.
        let mut after = waiting.clone().unwrap_or_default();
        after.extend(people.clone());
        let heir_after = after.get("heir_x").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| self.ctx.keys.public().heir_x);
        let has_heir = !heir_after.is_empty();
        if let Err(e) = dials.check(has_heir, self.ctx.network.is_mainnet()) {
            return fail(&e);
        }
        let open = match self.ctx.try_load_vault() {
            Ok(v) => v.is_some_and(|v| !is_stopped(&v)),
            Err(e) => return fail(&e),
        };
        // Route "move" goes through the purse: no safe take-out and no sheets. The contract never lets a payment empty
        // an account (it always keeps its minimum), so the amount is the balance less that minimum and a fee margin;
        // the kept minimum then goes to the safe with an ordinary stop. Fees: the payment (plus its finish when it
        // waits), the stop and the reopening.
        let mut plan = (Value::Null, Value::Null, Value::Null);
        let mut now = Value::Null;
        let (route, steps, fee) = if open {
            let moved = match self.move_plan().await {
                Ok(m) => m,
                Err(e) => return fail(&e),
            };
            let (move_sompi, to, kind, pay_fee) = moved;
            plan = (json!(kas(move_sompi)), json!(to), json!(kind));
            let pay = if kind == "now" { STEP_PAY_NOW } else { STEP_PAY_NOTICE };
            let mut steps = vec![pay];
            if kind != "now" {
                steps.push(STEP_FINISH);
            }
            steps.extend([STEP_KEEP, STEP_OPEN]);
            match self.quote_stop().await {
                Ok((stop_fee, _, _)) => {
                    now = now_option(&self.ctx, stop_fee);
                    ("move", steps, Some(kas(pay_fee + stop_fee)))
                }
                Err(e) => return fail(&e),
            }
        } else {
            ("open", vec![STEP_OPEN], None)
        };
        let pending = if people.is_empty() {
            waiting
        } else {
            match write_people_next(&self.ctx.dir, &people) {
                Ok(m) => Some(m),
                Err(e) => return fail(&e),
            }
        };
        if let Err(e) = write_rules_next(&self.ctx.dir, self.ctx.account, &dials) {
            return fail(&e);
        }
        guard(&self.cache).clear();
        // fee_kas = what is priced exactly now (the payment and the stop); about = with the finish and the reopening.
        json!({"ok": true, "route": route, "steps": steps, "fee_kas": fee, "fee_kas_about": fee.map(|f| f * 2.0), "move_kas": plan.0, "to": plan.1, "kind": plan.2, "now": now, "dials": dials, "words": dials.describe(self.ctx.network.daa_per_sec, has_heir), "people": pending.map(Value::Object), "error": null})
    }

    /// The guardian and heir fields a request names, checked (empty when it names neither).
    fn people_changes(&self, args: &Value) -> Result<People, String> {
        let mut changes = People::new();
        if let Some(x) = args["guardian_x"].as_str() {
            let (field, x) = self.parse_guardian(x, &args["guardian_which"])?;
            changes.insert(field.into(), json!(x));
        }
        if args["heir_x"].is_string() || args["heir_address"].is_string() {
            let (x, address) = self.parse_heir(args)?;
            changes.insert("heir_x".into(), json!(x));
            changes.insert("heir_address".into(), json!(address));
        }
        Ok(changes)
    }

    /// A guardian's stop slot and key: (`cancel2_x` | `cancel3_x`, the key lower-cased, or "" = the slot repeats
    /// stop key 1). `which` 2|3, absent = 2.
    fn parse_guardian(&self, x: &str, which: &Value) -> Result<(&'static str, String), String> {
        let field = match (which.is_null(), which.as_u64()) {
            (true, _) | (_, Some(2)) => "cancel2_x",
            (_, Some(3)) => "cancel3_x",
            _ => return Err(GUARDIAN_WHICH.into()),
        };
        let x = x.trim();
        if x.is_empty() {
            return Ok((field, String::new()));
        }
        // heir_of is the x-only check (and lower-cases); its address is not needed here.
        let (x, _) = Keys::heir_of(&self.ctx.network, x).map_err(|_| GUARDIAN_NOT_A_KEY.to_string())?;
        self.not_own_key(&x, Some(field))?;
        Ok((field, x))
    }

    /// Refuses a guardian or heir key that is one of this wallet's own: spend, stop or safe keys. `slot` is the
    /// stop slot being set, whose current key may be named again (no change).
    fn not_own_key(&self, x: &str, slot: Option<&str>) -> Result<(), String> {
        let p = self.ctx.keys.public();
        let mut mine = vec![&p.spend_x, &p.cancel_x];
        if slot != Some("cancel2_x") {
            mine.push(&p.cancel2_x);
        }
        if slot != Some("cancel3_x") {
            mine.push(&p.cancel3_x);
        }
        if mine.into_iter().chain(p.cold_x.iter()).any(|k| !k.is_empty() && k.eq_ignore_ascii_case(x)) { Err(PEOPLE_OWN_KEY.into()) } else { Ok(()) }
    }

    /// A guardian or heir covers every account of the key set, so no OTHER account may be open while one waits.
    async fn others_stopped(&self) -> Result<(), String> {
        for n in accounts_in(&self.ctx.dir).into_iter().filter(|n| *n != self.ctx.account) {
            let c = self.ctx.with_account(n);
            if let Some(v) = c.try_load_vault()?
                && !c.record_ended(&v).await? {
                return Err(PEOPLE_OTHERS_OPEN.into());
            }
        }
        Ok(())
    }

    /// The payment a rule change starts with: (sompi to send, the purse address, "now" or "notice", its fee in sompi).
    /// The amount is the balance less the account's kept minimum and a fee margin; `quote_send` prices it exactly the
    /// way `send` will route it.
    async fn move_plan(&self) -> Result<(u64, String, &'static str, u64), String> {
        let v = self.open_vault()?;
        if v.state.stage != 0 {
            return Err("A payment is already waiting on this account. Finish or stop it first.".into());
        }
        let coin = self.ctx.try_vault_coin(&v).await?;
        let to = self.ctx.funding_addr()?.to_string();
        let keep = v.dials.min_remainder as u64 + v.dials.max_fee as u64 * 2;
        let amount = coin.1.amount.checked_sub(keep).filter(|a| *a > 0).ok_or("This account holds too little to move; stop it and open again instead.")?;
        let (fee, kind, _) = self.quote_send(kas(amount), &to).await?;
        Ok((amount, to, if kind == "spend" || kind == "now" { "now" } else { "notice" }, fee))
    }

    /// `heir.set {heir_x, auto_heir?}`: names, changes or (empty heir_x) removes the heir after the keys
    /// were made. The heir is part of the rule block every account record is anchored to, so it changes only while
    /// no account holds money: every record must say stopped with nothing left at its address (the purse and the
    /// safe may hold money; neither depends on the heir). Then every record moves into `closed/`, the `public`
    /// block of keys.json gets the heir (its code changes with it; the rest of the file is kept byte for byte in
    /// meaning), vault-public.json is written again, and the caller reopens the engine on the new block
    /// (android.rs does). Answers {ok, heir_address, error}.
    async fn heir_set(&self, args: &Value) -> Value {
        match self.try_heir_set(args).await {
            Ok(address) => json!({"ok": true, "heir_address": address, "error": null}),
            Err(e) => refusal(&plain(&e)),
        }
    }

    async fn try_heir_set(&self, args: &Value) -> Result<String, String> {
        let (heir_x, heir_address) = self.parse_heir(args)?;
        let _money = self.money.try_lock().map_err(|_| BUSY.to_string())?;
        let changes = People::from_iter([("heir_x".to_string(), json!(heir_x)), ("heir_address".to_string(), json!(heir_address))]);
        self.rewrite_public(&changes, args["auto_heir"].as_bool()).await?;
        Ok(heir_address)
    }

    /// The heir a request names: `heir_x` (64 hex) or their pay-to-pubkey address `heir_address` on this network
    /// (the page sends whichever the owner pasted; the key is read out of the address; a safe (script) address
    /// cannot be an heir). Neither or "" = no heir. Answers (heir_x lower-cased, heir_address), both "" for none.
    fn parse_heir(&self, args: &Value) -> Result<(String, String), String> {
        let typed_x = args["heir_x"].as_str().unwrap_or("").trim().to_string();
        let heir_x = match args["heir_address"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
            Some(address) if typed_x.is_empty() => {
                let a = self.ctx.network.address(address)?;
                if a.version != kaspa_addresses::Version::PubKey || a.payload.len() != 32 {
                    return Err(HEIR_NOT_A_PERSON.into());
                }
                faster_hex::hex_string(&a.payload)
            }
            _ => typed_x,
        };
        if heir_x.is_empty() {
            return Ok((String::new(), String::new()));
        }
        let (heir_x, heir_address) = Keys::heir_of(&self.ctx.network, &heir_x).map_err(|_| HEIR_NOT_A_KEY.to_string())?;
        self.not_own_key(&heir_x, None)?;
        Ok((heir_x, heir_address))
    }

    /// `guardian.set {stop_x, which?}`: the immediate form of a `rules.change` guardian (laptop and tests). Stop
    /// slot `which` (2|3, default 2) takes the guardian's key; "" gives the slot back to stop key 1. Same rules as
    /// heir.set: every account stopped and empty, the records closed, the caller reopens on the new block
    /// (android.rs does). Answers {ok, which, stop_x, error}.
    async fn guardian_set(&self, args: &Value) -> Value {
        match self.try_guardian_set(args).await {
            Ok((which, x)) => json!({"ok": true, "which": which, "stop_x": x, "error": null}),
            Err(e) => refusal(&plain(&e)),
        }
    }

    async fn try_guardian_set(&self, args: &Value) -> Result<(u8, String), String> {
        let Some(x) = args["stop_x"].as_str() else { return Err(GUARDIAN_NOT_A_KEY.into()) };
        let (field, x) = self.parse_guardian(x, &args["which"])?;
        let _money = self.money.try_lock().map_err(|_| BUSY.to_string())?;
        self.rewrite_public(&People::from_iter([(field.to_string(), json!(x))]), None).await?;
        Ok((if field == "cancel2_x" { 2 } else { 3 }, x))
    }

    /// The rule-block rewrite heir.set, guardian.set and fund's waiting guardian/heir share; the CALLER holds the
    /// money lock. Every record must say stopped (all checked before any moves), each with nothing left at its
    /// address moves into `closed/`; then keys.json's `public` block takes `changes` (field -> value; the rest of
    /// the file is kept, its code moves with it), vault-public.json is written again. Answers the new public-only keys.
    async fn rewrite_public(&self, changes: &People, auto_heir: Option<bool>) -> Result<Keys, String> {
        let ctx = &self.ctx;
        let mut records = Vec::new();
        for n in accounts_in(&ctx.dir) {
            let c = ctx.with_account(n);
            let Some(v) = c.try_load_vault()? else { continue };
            if !c.record_ended(&v).await? {
                return Err(format!("Account {n} is still open. Stop it and take its funds out first."));
            }
            records.push((c, v));
        }
        for (c, v) in &records {
            let history = c.close_record(v).await?;
            // A line added on closing (moved from another phone) joins the lines waiting for the reopening.
            if history.len() != v.history.len() && ctx.dir.join(crate::commands::carry_file_name(c.account)).exists() {
                crate::commands::write_carry(&ctx.dir, c.account, &history)?;
            }
        }
        let path = ctx.dir.join("keys.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("keys.json: {e}"))?;
        let mut file: Value = serde_json::from_str(&text).map_err(|e| format!("keys.json: {e}"))?;
        let Some(public) = file.get_mut("public").filter(|p| p.is_object()) else { return Err("keys.json has no rule block.".into()) };
        let mut unheld = Vec::new();
        for (k, v) in changes.iter().filter(|(k, _)| PEOPLE_FIELDS.contains(&k.as_str())) {
            // A removed guardian ("") leaves a key nobody holds: the block must keep five different keys.
            if k.starts_with("cancel") && v.as_str() == Some("") {
                unheld.push(unheld_x());
                public[k] = json!(unheld[unheld.len() - 1]);
            } else {
                public[k] = v.clone();
            }
        }
        if let Some(a) = auto_heir {
            public["auto_heir"] = json!(a);
        }
        let block: Public = serde_json::from_value(public.clone()).map_err(|e| format!("keys.json: {e}"))?;
        if file.get("public_sha256").is_some() {
            file["public_sha256"] = json!(block.sha256());
        }
        let out = serde_json::to_string_pretty(&file).map_err(|e| format!("keys.json: {e}"))?;
        write_private(&path, out.as_bytes()).map_err(|e| format!("keys.json: {e}"))?;
        record_unheld(&ctx.dir, &unheld, false)?;
        let fresh = Keys::load_public(&path)?;
        crate::commands::write_public_export(&ctx.dir, &fresh)?;
        guard(&self.cache).clear();
        Ok(fresh)
    }

    /// Waits for the new coin to show, so the next action and status see it.
    async fn settle(&self) {
        for _ in 0..20 {
            if let Ok(Some(v)) = self.ctx.try_load_vault()
                && self.ctx.try_vault_coin(&v).await.is_ok() {
                    break;
                }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        guard(&self.cache).clear();
    }

    async fn send(&self, parsed: &Value) -> Value {
        // An optional `id` from the app: a repeat (the first answer lost on the wifi) gets the first
        // answer back instead of paying twice. Without an `id` every request is its own payment.
        let id = parsed["id"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).map(str::to_string);
        if let Some(id) = &id
            && let Some((_, first)) = guard(&self.sends).iter().find(|(k, _)| k == id) {
                return first.clone();
            }
        let to = parsed["to"].as_str().unwrap_or("").trim().to_string();
        if self.ctx.network.address(&to).is_err() {
            return send_result(Err(BAD_ADDRESS.into()));
        }
        let Some(amount_kas) = parse_kas(&parsed["kas"]).map(kas) else {
            return send_result(Err(BAD_AMOUNT.into()));
        };
        let Ok(_money) = self.money.try_lock() else { return send_result(Err(BUSY.into())) };
        let sent = self.route_send(amount_kas, &to).await;
        if sent.is_ok() {
            self.settle().await;
        }
        let answer = send_result(sent);
        if let (Some(id), true) = (id, answer["ok"] == true) {
            let mut sends = guard(&self.sends);
            if sends.len() >= REMEMBERED_SENDS {
                sends.pop_front();
            }
            sends.push_back((id, answer.clone()));
        }
        answer
    }

    async fn route_send(&self, amount_kas: f64, to: &str) -> Result<(String, &'static str, Option<f64>), String> {
        let v = self.open_vault()?;
        if v.state.stage != 0 {
            return Err("A withdrawal is already waiting. Finish it first.".into());
        }
        let balance = self.ctx.try_vault_coin(&v).await?.1.amount;
        if sompi(amount_kas) >= balance {
            return Err("The account doesn't hold that much.".into());
        }
        let (tx, kind) = match self.ctx.do_spend(amount_kas, to).await {
            Ok(tx) => (tx, "now"),
            // The leftover and fee rules request and finish meet on chain live in the client
            // (commands.rs do_request), in its words; the service no longer keeps a copy.
            Err(e) if e.starts_with(OVER_CAP) || e.starts_with(LOW_REMAINDER) => {
                // v6 hard-limit dial: nothing above the limit may wait; the same words as `request`.
                if self.ctx.keys.public().hard_limit {
                    println!("not a spend ({e}); the hard limit is on, so no withdrawal with notice");
                    return Err(HARD_LIMIT.into());
                }
                println!("not a spend ({e}); trying a withdrawal with notice");
                (self.ctx.do_request(amount_kas, to).await?, "pending")
            }
            Err(e) => return Err(e),
        };
        Ok((tx, kind, self.paid_fee()))
    }

    /// The fee of the transaction that just went out, from the history line the command wrote.
    fn paid_fee(&self) -> Option<f64> {
        self.ctx.try_load_vault().ok().flatten().and_then(|v| v.history.last().and_then(|h| fee_of(h)))
    }

    async fn finish(&self) -> Value {
        let Ok(_money) = self.money.try_lock() else { return result(Err(BUSY.into())) };
        let finished = self.try_finish().await;
        match &finished {
            Ok((_, false)) => self.settle().await,
            _ => guard(&self.cache).clear(),
        }
        result(finished.map(|(tx, _)| (tx, self.paid_fee())))
    }

    async fn try_finish(&self) -> Result<(String, bool), String> {
        let v = self.open_vault()?;
        if v.state.stage != 1 {
            return Err("There is no withdrawal waiting to finish.".into());
        }
        let coin = self.ctx.try_vault_coin(&v).await?;
        let age = self.ctx.try_daa().await?.saturating_sub(coin.1.block_daa_score) as i64;
        if age < v.dials.delay {
            return Err(format!("Too early: about {} seconds of the notice period are left.", seconds(&self.ctx.network, v.dials.delay - age)));
        }
        self.ctx.do_finish().await
    }

    /// The emergency stop never waits for the money lock and never answers BUSY: it fires
    /// even while a send is settling, and the covenant decides between a racing spend and
    /// the cancel (the cancel wins on chain or the spend already moved the coin).
    async fn stop(&self) -> Value {
        no_safe_sheet(&self.ctx.keys).unwrap();
        let stopped = match self.open_vault() {
            // The phone's emergency stop always fires stop key 1, the one serve loads from the offline folder.
            Ok(_) => self.ctx.do_cancel(1).await,
            Err(e) => Err(e),
        };
        guard(&self.cache).clear();
        result(stopped.map(|tx| (tx, self.paid_fee())))
    }

    /// `POST /api/decline` `{ which?: 1|2|3 }` (v5): a stop key sends the waiting withdrawal back to
    /// Locked (commands.rs do_decline); nothing leaves the vault but the network fee. Default: stop
    /// key 1, the one serve loads from the offline folder. Unlike the stop it takes the money lock:
    /// a decline is a quiet move, and two of this laptop's own moves never overlap.
    async fn decline(&self, parsed: &Value) -> Value {
        no_safe_sheet(&self.ctx.keys).unwrap();
        let which = match &parsed["which"] {
            Value::Null => 1,
            w => match w.as_u64() {
                Some(n @ 1..=3) => n as u8,
                _ => return result(Err("Which emergency stop button? 1, 2 or 3.".into())),
            },
        };
        let Ok(_money) = self.money.try_lock() else { return result(Err(BUSY.into())) };
        let declined = match self.open_vault() {
            Ok(v) if v.state.stage != 1 => Err(NOTHING_WAITING.into()),
            Ok(_) => self.ctx.do_decline(which).await,
            Err(e) => Err(e),
        };
        if declined.is_ok() {
            self.settle().await;
        } else {
            guard(&self.cache).clear();
        }
        result(declined.map(|tx| (tx, self.paid_fee())))
    }

    // Quotes: the fee a move would pay right now, from the client's one fee loop (tx.rs `quote`),
    // nothing signed for the network. Each builder below mirrors its commands.rs twin (do_spend,
    // do_request, do_finish, do_cancel), which still holds its closure inline; keep them in step.

    /// `{"kind": "send", "amount": <kas>, "to": <addr>}`, `kind=finish`, `kind=stop`, `kind=decline` (the https route
    /// builds this from `GET /api/quote?kind=..`).
    async fn quote(&self, args: &Value) -> Value {
        let get = |k: &str| args[k].as_str().unwrap_or("");
        let priced = match get("kind") {
            "send" => {
                let to = get("to").trim().to_string();
                if self.ctx.network.address(&to).is_err() {
                    return quote_result(Err(BAD_ADDRESS.into()));
                }
                let Some(amount_kas) = parse_kas(&args["amount"]).map(kas) else {
                    return quote_result(Err(BAD_AMOUNT.into()));
                };
                self.quote_send(amount_kas, &to).await
            }
            "finish" => self.quote_finish().await,
            "stop" => self.quote_stop().await,
            "decline" => self.quote_decline().await,
            _ => Err("That kind of quote is not known: send, finish, stop or decline.".into()),
        };
        quote_result(priced)
    }

    /// Prices a send the way `route_send` sends it: a spend now, else a withdrawal with notice.
    /// Returns (fee, kind, what leaves the vault) in sompi.
    async fn quote_send(&self, amount_kas: f64, to: &str) -> Result<(u64, &'static str, u64), String> {
        let v = self.open_vault()?;
        if v.state.stage != 0 {
            return Err("A withdrawal is already waiting. Finish it first.".into());
        }
        let ctx = &self.ctx;
        let amount = sompi(amount_kas);
        let to_script = ctx.network.try_to_spk(to)?;
        let coin = ctx.try_vault_coin(&v).await?;
        if amount >= coin.1.amount {
            return Err("The account doesn't hold that much.".into());
        }
        let lock_time = ctx.try_daa().await?.saturating_sub(20);
        let in_daa = coin.1.block_daa_score as i64;
        // Without the secret here (the phone quotes before the fingerprint) the move is priced over the shadow vault.
        let (ctx, coin, spend_key) = self.sized(&coin, &v.state, pick_spend);
        let ctx = &ctx;
        // Spend (commands.rs do_spend): the ceiling is the vault's allowance or a tenth of the payment.
        let spend = ctx.quote(std::slice::from_ref(&coin.1), Some((v.dials.max_fee as u64).max(amount / 10)), &|fee| {
            let outflow = (amount + fee) as i64;
            let next = if lock_time as i64 >= v.state.window_start + v.dials.window && lock_time as i64 >= in_daa + v.dials.window {
                State { window_start: lock_time as i64, spent: outflow, ..v.state.clone() }
            } else {
                State { spent: v.state.spent + outflow, ..v.state.clone() }
            };
            if next.spent > v.dials.cap {
                return Err(format!("{OVER_CAP}: {} > {} sompi in the current window", next.spent, v.dials.cap));
            }
            let left = coin.1.amount.checked_sub(amount + fee).ok_or("The account doesn't hold that much.")?;
            if left < v.dials.min_remainder as u64 {
                return Err(format!("{LOW_REMAINDER}: {left} < {} sompi", v.dials.min_remainder));
            }
            let outputs = vec![TransactionOutput::new(amount, to_script.clone()), ctx.continuation(left, &next, &v)];
            ctx.vault_tx(&v, &coin, "spend", Some(&spend_key), &[], outputs, 0, lock_time)
        });
        match spend {
            Ok(q) => return Ok((q.fee, "now", amount + q.fee)),
            Err(FeeError::Build(e)) if e.starts_with(OVER_CAP) || e.starts_with(LOW_REMAINDER) => {}
            Err(e) => return Err(e.to_string()),
        }
        // Withdrawal with notice (commands.rs do_request).
        if self.ctx.keys.public().hard_limit {
            return Err(HARD_LIMIT.into()); // v6: the quote says it like the send
        }
        if (amount as i64) < v.dials.min_remainder {
            return Err(format!("A withdrawal with notice must be at least {} KAS on this account.", kas(v.dials.min_remainder as u64)));
        }
        let next = State { stage: 1, dest_hash: spk_hash(&to_script), amount: amount as i64, ..v.state.clone() };
        let args = vec![V::Bytes(spk_bytes(&to_script)), V::Int(amount as i64)];
        let q = ctx
            .quote(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
                let left = coin.1.amount.checked_sub(fee).ok_or("The account cannot pay the fee.")?;
                let after = left.checked_sub(amount).ok_or("The account doesn't hold that much.")?;
                crate::commands::request_leftover_check(&v.dials, after)?;
                ctx.vault_tx(&v, &coin, "request", Some(&spend_key), &args, vec![ctx.continuation(left, &next, &v)], 0, 0)
            })
            .map_err(|e| e.to_string())?;
        Ok((q.fee, "pending", amount + q.fee))
    }

    /// Prices the finish of the waiting withdrawal (commands.rs do_finish), whatever the notice period says.
    async fn quote_finish(&self) -> Result<(u64, &'static str, u64), String> {
        let v = self.open_vault()?;
        if v.state.stage != 1 {
            return Err("There is no withdrawal waiting to finish.".into());
        }
        let ctx = &self.ctx;
        let coin = ctx.try_vault_coin(&v).await?;
        let dest_script = ctx.network.try_to_spk(v.pending_to.as_deref().ok_or(RECORDS_UNREADABLE)?)?;
        let amount = v.state.amount as u64;
        let remainder = coin.1.amount.checked_sub(amount).ok_or("The account holds less than the waiting withdrawal.")?;
        let next = after_finish(&v.state);
        // The contract's rule: the rest stays as a coin of at least minRemainder, or burns as the whole fee
        // when it is at most maxFee.
        let burn = remainder <= v.dials.max_fee as u64;
        let q = ctx
            .quote(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
                let mut outputs = vec![TransactionOutput::new(amount, dest_script.clone())];
                if burn {
                    if remainder < fee {
                        return Err(format!("finishing now would leave {:.5} KAS for a network fee of {:.5} KAS. Cancel to move everything to your safe.", kas(remainder), kas(fee)));
                    }
                } else {
                    let left = remainder - fee;
                    if left < v.dials.min_remainder as u64 {
                        return Err(format!(
                            "finishing now would leave {:.5} KAS in the vault, less than the {} KAS it must keep. Cancel to move everything to your safe.",
                            kas(left),
                            kas(v.dials.min_remainder as u64)
                        ));
                    }
                    outputs.push(ctx.continuation(left, &next, &v));
                }
                ctx.vault_tx(&v, &coin, "finish", None, &[], outputs, v.dials.delay as u64, 0)
            })
            .map_err(|e| e.to_string())?;
        let paid = if burn { remainder } else { q.fee };
        Ok((paid, "finish", amount + paid))
    }

    /// Prices the emergency stop (commands.rs do_cancel with stop key 1), lift coin included.
    async fn quote_stop(&self) -> Result<(u64, &'static str, u64), String> {
        let v = self.open_vault()?;
        let ctx = &self.ctx;
        let slot = stop_slot(1)?;
        let coin = ctx.try_vault_coin(&v).await?;
        let lift = self.lift_for(coin.1.amount).await?;
        let (ctx, coin, stop_key) = self.sized(&coin, &v.state, pick_stop);
        let ctx = &ctx;
        let cold = ctx.cold_spk();
        let mut entries = vec![coin.1.clone()];
        if let Some((c, _)) = &lift {
            entries.push(c.1.clone());
        }
        let total = coin.1.amount + lift.as_ref().map_or(0, |(c, _)| c.1.amount);
        let q = ctx
            .quote(&entries, Some(v.dials.max_fee as u64), &|fee| {
                let left = total.checked_sub(fee).ok_or("The account cannot pay the fee.")?;
                if left < v.dials.min_remainder as u64 {
                    return Err(format!("stopping now would move {:.5} KAS, less than the {} KAS an exit must carry", kas(left), kas(v.dials.min_remainder as u64)));
                }
                let outputs = vec![TransactionOutput::new(left, cold.clone())];
                ctx.vault_tx_lifted(&v, &coin, lift.as_ref().map(|(c, k)| (c, k)), "cancel", Some(&stop_key), &[V::Int(slot as i64)], outputs, 0, 0)
            })
            .map_err(|e| e.to_string())?;
        Ok((q.fee, "stop", total))
    }

    /// Prices the decline of the waiting withdrawal (commands.rs do_decline with stop key 1): the fee is all that moves.
    async fn quote_decline(&self) -> Result<(u64, &'static str, u64), String> {
        let v = self.open_vault()?;
        if v.state.stage != 1 {
            return Err(NOTHING_WAITING.into());
        }
        let coin = self.ctx.try_vault_coin(&v).await?;
        let (ctx, coin, stop_key) = self.sized(&coin, &v.state, pick_stop);
        let q = ctx.quote(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| decline_tx(&ctx, &v, &coin, 1, fee, &stop_key)).map_err(|e| e.to_string())?;
        Ok((q.fee, "decline", q.fee))
    }

    /// Mirrors commands.rs lift_for (the smallest funding coin that carries a vault too small to pay
    /// its own way out), but this service never holds the funding key: run() drops it and no route
    /// reads it back from disk. GET /api/quote?kind=stop is the only path here, and the stop itself
    /// (do_cancel) runs without it too, so a vault below the floor is a refusal saying what to do.
    async fn lift_for(&self, have: u64) -> Result<Option<(Coin, Keypair)>, String> {
        let d = self.ctx.dials()?;
        if have >= d.floor() {
            return Ok(None);
        }
        let short = d.floor() - have;
        let too_small = format!(
            "this vault has {:.5} KAS to move, too little to move on its own (it needs {} KAS to pay the network fee and still leave {} KAS at the exit)",
            kas(have),
            kas(d.floor()),
            kas(d.min_remainder as u64)
        );
        debug_assert!(self.ctx.keys.funding.is_none(), "serve never holds the funding key");
        Err(format!("{too_small}, and the funding key is not on this device to carry it (it needs {:.5} KAS more). Add to the vault first.", kas(short)))
    }
}

/// The decline transaction at `fee`: the shape of commands.rs do_decline's closure (kept in step
/// with it): one continuation carrying `after_decline` and the whole coin minus the fee, signed by
/// `stop_key` as stop key `which` (1, 2 or 3). From Locked there is nothing to decline.
fn decline_tx(ctx: &Ctx, v: &Vault, coin: &Coin, which: u8, fee: u64, stop_key: &Keypair) -> Result<Transaction, String> {
    if v.state.stage != 1 {
        return Err(NOTHING_WAITING.into());
    }
    let slot = stop_slot(which)?;
    let left = coin.1.amount.checked_sub(fee).ok_or("The account cannot pay the fee.")?;
    if left < v.dials.min_remainder as u64 {
        return Err(format!(
            "declining now would leave {:.5} KAS in the vault, less than the {} KAS it must keep. Cancel to move everything to your safe.",
            kas(left),
            kas(v.dials.min_remainder as u64)
        ));
    }
    let outputs = vec![ctx.continuation(left, &after_decline(&v.state), v)];
    // The contract counts stop slots from 0; the phone and the command line from 1.
    ctx.vault_tx(v, coin, "decline", Some(stop_key), &[V::Int(slot as i64)], outputs, 0, 0)
}

/// The guard wants two witnesses on different hosts; a fresh setup may have typed none or one, so
/// the rest come from the secure resolver, asked at most this many times. The resolver hands out
/// its least-busy node, so quick asks often repeat one host (testnet: about half the answers):
/// asks are spaced by GUARD_FILL_GAP and the whole fill stops at GUARD_FILL_TOTAL.
const GUARD_FILL_ASKS: usize = 16;
/// How long one resolver ask may take.
const GUARD_FILL_WAIT: Duration = Duration::from_secs(5);
/// The pause after each resolver ask, so the next one can land on another node.
const GUARD_FILL_GAP: Duration = Duration::from_millis(1500);
/// The whole fill, every ask included.
const GUARD_FILL_TOTAL: Duration = Duration::from_secs(60);
const GUARD_NO_SECOND_NODE: &str =
    "The guard could not find a second network node. Check the internet connection and try again.";

/// The host of a node url, lowercased, without scheme, sign-in part or port: two urls on one host
/// are one witness, not two.
fn node_host(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower.split_once("://").map_or(lower.as_str(), |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    let host = host.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_string())
}

/// The different hosts among `nodes`, in order.
fn distinct_hosts(nodes: &[String]) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for host in nodes.iter().filter_map(|n| node_host(n)) {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// Fills `nodes` up to two different hosts by asking `ask` (the secure resolver) at most
/// GUARD_FILL_ASKS times; only a wss:// answer on a host not yet listed is taken. Answers the full
/// list and the urls that were added, or GUARD_NO_SECOND_NODE.
async fn fill_guard_nodes<F, Fut>(
    mut nodes: Vec<String>,
    mut ask: F,
) -> Result<(Vec<String>, Vec<String>), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut hosts = distinct_hosts(&nodes);
    let mut filled = Vec::new();
    for _ in 0..GUARD_FILL_ASKS {
        if hosts.len() >= 2 {
            break;
        }
        let Ok(answer) = ask().await else { continue };
        let url = answer.trim().to_string();
        let wss = url.to_ascii_lowercase().starts_with("wss://");
        let secure = wss && check_secure_url(&url).is_ok();
        match node_host(&url) {
            Some(host) if secure && !hosts.contains(&host) => {
                hosts.push(host);
                filled.push(url.clone());
                nodes.push(url);
            }
            _ => {}
        }
    }
    if hosts.len() < 2 {
        return Err(GUARD_NO_SECOND_NODE.into());
    }
    Ok((nodes, filled))
}

/// One ask of the secure resolver for a node of this network, GUARD_FILL_WAIT at most.
async fn resolver_node(resolver: &Resolver, net: &Network) -> Result<String, String> {
    let ask = resolver.get_url(WrpcEncoding::Borsh, net.resolver_id());
    let answer = match tokio::time::timeout(GUARD_FILL_WAIT, ask).await {
        Ok(Ok(url)) => Ok(url),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("the resolver did not answer in time".into()),
    };
    tokio::time::sleep(GUARD_FILL_GAP).await;
    answer
}

/// A status answer without its network time: a cached or stale answer must not pass an old DAA score
/// on as now (the phone's fingerprint-change wait would start too early).
pub(crate) fn without_daa_now(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("daa_now");
    }
    v
}

#[cfg(test)]
mod tests {
    /// A rule block holding uppercase vault keys (check_foreign accepts them) finds each slot.
    #[test]
    fn cold_slot_matches_uppercase_vault_keys() {
        let k = super::Keys::fresh(&crate::network::Network::testnet());
        let mut p = k.public();
        p.cold_x = p.cold_x.iter().map(|x| x.to_ascii_uppercase()).collect();
        assert_eq!(super::cold_slot(&p, &k.cold.unwrap()), Some(0));
        assert_eq!(super::cold_slot(&p, &k.cold3.unwrap()), Some(2));
    }

    use super::*;
    use crate::serve::CARD_EXPIRED;

    #[test]
    fn node_host_compares_hosts_not_urls() {
        assert_eq!(node_host("wss://A.example:443/x").as_deref(), Some("a.example"));
        assert_eq!(node_host(" wss://me@b.example ").as_deref(), Some("b.example"));
        assert_eq!(node_host("ws://127.0.0.1:17110").as_deref(), Some("127.0.0.1"));
        assert_eq!(node_host("wss://[::1]:443").as_deref(), Some("::1"));
        assert_eq!(node_host("wss://c.example.:443").as_deref(), Some("c.example"));
        assert_eq!(node_host("wss://"), None);
        let same = vec!["wss://a.example:443".to_string(), "wss://A.example:444/p".to_string()];
        assert_eq!(distinct_hosts(&same), vec!["a.example".to_string()]);
    }

    #[tokio::test]
    async fn guard_fill_takes_only_new_secure_hosts() {
        let mut answers = VecDeque::from([
            "ws://plain.example:17110", "wss://a.example:443",
            "wss://a.example:444", "wss://b.example",
        ]);
        let mut asks = 0;
        let ask = || {
            asks += 1;
            let next = answers.pop_front().map(str::to_string);
            async move { next.ok_or_else(|| "none".to_string()) }
        };
        let (nodes, filled) = fill_guard_nodes(Vec::new(), ask).await.unwrap();
        assert_eq!(filled, vec!["wss://a.example:443".to_string(), "wss://b.example".to_string()]);
        assert_eq!(nodes, filled);
        assert_eq!(asks, 4);
    }

    #[tokio::test]
    async fn guard_fill_keeps_typed_nodes_and_asks_nothing_when_enough() {
        let typed = vec!["wss://a.example".to_string(), "wss://b.example".to_string()];
        let ask = || async { panic!("no ask needed") };
        let (nodes, filled) = fill_guard_nodes(typed.clone(), ask).await.unwrap();
        assert_eq!((nodes, filled), (typed, Vec::new()));
        let one = vec!["wss://a.example:443".to_string(), "wss://a.example:444".to_string()];
        let ask = || async { Ok("wss://c.example".to_string()) };
        let (nodes, filled) = fill_guard_nodes(one, ask).await.unwrap();
        assert_eq!(filled, vec!["wss://c.example".to_string()]);
        assert_eq!(nodes.len(), 3);
    }

    #[tokio::test]
    async fn guard_fill_refuses_after_bounded_asks() {
        let mut asks = 0;
        let ask = || {
            asks += 1;
            async { Ok("wss://only.example".to_string()) }
        };
        let typed = vec!["wss://only.example:443".to_string()];
        assert_eq!(fill_guard_nodes(typed, ask).await.unwrap_err(), GUARD_NO_SECOND_NODE);
        assert_eq!(asks, GUARD_FILL_ASKS);
        let mut asks = 0;
        let ask = || {
            asks += 1;
            async { Err::<String, String>("offline".into()) }
        };
        assert_eq!(fill_guard_nodes(Vec::new(), ask).await.unwrap_err(), GUARD_NO_SECOND_NODE);
        assert_eq!(asks, GUARD_FILL_ASKS);
    }
    use std::sync::Arc;
    use kaspa_consensus_core::Hash;
    use kaspa_consensus_core::tx::{ScriptPublicKey, TransactionOutpoint, UtxoEntry};

    /// v5: the service's decline is one continuation bound to the covenant id and carrying
    /// after_decline (Locked, destination wiped, window counters kept); from Locked it refuses
    /// with the client's words, and a stop key that does not exist is a refusal too.
    #[test]
    fn decline_is_one_continuation_or_nothing_waiting() {
        let dir = std::env::temp_dir().join(format!("kv-serve-decline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::contract::pin_source(&dir).unwrap();
        let ctx = Ctx { dir: dir.clone(), keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        let cov = Hash::from_bytes([3u8; 32]);
        let coin_of = |state: &State| -> Coin { (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(3 * KAS, ctx.vault_spk(state), 1_000, false, Some(cov))) };
        let idle = Vault { covenant_id: cov, state: locked(), pending_to: None, history: vec![], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        assert_eq!(decline_tx(&ctx, &idle, &coin_of(&idle.state), 1, 1000, &ctx.keys.try_stop_key(0).unwrap()).unwrap_err(), NOTHING_WAITING);
        let dest = ScriptPublicKey::from_vec(0, vec![0xbb; 34]);
        let state = State { stage: 1, dest_hash: spk_hash(&dest), amount: KAS as i64, window_start: 7, spent: 5 };
        let v = Vault { state: state.clone(), pending_to: Some("kaspatest:q".into()), ..idle };
        let coin = coin_of(&state);
        let tx = decline_tx(&ctx, &v, &coin, 1, 1000, &ctx.keys.try_stop_key(0).unwrap()).unwrap();
        assert_eq!(tx.inputs.len(), 1, "the vault coin alone pays the fee");
        assert_eq!(tx.outputs.len(), 1, "nothing leaves: one continuation");
        let next = after_decline(&state);
        assert_eq!(next, State { stage: 0, dest_hash: vec![0; 32], amount: 0, window_start: 7, spent: 5 });
        assert_eq!(tx.outputs[0], ctx.continuation(3 * KAS - 1000, &next, &v), "bound to the covenant id with the Locked script");
        assert!(decline_tx(&ctx, &v, &coin, 2, 1000, &ctx.keys.try_stop_key(1).unwrap()).is_ok(), "any of the three stop keys may decline");
        assert!(decline_tx(&ctx, &v, &coin, 4, 1000, &ctx.keys.try_stop_key(0).unwrap()).unwrap_err().contains("1, 2 or 3"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plain_words_hide_node_text() {
        assert_eq!(plain("spend rejected by node: RejectedTransaction(x) at /home/x/node"), NETWORK_REFUSED);
        assert_eq!(plain("preflight: input 0 fails the script engine (pass 1): Foo"), NETWORK_REFUSED);
        assert_eq!(plain("get_utxos_by_addresses: RPC transport closed 10.0.0.5:17210"), NETWORK_DOWN);
        assert_eq!(plain("Can't reach the network right now: connection refused"), NETWORK_DOWN);
        assert_eq!(plain("state.json: The system cannot find the file (os error 2)"), RECORDS_UNREADABLE);
        assert_eq!(plain("spend rejected by node: sequence locks not satisfied"), "Too early: the notice period has not passed yet.");
        assert_eq!(plain(BUSY), BUSY, "the service's own words pass through");
        assert_eq!(plain("The account doesn't hold that much."), "The account doesn't hold that much.");
        assert_eq!(plain("pay submit answer lost: WebSocket timeout; then the check failed: rpc down"), ANSWER_LOST);
        assert_eq!(plain("pay submit answer lost: tx ab: RPC Server (remote error) -> RPC response error Text(\"Rejected transaction ab: transaction ab is already in the mempool\"); then x"), ANSWER_LOST);
        assert_eq!(plain("pay rejected by node: RPC Server (remote error) -> RPC response error Text(\"Rejected transaction ab: one of the transaction sequence locks conditions was not met\")"), "Too early: the notice period has not passed yet.");
        assert_eq!(plain("pay rejected by node: RPC Server (remote error) -> RPC response error Text(\"Rejected transaction ab: already spent\")"), NETWORK_REFUSED);
        assert_eq!(plain("pay not sent. Can't reach the network right now: RPC Server (remote error) -> WebSocket -> WebSocket is not connected"), NETWORK_DOWN);
        for w in [NETWORK_REFUSED, NETWORK_DOWN, RECORDS_UNREADABLE, CARD_EXPIRED, ANSWER_LOST] {
            assert!(!w.contains(':') && !w.contains('/'), "{w} carries no path or address");
        }
    }

    #[test]
    fn update_now_is_offered_only_with_the_safe_keys_on_the_phone() {
        let dir = std::env::temp_dir().join(format!("kv-now-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx { dir: dir.clone(), keys: Keys::default(), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        let off = now_option(&ctx, 200_000);
        assert_eq!((off["ok"].clone(), off["why"].as_str().is_some()), (json!(false), true), "{off}");
        std::fs::write(dir.join(SHEETS_ON_PHONE_FILE), b"").unwrap();
        let on = now_option(&ctx, 200_000);
        assert_eq!((on["ok"].clone(), on["why"].clone(), on["steps"].as_array().unwrap().len()), (json!(true), Value::Null, 3), "{on}");
        assert_eq!(on["fee_kas_about"], json!(kas(600_000)));
        // The take-out of the stop's own coin reads the stop from this account's record: open or missing refuses.
        assert_eq!(tx_of("cancel (stop key 1): everything to cold, fee 0.00200 KAS, tx abc123"), Some("abc123"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn protection_pips_and_closed_history() {
        let p = |cancel2: &str, cancel3: &str, heir: &str, sheets: usize| -> Public {
            serde_json::from_value(json!({
                "spend_x": "aa", "cancel_x": "01", "cold_address": "kaspatest:q", "cold_x": vec!["c1"; sheets],
                "cancel2_x": cancel2, "cancel3_x": cancel3, "heir_x": heir, "heir_address": "",
            }))
            .unwrap()
        };
        let full = (vec![], true); // a list kept since setup: every other key is a guardian's
        assert_eq!(protection(&p("", "", "", 0), &full, &[]), (1, false, 1), "legacy vault: slots repeat the first stop key");
        assert_eq!(protection(&p("02", "03", "", 3), &full, &[]), (3, false, 3));
        assert_eq!(protection(&p("02", "02", "ee", 3), &full, &[]), (2, true, 3));
        assert_eq!(protection(&p("02", "03", "", 3), &(vec!["02".into(), "03".into()], true), &[]), (1, false, 3), "a fresh phone: both other slots unheld");
        assert_eq!(protection(&p("02", "03", "", 3), &(vec!["02".into()], true), &[]), (2, false, 3), "one guardian, one unheld");
        assert_eq!(protection(&p("02", "03", "", 3), &(vec!["02".into()], false), &[]), (1, false, 3), "a removed guardian (unheld) and a slot from before the list (unknown)");
        let paper = vec!["03".to_string()];
        assert_eq!(protection(&p("02", "03", "", 3), &(vec!["02".into()], false), &paper), (2, false, 3), "a paper stop counts even on an old key set");
        assert_eq!(stopped_status(&Network::testnet(), &Dials::testnet(), &p("02", "03", "ee", 3), &full, &[], 0, vec![])["sheets"], 3);

        let vault = |last: &str| Vault { covenant_id: Hash::default(), state: locked(), pending_to: None, history: vec![last.into()], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        assert!(is_stopped(&vault("cancel 2 tx abc")));
        assert!(is_stopped(&vault("inherit tx abc")));
        assert!(!is_stopped(&vault("request 0.3 tx abc")));
    }

    #[test]
    fn status_carries_the_watcher_file_or_leaves_the_fields_out() {
        let dir = std::env::temp_dir().join(format!("kv-serve-alerts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_alerts(&dir), Ok(None), "no watcher file: the fields stay out");
        std::fs::write(dir.join(ALERTS_FILE), b"{not json").unwrap();
        assert!(read_alerts(&dir).unwrap_err().starts_with(ALERTS_FILE));
        std::fs::write(dir.join(ALERTS_FILE), b"[1]").unwrap();
        assert!(read_alerts(&dir).unwrap_err().contains("not a JSON object"));
        let mut alerts: Vec<Value> = (0..12).map(|i| json!({"at": 1000 + i, "kind": "watcher_silent", "text": format!("quiet {i}")})).collect();
        alerts.reverse();
        std::fs::write(dir.join(ALERTS_FILE), json!({"seen_at": 5000, "alerts": alerts}).to_string()).unwrap();
        let file = read_alerts(&dir).unwrap().unwrap();
        let s = with_watcher(json!({"pending": null, "stopped": false}), &file, 5010);
        assert_eq!(s["watcher_seen_at"], 5000);
        assert_eq!(s["alerts"].as_array().unwrap().len(), ALERTS_SHOWN);
        assert_eq!(s["alerts"][0], json!({"at": 1011, "kind": "watcher_silent", "text": "quiet 11"}), "newest first, verbatim");
        assert_eq!(s["alerts"][9]["at"], 1002);
        assert!(s["pending"].is_null(), "no stage alert: pending is state.json's");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pending_follows_a_fresh_watcher() {
        let thief = json!({"seen_at": 5000, "alerts": [{"at": 4990, "kind": "withdrawal_pending", "text": "A withdrawal was started."}]});
        let s = with_watcher(json!({"pending": null}), &thief, 5010);
        assert_eq!(s["pending"]["watcher"], true);
        assert_eq!(s["pending"]["text"], "A withdrawal was started.");
        // the guard went quiet: state.json stands and the phone shows the quiet banner instead
        assert!(with_watcher(json!({"pending": null}), &thief, 5000 + WATCHER_FRESH + 1)["pending"].is_null());
        assert!(with_watcher(json!({"pending": null}), &json!({"seen_at": null, "alerts": thief["alerts"]}), 5010)["pending"].is_null());
        let mine = json!({"to": "kaspatest:q", "amount_kas": 0.5});
        let done = json!({"seen_at": 5000, "alerts": [{"at": 4995, "kind": "withdrawal_completed", "text": "done"}, {"at": 4990, "kind": "withdrawal_pending", "text": "x"}]});
        assert!(with_watcher(json!({"pending": mine.clone()}), &done, 5010)["pending"].is_null(), "the coin is gone: nothing pending");
        let stopped = json!({"seen_at": 5000, "alerts": [{"at": 4990, "kind": "withdrawal_pending", "text": "x"}, {"at": 4995, "kind": "stopped", "text": "s"}]});
        assert!(with_watcher(json!({"pending": mine.clone()}), &stopped, 5010)["pending"].is_null(), "sorted by `at`, not file order");
        let declined = json!({"seen_at": 5000, "alerts": [{"at": 4995, "kind": "withdrawal_declined", "text": "d"}, {"at": 4990, "kind": "withdrawal_pending", "text": "x"}]});
        assert!(with_watcher(json!({"pending": mine.clone()}), &declined, 5010)["pending"].is_null(), "v5: a decline clears a pending the watcher saw");
        assert_eq!(with_watcher(json!({"pending": mine.clone()}), &thief, 5010)["pending"], mine, "both agree: state.json's detail stays");
    }

    /// A full quote needs a node (the vault coin, the DAA); the refusal wording, the answer shape,
    /// the query parsing and the fee read-back are checked without one.
    #[test]
    fn quote_refusals_are_the_clients_bank_words() {
        let over = FeeError::OverCeiling { need: 7_000_000, ceiling: MAX_FEE as u64 }.to_string();
        let r = quote_result(Err(over));
        assert_eq!(r["ok"], false);
        assert_eq!(r["error"], "the network fee would be 0.07000 KAS, more than the 0.05 KAS this move allows");
        assert_eq!(quote_result(Err(FeeError::NotSettled.to_string()))["error"], "The network charge could not be worked out. Try again in a moment.");
        let r = quote_result(Ok((250_000, "now", 50_250_000)));
        assert_eq!((r["ok"].clone(), r["kind"].clone(), r["fee_sompi"].clone()), (json!(true), json!("now"), json!(250_000)));
        assert_eq!(r["fee_kas"], 0.0025);
        assert_eq!(r["total_kas"], 0.5025);
        let q = query_pairs("kind=send&amount=0.5&to=kaspatest%3Aqabc+def&junk");
        assert_eq!((q["kind"].as_str(), q["amount"].as_str(), q["to"].as_str()), ("send", "0.5", "kaspatest:qabc def"));
        assert_eq!(percent_decode("a%zz%4"), "a%zz%4", "bad escapes pass through");
        assert_eq!(fee_of("spend 0.5 KAS to kaspatest:q, fee 0.00250 KAS, tx abc"), Some(0.0025));
        assert_eq!(fee_of("cancel (stop key 1): everything to cold, fee 0.00031 KAS, tx abc"), Some(0.00031));
        assert_eq!(fee_of("fund 2 KAS tx abc"), None);
        assert_eq!(result(Ok(("abc".into(), Some(0.0025))))["fee_kas"], 0.0025);
        assert_eq!(send_result(Err(BUSY.into()))["fee_kas"], Value::Null);
    }

    /// D6: guard.export writes a secret-free file and guard.import rebuilds the three files a keyless guard
    /// needs (public keys.json, anchored state.json, the pinned vault.sil by hash) at the same vault address.
    #[tokio::test]
    async fn guard_export_import_round_trip_restores_the_three_files() {
        let net = Network::testnet();
        let a = std::env::temp_dir().join(format!("kv-core-guard-a-{}", std::process::id()));
        let b = std::env::temp_dir().join(format!("kv-core-guard-b-{}", std::process::id()));
        for d in [&a, &b] {
            std::fs::remove_dir_all(d).ok();
        }
        std::fs::create_dir_all(&a).unwrap();
        let full = Keys::fresh(&net);
        full.save(&a.join("keys.json"), true).unwrap();
        let sha = crate::contract::pin_source(&a).unwrap();
        let state = State { stage: 1, dest_hash: vec![0xab; 32], amount: 2 * KAS as i64, window_start: 7, spent: 5 };
        let v = Vault { covenant_id: Hash::from_bytes([9u8; 32]), state: state.clone(), pending_to: Some("kaspatest:q".into()), history: vec!["fund".into()], contract_sha256: Some(sha.clone()), network: net.name.into(), dials: Dials::testnet() };
        save_vault_anchored(&a.join("state.json"), &v, &full.public().sha256()).unwrap();
        let out = guard_export(&a, &net, None, &["ws://a:1".into(), "ws://b:1".into()], None).unwrap();
        let text = std::fs::read_to_string(a.join(GUARD_FILE)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), out);
        assert_eq!((out["kind"].as_str(), out["v"].as_u64(), out["contract_sha256"].as_str()), (Some(GUARD_KIND), Some(1), Some(sha.as_str())));
        assert!(out["vault"].get("history").is_none() && out["vault"].get("pending_to").is_none(), "the record travels without its history");
        for secret in ["spend", "cancel", "cold", "funding", "heir"] {
            assert!(out.get(secret).is_none() && out["public"].get(secret).is_none(), "{secret} must not travel");
        }
        let back = guard_import(&b, &net, &text).unwrap();
        for f in ["keys.json", "state.json", crate::contract::SOURCE_FILE] {
            assert!(b.join(f).exists(), "{f} restored");
        }
        assert_eq!(sha256_hex(&std::fs::read(b.join(crate::contract::SOURCE_FILE)).unwrap()), sha);
        let kb = Keys::load_public(&b.join("keys.json")).unwrap();
        assert!(!kb.has_secret() && kb.public().sha256() == full.public().sha256());
        let ctx_a = Ctx { dir: a.clone(), keys: full.clone(), rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 };
        let ctx_b = Ctx { dir: b.clone(), keys: kb, rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 };
        let vb = ctx_b.try_load_vault().unwrap().unwrap();
        assert_eq!((vb.covenant_id, &vb.state, vb.contract_sha256.as_deref()), (v.covenant_id, &state, Some(sha.as_str())));
        assert_eq!(back["address"], ctx_a.vault_address(&state).to_string());
        assert_eq!(ctx_b.try_vault_address(&state).unwrap().to_string(), back["address"]);
        // a folder holding a wallet's own keys never takes a guard import; a stranger's hash is refused
        assert_eq!(guard_import(&a, &net, &text).unwrap_err(), FOLDER_HAS_KEYS);
        assert!(crate::contract::restore_source(&b, "0000").unwrap_err().contains(UNKNOWN_RULES));
        assert_eq!(guard_import(&b, &net, "not json").unwrap_err(), NOT_GUARD_FILE);
        // guard.export {account: 2} sends state-2.json; no account sends state.json; a bad account refuses.
        let v2 = Vault { covenant_id: Hash::from_bytes([8u8; 32]), ..v.clone() };
        save_vault_anchored(&a.join(state_file_name(2)), &v2, &full.public().sha256()).unwrap();
        let core = Core::open(Ctx { keys: Keys::load_public(&a.join("keys.json")).unwrap(), ..ctx_a.clone() }).unwrap();
        let (_, two) = core.call("guard.export", &json!({"account": 2})).await;
        assert_eq!(two["vault"]["covenant_id"], serde_json::to_value(v2.covenant_id).unwrap(), "{two}");
        let (_, one) = core.call("guard.export", &json!({})).await;
        assert_eq!(one["vault"]["covenant_id"], serde_json::to_value(v.covenant_id).unwrap(), "{one}");
        assert_eq!(core.call("guard.export", &json!({"account": 99})).await.1["ok"], json!(false));
        for d in [&a, &b] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// guard.status on a folder with no alerts.json and no guard started: not running, nothing seen.
    #[test]
    fn guard_status_without_a_guard_or_alerts_file() {
        let dir = std::env::temp_dir().join(format!("kv-core-guard-status-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = guard_status(&dir);
        assert_eq!((s["running"].as_bool(), &s["seen_at"], &s["exited"], &s["started"]), (Some(false), &Value::Null, &Value::Null, &Value::Null));
        assert_eq!(s["alerts"], json!([]));
        std::fs::write(dir.join(ALERTS_FILE), json!({"seen_at": 77, "alerts": [{"at": 70, "kind": "watcher_silent", "text": "quiet"}]}).to_string()).unwrap();
        let s = guard_status(&dir);
        assert_eq!((s["seen_at"].as_u64(), s["alerts"].as_array().map(Vec::len)), (Some(77), Some(1)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn poisoned_lock_still_serves() {
        let m = Arc::new(Mutex::new(1u32));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(m.is_poisoned());
        *guard(&m) += 1;
        assert_eq!(*guard(&m), 2);
    }

    /// A reopen after a stop keeps the account's own rules (not the network preset); a waiting rules.change wins.
    #[test]
    fn a_reopen_keeps_the_accounts_own_rules() {
        let dir = std::env::temp_dir().join(format!("kv-core-reopen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx { dir: dir.clone(), keys: Keys::default(), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        assert_eq!(opening_dials(&ctx, &json!({})).unwrap(), Dials::testnet(), "first opening: the preset");
        let own = Dials { cap: 7 * KAS as i64, ..Dials::testnet() };
        let v = Vault { covenant_id: Hash::from_bytes([4u8; 32]), state: locked(), pending_to: None, history: vec!["cancel tx a".into()], contract_sha256: None, network: "testnet".into(), dials: own.clone() };
        std::fs::write(dir.join("state.json"), serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(opening_dials(&ctx, &json!({})).unwrap(), own, "after a stop: the account's own rules");
        let next = Dials { cap: 9 * KAS as i64, ..Dials::testnet() };
        write_rules_next(&dir, 1, &next).unwrap();
        assert_eq!(opening_dials(&ctx, &json!({})).unwrap(), next, "a waiting rules.change wins");
        clear_rules_next(&dir, 1);
        // Account 2 reads its own record, never account 1's.
        let ctx2 = Ctx { account: 2, ..ctx.clone() };
        assert_eq!(opening_dials(&ctx2, &json!({})).unwrap(), Dials::testnet(), "account 2 has no record yet");
        // An opening that failed after the archive: the newest archived record of THIS account gives the rules.
        std::fs::create_dir_all(dir.join("closed")).unwrap();
        std::fs::rename(dir.join("state.json"), dir.join("closed").join("100-state.json")).unwrap();
        let older = Vault { dials: Dials::testnet(), ..v.clone() };
        std::fs::write(dir.join("closed").join("50-state.json"), serde_json::to_string(&older).unwrap()).unwrap();
        std::fs::write(dir.join("closed").join("200-state-2.json"), serde_json::to_string(&older).unwrap()).unwrap();
        assert_eq!(opening_dials(&ctx, &json!({})).unwrap(), own, "the newest archived record of account 1");
        assert_eq!(opening_dials(&ctx2, &json!({})).unwrap(), Dials::testnet(), "account 2's own archive");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn amounts_are_read_as_exact_sompi() {
        assert_eq!(parse_kas(&json!("0.1")), Some(10_000_000));
        assert_eq!(parse_kas(&json!(0.1)), Some(10_000_000));
        assert_eq!(parse_kas(&json!(" 2 ")), Some(2 * KAS));
        assert_eq!(parse_kas(&json!(".5")), Some(50_000_000));
        assert_eq!(parse_kas(&json!("1.")), Some(KAS));
        assert_eq!(parse_kas(&json!("0.00000001")), Some(1));
        assert_eq!(parse_kas(&json!("1000000000")), Some(1_000_000_000 * KAS));
        for bad in [json!("1.123456789"), json!("1e3"), json!("-1"), json!(0), json!("0"), json!("."), json!(""), json!("1000000000.00000001"), json!(null), json!(true), json!("1,5"),
                    json!("abc"), json!("-5"), json!(-5), json!("1e999"), json!(1e300), json!("NaN"), json!("inf"), json!(" "), json!([1]), json!({"kas": 1})] {
            assert_eq!(parse_kas(&bad), None, "{bad}");
        }
    }

    /// The engine is the real guard on a destination: bech32 checksum and network prefix, whatever the page let through.
    #[test]
    fn destinations_need_a_valid_address_on_this_network() {
        let (tn, mn) = (Network::testnet(), Network::mainnet());
        let good = "kaspatest:qqv72prqwp64unmtsa7fyrv4fk62axsfgp3p5j8wlak5wh9ztfs057fjn6jyj";
        assert!(tn.address(good).is_ok());
        let typo = format!("{}{}", &good[..good.len() - 1], if good.ends_with('j') { "k" } else { "j" });
        assert!(tn.address(&typo).is_err(), "a one-letter typo fails the checksum");
        assert!(mn.address(good).is_err(), "a testnet address on mainnet");
        for bad in ["", "kaspatest:", "kaspa:qqqq", "bitcoin:qqv72prqwp64unmtsa7fyrv4fk62axsfgp3p5j8wlak5wh9ztfs057fjn6jyj", "kaspatest:QQV72PRQWP64UNMTSA7FYRV4FK62AXSFGP3P5J8WLAK5WH9ZTFS057FJN6JYJ"] {
            assert!(tn.address(bad).is_err(), "{bad}");
        }
        assert_eq!(no_keys()["no_keys"], json!(true));
    }

    fn purse_coins(amounts: &[u64]) -> Vec<Coin> {
        let spk = ScriptPublicKey::from_vec(0, vec![0xaa; 34]);
        let at = |i: usize| TransactionOutpoint::new(Hash::from_bytes([9u8; 32]), i as u32);
        let entry = |a: u64| UtxoEntry::new(a, spk.clone(), 100, false, None);
        let coin = |(i, a): (usize, &u64)| (at(i), entry(*a));
        amounts.iter().enumerate().map(coin).collect()
    }

    fn coins_ctx() -> Ctx {
        let net = Network::testnet();
        let keys = Keys::fresh(&net);
        let dir = std::env::temp_dir();
        let dials = Dials::testnet();
        Ctx { dir, keys, rpc: None, force: false, network: net, dials, account: 1 }
    }

    /// coins.info on an empty purse: nothing listed, nothing to combine.
    #[test]
    fn coins_info_empty_purse() {
        let a = coins_answer(&coins_ctx(), &[]);
        let n = |k: &str| a[k].as_u64();
        let got = (a["ok"].as_bool(), n("count"), n("total_sompi"), n("fee_sompi"));
        assert_eq!(got, (Some(true), Some(0), Some(0), Some(0)));
        assert_eq!(a["amounts_sompi"], json!([]));
    }

    /// coins.info lists the amounts largest first, totals them, and prices combining them.
    #[test]
    fn coins_info_sorts_and_totals() {
        let a = coins_answer(&coins_ctx(), &purse_coins(&[3 * KAS, 10 * KAS, KAS]));
        assert_eq!(a["amounts_sompi"], json!([10 * KAS, 3 * KAS, KAS]));
        assert_eq!((a["count"].clone(), a["total_sompi"].clone()), (json!(3), json!(14 * KAS)));
        let fee = a["fee_sompi"].as_u64().expect("priced");
        assert!(fee > 0 && fee < KAS, "fee {fee}");
    }

    /// coins.info lists at most COINS_LISTED amounts; count and total still cover every coin.
    #[test]
    fn coins_info_caps_the_list() {
        let amounts: Vec<u64> = (1..=250u64).map(|i| i * KAS).collect();
        let a = coins_answer(&coins_ctx(), &purse_coins(&amounts));
        let listed = a["amounts_sompi"].as_array().unwrap();
        assert_eq!((listed.len(), listed[0].as_u64()), (COINS_LISTED, Some(250 * KAS)));
        assert_eq!(a["count"].as_u64(), Some(250));
        assert_eq!(a["total_sompi"].as_u64(), Some(amounts.iter().sum::<u64>()));
    }

    /// D2: a quote on a device without the secret (the phone before the fingerprint) is priced over the
    /// shadow vault, passes the engine preflight there, and comes out at the real key's fee.
    #[test]
    fn sizing_quotes_equal_real_key_quotes() {
        let dir = std::env::temp_dir().join(format!("kv-core-sizing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::contract::pin_source(&dir).unwrap();
        let net = Network::testnet();
        let full = Keys::fresh(&net);
        let real = Ctx { dir: dir.clone(), keys: full.clone(), rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 };
        let cov = Hash::from_bytes([3u8; 32]);
        let dest = ScriptPublicKey::from_vec(0, vec![0xbb; 34]);
        let state = State { stage: 1, dest_hash: spk_hash(&dest), amount: KAS as i64, window_start: 7, spent: 5 };
        let v = Vault { covenant_id: cov, state: state.clone(), pending_to: Some("kaspatest:q".into()), history: vec![], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        let coin: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(3 * KAS, real.vault_spk(&state), 1_000, false, Some(cov)));
        // The phone's shape: the rule block only.
        let mut public_only = full.clone();
        public_only.wipe();
        let core = Core::open(Ctx { keys: public_only, ..real.clone() }).unwrap();
        let (sctx, scoin, stop) = core.sized(&coin, &state, pick_stop);
        assert_ne!(sctx.keys.public().cancel_x, full.public().cancel_x, "a shadow, not the real key");
        assert_eq!((sctx.keys.public().cold_x.len(), sctx.keys.public().v, scoin.1.amount), (3, full.public().v, coin.1.amount));
        let real_stop = real.keys.try_stop_key(0).unwrap();
        let decline = |ctx: &Ctx, coin: &Coin, k: &Keypair| ctx.quote(&[coin.1.clone()], Some(MAX_FEE as u64), &|fee| decline_tx(ctx, &v, coin, 1, fee, k)).unwrap().fee;
        assert_eq!(decline(&sctx, &scoin, &stop), decline(&real, &coin, &real_stop));
        let cancel = |ctx: &Ctx, coin: &Coin, k: &Keypair| {
            ctx.quote(&[coin.1.clone()], Some(MAX_FEE as u64), &|fee| {
                let outputs = vec![TransactionOutput::new(coin.1.amount - fee, ctx.cold_spk())];
                ctx.vault_tx(&v, coin, "cancel", Some(k), &[V::Int(0)], outputs, 0, 0)
            })
            .unwrap()
            .fee
        };
        assert_eq!(cancel(&sctx, &scoin, &stop), cancel(&real, &coin, &real_stop));
        // With the secret here (the laptop), `sized` is this vault itself.
        let mut laptop = full.clone();
        (laptop.heir, laptop.cold, laptop.cold2, laptop.cold3) = (None, None, None, None);
        let (rctx, rcoin, _) = Core::open(Ctx { keys: laptop, ..real.clone() }).unwrap().sized(&coin, &state, pick_stop);
        assert_eq!((rctx.keys.public().cancel_x, rcoin.1.script_public_key), (full.public().cancel_x, coin.1.script_public_key));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// D3: a secret armed for one call is in the armed copy alone and is wiped after it, on the refusal
    /// path too; the shared Core never holds it, and a block of the wrong size arms nothing.
    #[tokio::test]
    async fn armed_keys_are_wiped_after_an_error() {
        let dir = std::env::temp_dir().join(format!("kv-core-armed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let net = Network::testnet();
        let mut keys = Keys::fresh(&net);
        keys.wipe();
        let rpc = KaspaRpcClient::new(WrpcEncoding::Borsh, Some("ws://127.0.0.1:9"), None, Some(net.resolver_id()), None).unwrap();
        let core = Core::open(Ctx { dir: dir.clone(), keys, rpc: Some(rpc), force: false, network: net.clone(), dials: Dials::testnet(), account: 1 }).unwrap();
        let block = Zeroizing::new(vec![7u8; 64]);
        let mut armed = Armed::new(&core, Slot::Stop, &block).unwrap();
        assert_eq!((armed.core.ctx.keys.cancel, armed.core.ctx.keys.funding, armed.core.ctx.keys.spend), (Some([7u8; 32]), Some([7u8; 32]), None));
        // No node in the test: the gate refuses, as the phone would see it offline.
        let (status, body) = armed.core.call("stop", &json!({})).await;
        assert_eq!((status, &body["error"]), (503, &json!(NETWORK_DOWN)));
        armed.core.ctx.keys.wipe(); // what Drop does on every path
        assert!(!armed.core.ctx.keys.has_secret());
        assert!(!core.ctx.keys.has_secret(), "the shared Core never holds the secret");
        assert_eq!(core.call_with_secret("stop", &json!({}), Slot::Stop, Zeroizing::new(vec![1u8; 32])).await, (400, refusal(KEY_SIZE)));
        assert_eq!(core.call_with_secret("send", &json!({}), Slot::Card, Zeroizing::new(vec![1u8; 32])).await.0, 503);
        assert_eq!((Slot::of_method("fund"), Slot::of_method("decline"), Slot::of_method("status")), (Some(Slot::Funding), Some(Slot::Stop), None));
        // The helper's link: guardian and heir slots, link.code / link.read offline (no node here), heir words from the block.
        for (m, s) in [("watch.stop", Slot::Guardian), ("watch.decline", Slot::Guardian), ("watch.claim", Slot::Heir), ("link.heir_words", Slot::Heir), ("heir.pay", Slot::Heir)] {
            assert_eq!(Slot::of_method(m), Some(s), "{m}");
            assert!(s.serves(m) && s.accepts(32) && !s.accepts(64), "{m}");
        }
        assert!(Slot::Heir.block_from(&Keys::fresh(&net)).is_err(), "the heir key is never taken from keys.json");
        std::fs::remove_file(dir.join(crate::link::LINK_FILE)).ok();
        assert_eq!(core.call("link.code", &json!({})).await, (200, refusal(crate::link::NO_LINK)));
        let (secrets, link) = crate::link::make_link(&dir, &net, "Helper").unwrap();
        let code = core.call("link.code", &json!({})).await;
        assert_eq!((code.0, &code.1["heir_x"]), (200, &json!(link.heir_x)));
        let read = core.call("link.read", &json!({"text": code.1["code"]})).await;
        assert_eq!(read, (200, refusal(crate::link::LINK_THIS_PHONE)), "this phone's own code");
        let other = dir.join("other"); std::fs::create_dir_all(&other).unwrap(); // another phone
        crate::link::make_link(&other, &net, "Helper").unwrap();
        let code = crate::link::link_code(&other, &net).unwrap()["code"].clone();
        assert_eq!(core.call("link.read", &json!({"text": code})).await.1["ok"], json!(true));
        let words = core.call_with_secret("link.heir_words", &json!({}), Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await;
        assert_eq!((words.0, words.1["heir_words"].as_array().map(|w| w.len())), (200, Some(24)));
        assert_eq!(core.call_with_secret("link.heir_words", &json!({}), Slot::Heir, Zeroizing::new(secrets[..32].to_vec())).await, (200, refusal(crate::link::WRONG_HEIR)));
        assert_eq!(core.call_with_secret("watch.claim", &json!({}), Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await, (200, refusal(crate::watched::BAD_ID)));
        // heir.pay: only this phone's own heir key, a real address, before any node is asked.
        let a = json!({"to": link.heir_address});
        assert_eq!(core.call_with_secret("heir.pay", &a, Slot::Heir, Zeroizing::new(secrets[..32].to_vec())).await.1["error"], json!(crate::link::WRONG_HEIR));
        let a = json!({"to": "nowhere"});
        assert_eq!(core.call_with_secret("heir.pay", &a, Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await.1["error"], json!(plain(BAD_ADDRESS)));
        let a = json!({"to": link.heir_address, "kas": "lots"});
        assert_eq!(core.call_with_secret("heir.pay", &a, Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await.1["error"], json!(plain(BAD_AMOUNT)));
        assert!(!core.ctx.keys.has_secret() && core.ctx.keys.heir.is_none(), "the heir key never reached the Core");
        std::fs::remove_file(dir.join(crate::link::LINK_FILE)).ok();
        let full = Keys::fresh(&net);
        assert_eq!(Slot::Stop.block_from(&full).unwrap().len(), 64);
        assert!(Slot::Card.block_from(&core.ctx.keys).unwrap_err().contains("spend"));
        // a slot that is not the method's own arms nothing
        assert_eq!(core.call_with_secret("send", &json!({}), Slot::Stop, Zeroizing::new(vec![1u8; 64])).await, (400, refusal(WRONG_KEY)));
        assert_eq!(Slot::Topup.block_from(&full).unwrap().len(), 64);
        assert!(Slot::Sheets.block_from(&full).unwrap_err().contains("typed in from paper"));
        assert_eq!((Slot::of_method_block("takeout", 64), Slot::of_method_block("takeout", 0), Slot::of_method_block("takeout", 310), Slot::of_method("keys.sheets"), Slot::of_method("topup"), Slot::of_method("purse.pay")), (Some(Slot::Safe), Some(Slot::Safe), Some(Slot::Sheets), Some(Slot::Safe), Some(Slot::Topup), Some(Slot::Funding)));
        assert!(Slot::Safe.serves("takeout") && Slot::Safe.serves("keys.sheets") && Slot::Sheets.serves("takeout") && !Slot::Sheets.serves("keys.sheets") && !Slot::Safe.serves("send"));
        assert!(Slot::Sheets.accepts(300) && !Slot::Sheets.accepts(0) && Slot::Safe.accepts(0) && Slot::Safe.accepts(96) && !Slot::Safe.accepts(32));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A phone-made key set with its block and sheets, under a Core that holds the public block only.
    fn phone_core(tag: &str) -> (Core, Zeroizing<[u8; 192]>, [[String; 24]; 3], std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("kv-core-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let net = Network::testnet();
        let (keys, block, sheets) = Keys::fresh_phone_all(&net, "", false, false).unwrap();
        let rpc = KaspaRpcClient::new(WrpcEncoding::Borsh, Some("ws://127.0.0.1:9"), None, Some(net.resolver_id()), None).unwrap();
        let core = Core::open(Ctx { dir: dir.clone(), keys, rpc: Some(rpc), force: false, network: net, dials: Dials::testnet(), account: 1 }).unwrap();
        (core, Zeroizing::new(block), sheets, dir)
    }
    fn x_of(secret: &[u8; 32]) -> String {
        faster_hex::hex_string(&xonly(&keypair(secret)))
    }

    /// Two sheets typed from paper, in any order, arm exactly their two cold slots (redeem-key order falls out
    /// of the slots); the wrong word, a foreign sheet, a repeated sheet and one sheet are refused naming the
    /// sheet (and the word) and never a word itself.
    #[test]
    fn two_sheets_arm_their_cold_slots_and_bad_sheets_are_refused_by_name() {
        let (core, _block, sheets, dir) = phone_core("sheets");
        let p = core.ctx.keys.public();
        let text = format!("{}\n{}\n", sheets[2].join(" "), sheets[0].join(" "));
        let armed = Armed::new(&core, Slot::Sheets, text.as_bytes()).unwrap();
        let k = &armed.core.ctx.keys;
        assert!(k.cold.is_some() && k.cold2.is_none() && k.cold3.is_some(), "sheets 1 and 3 at their slots, whatever order they were typed");
        let present = k.cold_keys();
        assert_eq!(present.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 3], "redeem-key order");
        assert_eq!((x_of(k.cold.as_ref().unwrap()), x_of(k.cold3.as_ref().unwrap())), (p.cold_x[0].clone(), p.cold_x[2].clone()));
        k.check_self().unwrap();
        assert!(!core.ctx.keys.has_secret(), "the shared Core never holds a sheet");
        drop(armed);
        let refusal_of = |text: String| Armed::new(&core, Slot::Sheets, text.as_bytes()).err().unwrap();
        // a wrong word: the sheet and the position, not the word
        let mut bad = sheets[0].clone();
        bad[4] = "zzzz".into(); // not a sheet word (zebra is one)
        let e = refusal_of(format!("{}\n{}", bad.join(" "), sheets[1].join(" ")));
        assert!(e.starts_with("Vault seed 1: Word 5") && !e.contains("zzzz"), "{e}");
        let mut swapped = sheets[1].clone();
        swapped.swap(0, 1);
        let e = refusal_of(format!("{}\n{}", sheets[0].join(" "), swapped.join(" ")));
        // seeds are random per run: 1 swap in 256 keeps a valid checksum, then it is a foreign seed
        let foreign_ok = e == format!("Vault seed 2: {NOT_A_SAFE_SHEET}");
        assert!(e.starts_with("Vault seed 2:") && (e.contains("do not check out") || foreign_ok), "{e}");
        // a sheet of another safe
        let foreign = sheet_words(&[5u8; 32]);
        let e = refusal_of(format!("{}\n{}", sheets[0].join(" "), foreign.join(" ")));
        assert_eq!(e, format!("Vault seed 2: {NOT_A_SAFE_SHEET}"));
        // the same sheet twice, and only one sheet
        assert_eq!(refusal_of(format!("{}\n{}", sheets[1].join(" "), sheets[1].join(" "))), SAME_SHEET);
        assert!(refusal_of(sheets[1].join(" ")).starts_with("Two vault seeds"));
        assert_eq!(Armed::new(&core, Slot::Sheets, &[0xff, 0xfe]).err().unwrap(), SHEETS_UNREADABLE);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The phone's own sealed safe keys: all three arm the first two slots, the last two arm theirs, another
    /// account's keys are refused, an empty block is the burned refusal; `keys.sheets` hands the words back.
    #[tokio::test]
    async fn sealed_safe_keys_arm_two_slots_and_keys_sheets_returns_the_words() {
        let (core, block, sheets, dir) = phone_core("safe");
        let armed = Armed::new(&core, Slot::Safe, &block[96..]).unwrap();
        let k = &armed.core.ctx.keys;
        assert!(k.cold.is_some() && k.cold2.is_some() && k.cold3.is_none());
        k.check_self().unwrap();
        drop(armed);
        let armed = Armed::new(&core, Slot::Safe, &block[128..]).unwrap();
        let k = &armed.core.ctx.keys;
        assert!(k.cold.is_none() && k.cold2.is_some() && k.cold3.is_some());
        drop(armed);
        assert_eq!(Armed::new(&core, Slot::Safe, &[9u8; 64]).err().unwrap(), SAFE_KEYS_MISMATCH);
        assert_eq!(Armed::new(&core, Slot::Safe, &[]).err().unwrap(), SHEETS_BURNED);
        // keys.sheets: the words of each sealed key, offline, no Core state touched
        let (status, body) = core.call_with_secret("keys.sheets", &json!({}), Slot::Safe, Zeroizing::new(block[96..].to_vec())).await;
        assert_eq!(status, 200);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["sheets"], json!(sheets));
        assert_eq!(core.call_with_secret("keys.sheets", &json!({}), Slot::Safe, Zeroizing::new(vec![])).await, (200, refusal(SHEETS_BURNED)));
        assert_eq!(core.call_with_secret("keys.sheets", &json!({}), Slot::Safe, Zeroizing::new(vec![1u8; 32])).await, (400, refusal(KEY_SIZE)));
        // takeout through call, with nothing armed, is refused before any network work
        let (status, body) = core.call_with_secret("takeout", &json!({}), Slot::Safe, Zeroizing::new(vec![])).await;
        assert_eq!((status, &body["error"]), (200, &json!(SHEETS_BURNED)));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The burn marker: keys.burned removes it (twice is fine) and status reads it; keys.reset wants a confirmation.
    #[tokio::test]
    async fn burn_marker_and_reset_confirmation() {
        let (core, _block, _sheets, dir) = phone_core("burn");
        std::fs::write(dir.join(SHEETS_ON_PHONE_FILE), b"").unwrap();
        assert_eq!(core.call("keys.burned", &json!({})).await, (200, json!({"ok": true})));
        assert!(!dir.join(SHEETS_ON_PHONE_FILE).exists());
        assert_eq!(core.call("keys.burned", &json!({})).await, (200, json!({"ok": true})));
        assert_eq!(core.try_keys_reset(&json!({})).await.err().unwrap(), RESET_UNCONFIRMED);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("kv-core-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Statements: each history item says its kind and amount; `at` comes from the times file
    /// beside the state file. Lines from before the file get none, a new line gets one, and
    /// Update now keeps the old stamps.
    #[test]
    fn history_items_carry_kind_amount_and_when() {
        let dir = std::env::temp_dir().join(format!("kv-history-at-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let mut v = Vault {
            covenant_id: kaspa_consensus_core::Hash::from_bytes([7u8; 32]),
            state: locked(),
            pending_to: None,
            history: vec!["fund 10 KAS tx aa".into()],
            contract_sha256: None,
            network: String::new(),
            dials: Dials::testnet(),
        };
        save_vault_anchored(&state, &v, "x").unwrap();
        let undated = history_json(&v, &state)[0].get("at").is_none();
        assert!(undated, "a line from before the times file gets no date");
        v.history.push("spend 2.5 KAS to kaspatest:q1, fee 0.00010 KAS, tx bb".into());
        save_vault_anchored(&state, &v, "x").unwrap();
        v.history = crate::commands::carry_history(v.history.clone(), "fund 7 KAS tx cc".into());
        v.history.push("cancel (stop key 2): everything to cold, fee 0.00010 KAS, tx dd".into());
        v.history.push("topup 3 (+0.00000 unused reserve) stage tx ee tx ff".into());
        v.history.push("inherit: everything to heir kaspatest:q2, fee 0.00010 KAS, tx gg".into());
        save_vault_anchored(&state, &v, "x").unwrap();
        let h = history_json(&v, &state);
        let kinds: Vec<&str> = h.iter().map(|i| i["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["fund", "paid", "rules", "fund", "stopped", "received", "stopped"]);
        let kas: Vec<Option<f64>> = h.iter().map(|i| i["kas"].as_f64()).collect();
        assert_eq!(kas, [Some(10.0), Some(2.5), None, Some(7.0), None, Some(3.0), None]);
        let dated = h[1..].iter().all(|i| i["at"].as_u64().is_some_and(|t| t > 1_700_000_000));
        assert!(dated, "{h:?}");
        assert_eq!(h[1]["tx"], "bb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Account ids map to state files; a folder lists the accounts that have a record and the next free id;
    /// `account` is read off the arguments with its bounds; the per-account files travel with keys.reset.
    #[test]
    fn accounts_have_their_own_state_files() {
        assert_eq!(state_file_name(1), "state.json");
        assert_eq!(state_file_name(3), "state-3.json");
        let dir = tmp("accounts");
        assert_eq!(accounts_in(&dir), Vec::<u32>::new());
        assert_eq!(next_account(&dir), Some(1));
        std::fs::write(dir.join("state.json"), b"{}").unwrap();
        std::fs::write(dir.join("state-3.json"), b"{}").unwrap();
        assert_eq!(accounts_in(&dir), vec![1, 3]);
        assert_eq!(next_account(&dir), Some(2));
        let ctx = Ctx { dir: dir.clone(), keys: Keys::default(), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        assert_eq!(ctx.with_account(3).state_path(), dir.join("state-3.json"));
        assert_eq!(ctx.state_path(), dir.join("state.json"));
        assert_eq!(account_of(&json!({})), Ok(1));
        assert_eq!(account_of(&json!({"account": null})), Ok(1));
        assert_eq!(account_of(&json!({"account": 4})), Ok(4));
        assert_eq!(account_of(&json!({"account": 0})), Err(BAD_ACCOUNT));
        assert_eq!(account_of(&json!({"account": 9})), Err(BAD_ACCOUNT));
        assert_eq!(account_of(&json!({"account": "2"})), Err(BAD_ACCOUNT));
        for f in ["state-2.json", "status-cache-2.json", "status-cache.json", "rules-next-1.json", "closed"] {
            assert!(is_account_file(f), "{f}");
        }
        assert!(!is_account_file("state.json") && !is_account_file("keys.json") && !is_account_file("statement.json"));
        assert_eq!(status_cache_name(1), STATUS_CACHE_FILE);
        assert_eq!(status_cache_name(2), "status-cache-2.json");
    }

    /// rules.change keeps the dials beside the account; fund's base takes them only when the call turns no dial.
    #[test]
    fn rules_next_round_trip() {
        let dir = tmp("rules-next");
        assert_eq!(read_rules_next(&dir, 2), Ok(None));
        let mut d = Dials::testnet();
        d.cap = 7 * KAS as i64;
        write_rules_next(&dir, 2, &d).unwrap();
        assert_eq!(read_rules_next(&dir, 2), Ok(Some(d.clone())));
        assert_eq!(read_rules_next(&dir, 1), Ok(None));
        assert!(!has_dial_args(&json!({"kas": 10, "account": 2})));
        assert!(has_dial_args(&json!({"cap_kas": 3})));
        assert!(!has_dial_args(&json!({"cap_kas": null})));
        clear_rules_next(&dir, 2);
        assert_eq!(read_rules_next(&dir, 2), Ok(None));
    }

    /// rules.change on an account that was never opened answers the "open" route (no stop to price), stores the
    /// turned dials for that account alone, and refuses a dial set that cannot work. Routed through `at`, offline.
    #[tokio::test]
    async fn rules_change_offline_stores_next_rules_for_the_account() {
        let net = Network::testnet();
        let dir = tmp("rules-change");
        Keys::fresh(&net).save(&dir.join("keys.json"), true).unwrap();
        let public_only = Keys::load_public(&dir.join("keys.json")).unwrap();
        let core = Core::open(Ctx { dir: dir.clone(), keys: public_only, rpc: None, force: false, network: net, dials: Dials::testnet(), account: 1 }).unwrap();
        let a = core.at(2).rules_change(&json!({"cap_kas": 5, "heir_delay": 30000})).await;
        assert_eq!(a["ok"], json!(true), "{a}");
        assert_eq!(a["route"], "open");
        assert!(a["fee_kas"].is_null() && a["steps"].as_array().unwrap().len() == 1);
        assert_eq!(a["dials"]["cap"].as_i64(), Some(5 * KAS as i64));
        assert!(!a["words"].as_array().unwrap().is_empty());
        assert_eq!(read_rules_next(&dir, 2).unwrap().unwrap().cap, 5 * KAS as i64);
        assert!(read_rules_next(&dir, 1).unwrap().is_none(), "account 1 untouched");
        let bad = core.rules_change(&json!({"cap_kas": 0})).await;
        assert_eq!(bad["ok"], json!(false));
        assert!(bad["error"].as_str().unwrap().contains("cap"), "{bad}");
        assert!(read_rules_next(&dir, 1).unwrap().is_none(), "a refused set is not stored");
    }

    /// heir.set rewrites only the heir in keys.json's public block (its code moves with it), writes
    /// vault-public.json again, refuses a key that is not one, and refuses while an account record says open.
    #[tokio::test]
    async fn heir_set_rewrites_the_public_block_offline() {
        let net = Network::testnet();
        let dir = tmp("heir-set");
        let full = Keys::fresh(&net);
        full.save(&dir.join("keys.json"), true).unwrap();
        let before = full.public();
        assert!(!before.heir_x.is_empty(), "fresh keys carry an heir");
        let public_only = Keys::load_public(&dir.join("keys.json")).unwrap();
        let core = Core::open(Ctx { dir: dir.clone(), keys: public_only, rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 }).unwrap();
        let bad = core.heir_set(&json!({"heir_x": "zz"})).await;
        assert_eq!(bad["ok"], json!(false), "{bad}");
        let gone = core.heir_set(&json!({"heir_x": "", "auto_heir": true})).await;
        assert_eq!(gone["ok"], json!(true), "{gone}");
        assert_eq!(gone["heir_address"], "");
        let after = Keys::load_public(&dir.join("keys.json")).unwrap().public();
        assert!(after.heir_x.is_empty() && after.heir_address.is_empty() && after.auto_heir);
        assert_eq!((&after.spend_x, &after.cold_address, &after.cancel_x), (&before.spend_x, &before.cold_address, &before.cancel_x), "the other keys stay");
        assert_ne!(after.sha256(), before.sha256(), "the rule block code moves with the heir");
        assert!(dir.join(crate::commands::PUBLIC_EXPORT_FILE).exists());
        // Back to the old heir by its key: the address comes back too.
        let again = core.heir_set(&json!({"heir_x": before.heir_x})).await;
        assert_eq!(again["ok"], json!(true), "{again}");
        assert_eq!(again["heir_address"], before.heir_address);
        // The address form: the heir's own pay-to-pubkey address reads back to the same key; a safe address is refused.
        let by_address = core.heir_set(&json!({"heir_address": before.heir_address})).await;
        assert_eq!(by_address["ok"], json!(true), "{by_address}");
        assert_eq!(by_address["heir_address"], before.heir_address);
        assert_eq!(Keys::load_public(&dir.join("keys.json")).unwrap().public().heir_x, before.heir_x);
        let safe = core.heir_set(&json!({"heir_address": before.cold_address})).await;
        assert_eq!(safe["ok"], json!(false));
        assert!(safe["error"].as_str().unwrap().contains("vault"), "{safe}");
        assert_eq!(core.heir_set(&json!({"heir_address": "kaspa:qqq"})).await["ok"], json!(false), "another network or a broken address");
        // An open account blocks the change before anything is written.
        let v = Vault { covenant_id: Hash::from_bytes([3u8; 32]), state: locked(), pending_to: None, history: vec!["fund 1 KAS".into()], contract_sha256: None, network: net.name.into(), dials: Dials::testnet() };
        std::fs::write(dir.join("state-2.json"), serde_json::to_string(&v).unwrap()).unwrap();
        let blocked = core.heir_set(&json!({"heir_x": ""})).await;
        assert_eq!(blocked["ok"], json!(false));
        assert!(blocked["error"].as_str().unwrap().contains("Account 2 is still open"), "{blocked}");
        assert_eq!(Keys::load_public(&dir.join("keys.json")).unwrap().public().heir_x, before.heir_x, "nothing written");
        let ctx = Ctx { dir: dir.clone(), keys: Keys::default(), rpc: None, force: false, network: net, dials: Dials::testnet(), account: 2 };
        assert_eq!(ctx.close_record(&v).await, Err(crate::commands::ACCOUNT_OPEN.to_string()));
    }

    /// people-next.json keeps only the guardian/heir fields being changed, merges later changes over earlier ones,
    /// drops anything else in the file, clears, and travels with keys.reset.
    #[test]
    fn people_next_round_trip_and_merge() {
        let dir = tmp("people-next");
        assert_eq!(read_people_next(&dir), Ok(None));
        let one = People::from_iter([("cancel2_x".to_string(), json!("aa"))]);
        assert_eq!(write_people_next(&dir, &one).unwrap(), one);
        let heir = People::from_iter([("heir_x".to_string(), json!("bb")), ("heir_address".to_string(), json!("cc"))]);
        let merged = write_people_next(&dir, &heir).unwrap();
        assert_eq!(merged.len(), 3);
        assert_eq!((merged["cancel2_x"].as_str(), merged["heir_x"].as_str()), (Some("aa"), Some("bb")));
        let removed = write_people_next(&dir, &People::from_iter([("cancel2_x".to_string(), json!(""))])).unwrap();
        assert_eq!(removed["cancel2_x"], "", "a later change of the same field wins");
        assert_eq!(read_people_next(&dir).unwrap().unwrap(), removed);
        std::fs::write(dir.join(PEOPLE_NEXT_FILE), r#"{"heir_x":"bb","spend_x":"zz","cancel3_x":5}"#).unwrap();
        assert_eq!(read_people_next(&dir).unwrap().unwrap(), People::from_iter([("heir_x".to_string(), json!("bb"))]), "only string people fields");
        clear_people_next(&dir);
        assert_eq!(read_people_next(&dir), Ok(None));
        assert!(is_account_file(PEOPLE_NEXT_FILE));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// rules.change takes a guardian and an heir beside the dials: a bad key, a bad slot, the wallet's own keys and
    /// another open account are refused before anything is written; good ones wait in people-next.json (merged),
    /// with or without dials, and the answer carries them.
    #[tokio::test]
    async fn rules_change_keeps_a_guardian_and_heir_waiting() {
        let net = Network::testnet();
        let dir = tmp("people-change");
        let full = Keys::fresh(&net);
        full.save(&dir.join("keys.json"), true).unwrap();
        let mine = full.public();
        let theirs = Keys::fresh(&net).public();
        let (g, h) = (theirs.spend_x.clone(), theirs.cancel_x.clone());
        let public_only = Keys::load_public(&dir.join("keys.json")).unwrap();
        let core = Core::open(Ctx { dir: dir.clone(), keys: public_only, rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 }).unwrap();
        let refused = |a: &Value, words: &str| {
            assert_eq!(a["ok"], json!(false), "{a}");
            assert_eq!(a["error"], words, "{a}");
            assert!(a["people"].is_null());
        };
        refused(&core.rules_change(&json!({"guardian_x": "zz"})).await, GUARDIAN_NOT_A_KEY);
        refused(&core.rules_change(&json!({"guardian_x": g, "guardian_which": 1})).await, GUARDIAN_WHICH);
        refused(&core.rules_change(&json!({"guardian_x": mine.cancel_x})).await, PEOPLE_OWN_KEY);
        refused(&core.rules_change(&json!({"guardian_x": mine.spend_x, "guardian_which": 3})).await, PEOPLE_OWN_KEY);
        refused(&core.rules_change(&json!({"heir_x": mine.spend_x})).await, PEOPLE_OWN_KEY);
        refused(&core.rules_change(&json!({"heir_x": "12"})).await, HEIR_NOT_A_KEY);
        assert_eq!(read_people_next(&dir), Ok(None), "nothing written by a refusal");
        // People only, no dials: the account's own dials, route "open", the guardian waiting (lower-cased).
        let a = core.rules_change(&json!({"guardian_x": g.to_ascii_uppercase()})).await;
        assert_eq!(a["ok"], json!(true), "{a}");
        assert_eq!(a["route"], "open");
        assert_eq!(a["people"], json!({"cancel2_x": g}));
        // The heir merges in, by key; the address comes with it.
        let b = core.rules_change(&json!({"heir_x": h, "cap_kas": 6, "heir_delay": 30000})).await;
        assert_eq!(b["ok"], json!(true), "{b}");
        let (_, heir_address) = Keys::heir_of(&net, &h).unwrap();
        assert_eq!(b["people"], json!({"cancel2_x": g, "heir_x": h, "heir_address": heir_address}));
        assert_eq!(read_people_next(&dir).unwrap().unwrap(), b["people"].as_object().unwrap().clone());
        assert_eq!(read_rules_next(&dir, 1).unwrap().unwrap().cap, 6 * KAS as i64);
        // A people-only change keeps the dials already waiting.
        let c = core.rules_change(&json!({"guardian_x": h, "guardian_which": 3})).await;
        assert_eq!(c["ok"], json!(true), "{c}");
        assert_eq!(read_rules_next(&dir, 1).unwrap().unwrap().cap, 6 * KAS as i64, "waiting dials kept");
        // Another open account blocks a guardian or heir, not a dial change.
        let v = Vault { covenant_id: Hash::from_bytes([3u8; 32]), state: locked(), pending_to: None, history: vec!["fund 1 KAS".into()], contract_sha256: None, network: net.name.into(), dials: Dials::testnet() };
        std::fs::write(dir.join("state-2.json"), serde_json::to_string(&v).unwrap()).unwrap();
        let before = read_people_next(&dir).unwrap();
        refused(&core.rules_change(&json!({"heir_x": ""})).await, PEOPLE_OTHERS_OPEN);
        assert_eq!(read_people_next(&dir).unwrap(), before, "file unchanged");
        assert_eq!(core.rules_change(&json!({"cap_kas": 7, "heir_delay": 30000})).await["ok"], json!(true), "a dial change still works");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// fund's apply step (offline; the opening itself needs a node): the waiting guardian and heir rewrite the
    /// rule block (cancel2_x, heir_x, the code), vault-public.json is written, the file is gone; "" gives the slot
    /// back to stop key 1; an open account blocks it and the file stays. guardian.set shares the rewrite.
    #[tokio::test]
    async fn waiting_people_rewrite_the_rule_block() {
        let net = Network::testnet();
        let dir = tmp("people-apply");
        let full = Keys::fresh(&net);
        full.save(&dir.join("keys.json"), true).unwrap();
        let mine = full.public();
        let theirs = Keys::fresh(&net).public();
        let (g, h) = (theirs.spend_x.clone(), theirs.cancel_x.clone());
        let load = || Keys::load_public(&dir.join("keys.json")).unwrap();
        let sha0 = load().public().sha256();
        let core = Core::open(Ctx { dir: dir.clone(), keys: load(), rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 }).unwrap();
        assert!(core.apply_people_next().await.unwrap().is_none(), "nothing waiting");
        assert_eq!(core.rules_change(&json!({"guardian_x": g, "heir_x": h})).await["ok"], json!(true));
        let fresh = core.apply_people_next().await.unwrap().expect("applied");
        let p = fresh.public();
        assert_eq!((p.cancel2_x.as_str(), p.heir_x.as_str()), (g.as_str(), h.as_str()));
        assert_eq!(p.heir_address, Keys::heir_of(&net, &h).unwrap().1);
        assert_eq!((&p.spend_x, &p.cancel_x, &p.cold_address, &p.cancel3_x), (&mine.spend_x, &mine.cancel_x, &mine.cold_address, &mine.cancel3_x), "the rest stays");
        let sha1 = p.sha256();
        assert_ne!(sha1, sha0, "the rule block code moves");
        if let Some(stored) = &fresh.public_sha256 {
            assert_eq!(stored, &sha1, "public_sha256 rewritten");
        }
        assert_eq!(load().public().sha256(), sha1, "on disk");
        assert_eq!(read_people_next(&dir), Ok(None), "applied, so cleared");
        assert!(dir.join(crate::commands::PUBLIC_EXPORT_FILE).exists());
        // Removing the guardian: the slot gets a key nobody holds (five different keys), the code moves again.
        assert_eq!(core.rules_change(&json!({"guardian_x": ""})).await["people"], json!({"cancel2_x": ""}));
        let back = core.apply_people_next().await.unwrap().unwrap().public();
        assert!(back.cancel2_x.len() == 64 && back.cancel2_x != g && back.cancel2_x != mine.cancel_x && back.heir_x == h);
        assert!(back.check_complete().is_ok(), "a removal still opens");
        let u = vec![back.cancel2_x.clone()];
        assert_eq!(read_unheld(&dir), (u.clone(), false), "the removal's key is listed as nobody's; a list started late");
        let holders = |p: &Public, u: (Vec<String>, bool)| stop_holders(p, &u).as_array().unwrap().iter().map(|s| s["holder"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(holders(&back, (u.clone(), false)), ["you", "unheld", "unknown"], "started late: the other slot stays unknown");
        assert_eq!(holders(&back, (u.clone(), true)), ["you", "unheld", "guardian"]);
        assert_eq!(holders(&back, (vec![], false)), ["you", "unknown", "unknown"], "a key set made before the list");
        let mut repeat = back.clone();
        repeat.cancel3_x = repeat.cancel_x.clone();
        assert_eq!(holders(&repeat, (u.clone(), true))[2], "you");
        record_unheld(&dir, &[], false).unwrap();
        assert_eq!(read_unheld(&dir), (u.clone(), false), "nothing to add: unchanged");
        record_unheld(&dir, &[], true).unwrap();
        assert!(read_unheld(&dir).1, "setup marks the list complete");
        assert_ne!(back.sha256(), sha1);
        // guardian.set: the immediate form, same rewrite and rules.
        let set = core.guardian_set(&json!({"stop_x": g, "which": 3})).await;
        assert_eq!(set, json!({"ok": true, "which": 3, "stop_x": g, "error": null}));
        assert_eq!(load().public().cancel3_x, g);
        assert_eq!(core.guardian_set(&json!({"stop_x": mine.cancel_x})).await["error"], PEOPLE_OWN_KEY);
        assert_eq!(core.guardian_set(&json!({"which": 2})).await["error"], GUARDIAN_NOT_A_KEY);
        assert_eq!(core.guardian_set(&json!({"stop_x": "", "which": 3})).await["ok"], json!(true));
        assert!(load().public().cancel3_x.len() == 64 && load().public().cancel3_x != g);
        // A block an earlier build tore (removed guardian written as an empty slot): the next opening fills it.
        let path = dir.join("keys.json");
        let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        file["public"]["cancel2_x"] = json!("");
        let torn: Public = serde_json::from_value(file["public"].clone()).unwrap();
        file["public_sha256"] = json!(torn.sha256());
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        assert!(load().public().check_complete().is_err(), "torn: no opening accepts it");
        let fixed = core.apply_people_next().await.unwrap().expect("repaired").public();
        assert!(fixed.cancel2_x.len() == 64 && fixed.check_complete().is_ok());
        assert!(core.apply_people_next().await.unwrap().is_none(), "whole again: nothing to do");
        // An open account: the rewrite is refused, nothing written, the waiting file stays for later.
        assert_eq!(core.rules_change(&json!({"heir_x": ""})).await["ok"], json!(true));
        let v = Vault { covenant_id: Hash::from_bytes([3u8; 32]), state: locked(), pending_to: None, history: vec!["fund 1 KAS".into()], contract_sha256: None, network: net.name.into(), dials: Dials::testnet() };
        std::fs::write(dir.join("state-2.json"), serde_json::to_string(&v).unwrap()).unwrap();
        let err = core.apply_people_next().await.unwrap_err();
        assert!(err.contains("Account 2 is still open"), "{err}");
        assert_eq!(load().public().heir_x, h, "nothing written");
        assert!(read_people_next(&dir).unwrap().is_some(), "still waiting");
        std::fs::remove_dir_all(&dir).ok();
    }
}
