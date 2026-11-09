//! The contract side of one vault: the testnet dials, the on-chain `State`, the stored `Vault`,
//! compiling `vault.sil` for a state into its P2SH script, and pinning the contract source.
//!
//! Pin: `fund` writes the rules built into this program (EMBEDDED_SOURCE, `include_str!` of
//! contracts/vault.sil at build time) to `<dir>/vault.sil` and records its SHA-256 in state.json
//! (`contract_sha256`). Every later command compiles from `<dir>/vault.sil`, refuses to run if that
//! file's hash is not the recorded one, and refuses a recorded hash that is neither the built-in
//! rules nor a reviewed version from contracts/history (also built in).

use crate::keys::{Keys, Public, unhex};
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::tx::{ScriptPublicKey, TransactionOutpoint, UtxoEntry};
use kaspa_txscript::pay_to_script_hash_script;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use silverscript_abi::{ArtifactValue as V, SilAbiArtifact};
use silverscript_lang::compiler::{CompileOptions, compile_to_sil_abi_artifact_with_options};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const KAS: u64 = 100_000_000;
// Testnet dials: short so a whole round fits in minutes. DAA units are ~10 per second.
pub const DELAY: i64 = 3_000; // about five minutes; each RPC command takes ~30 s, so one minute was too short to test
pub const CAP: i64 = KAS as i64; // 1 KAS per window
pub const WINDOW: i64 = 1_200; // about two minutes
pub const MAX_FEE: i64 = 5 * KAS as i64 / 100; // 0.05 KAS
pub const MIN_REMAINDER: i64 = KAS as i64;
/// Heir silence clock. With an heir key in keys.json: the testnet proof dial, 3000 DAA (about five
/// minutes; the engine shows anything under ~1500 cannot be proven early). Without one the heir
/// slot repeats the cancel key and the cold script, and the clock is set so far out it never
/// fires (it must stay below 2^32, the CSV domain).
pub const INHERIT_DELAY_TESTNET: i64 = 3_000;
pub const INHERIT_DELAY_NONE: i64 = 4_000_000_000;
pub const DAA_DOMAIN_LIMIT: u64 = 500_000_000_000;
/// Mainnet preset, in human units; the DAA numbers come from the network's clock rate at `fund`.
pub const MAINNET_DELAY_DAYS: f64 = 1.0;
pub const MAINNET_WINDOW_DAYS: f64 = 1.0;
pub const MAINNET_HEIR_DAYS: f64 = 180.0;
pub const MAINNET_CAP_KAS: f64 = 100.0;
pub const MAINNET_MAX_FEE_KAS: f64 = 0.05;
pub const MAINNET_MIN_REMAINDER_KAS: f64 = 1.0;
/// The heir's silence clock must be at least this many withdrawal delays, so a withdrawal that is
/// payable is never inheritable in the same breath (finding 3).
pub const HEIR_DELAY_RATIO: i64 = 10;
/// Every clock dial goes into a CSV lock and must stay below 2^32.
pub const CLOCK_LIMIT: i64 = 4_294_967_295;
pub const DIALS_TESTNET: &str = "testnet";
pub const DIALS_MAINNET: &str = "mainnet";

/// The six dials of one vault, fixed at `fund` and saved in state.json (they are constructor
/// arguments of the script, so they are part of the vault's address). Clocks are in DAA units,
/// money in sompi. A state.json from before the field loads with the testnet preset: every vault
/// opened before it was funded with exactly those numbers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct Dials {
    pub delay: i64,
    pub window: i64,
    pub cap: i64,
    pub max_fee: i64,
    pub min_remainder: i64,
    /// The heir's silence clock, used when the rule block has an heir key (without one the slot never fires).
    pub heir_delay: i64,
}

impl Dials {
    /// Today's testnet numbers: a whole proof round fits in minutes. Byte for byte what every open testnet vault was funded with.
    pub fn testnet() -> Dials {
        Dials { delay: DELAY, window: WINDOW, cap: CAP, max_fee: MAX_FEE, min_remainder: MIN_REMAINDER, heir_delay: INHERIT_DELAY_TESTNET }
    }
    /// Real-money numbers: a day's notice, a day's window, six months of silence before the heir.
    pub fn mainnet(daa_per_sec: f64) -> Dials {
        let days = |d: f64| (d * 86_400.0 * daa_per_sec).round() as i64;
        Dials {
            delay: days(MAINNET_DELAY_DAYS),
            window: days(MAINNET_WINDOW_DAYS),
            cap: sompi(MAINNET_CAP_KAS) as i64,
            max_fee: sompi(MAINNET_MAX_FEE_KAS) as i64,
            min_remainder: sompi(MAINNET_MIN_REMAINDER_KAS) as i64,
            heir_delay: days(MAINNET_HEIR_DAYS),
        }
    }
    /// A preset by name.
    pub fn preset(name: &str, daa_per_sec: f64) -> Result<Dials, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            DIALS_TESTNET => Ok(Dials::testnet()),
            DIALS_MAINNET => Ok(Dials::mainnet(daa_per_sec)),
            other => Err(format!("`{other}` is not a set of rules this account knows: testnet (minutes, for proofs) or mainnet (days, for real money).")),
        }
    }
    /// The least a vault may hold and still pay its way out: the exit output plus the fee allowance.
    pub fn floor(&self) -> u64 {
        (self.min_remainder + self.max_fee) as u64
    }
    /// These dials turned by a `fund` request: `delay`, `window`, `heir_delay` in DAA units and `cap_kas`,
    /// `max_fee_kas`, `min_remainder_kas` in KAS, each optional (absent or null keeps this set's value). The
    /// command line (clap) and the phone's `fund` call turn the same dials through here. A dial that is
    /// there but not a number is refused rather than quietly kept: these numbers become part of the address.
    pub fn turned(&self, args: &serde_json::Value) -> Result<Dials, String> {
        fn number(args: &serde_json::Value, name: &str) -> Result<Option<f64>, String> {
            match args.get(name) {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(v) => v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|n| n.is_finite()).map(Some).ok_or_else(|| format!("The dial `{name}` must be a number.")),
            }
        }
        let units = |name: &str, keep: i64| -> Result<i64, String> { Ok(number(args, name)?.map(|n| n as i64).unwrap_or(keep)) };
        let money = |name: &str, keep: i64| -> Result<i64, String> { Ok(number(args, name)?.map(|k| sompi(k) as i64).unwrap_or(keep)) };
        Ok(Dials {
            delay: units("delay", self.delay)?,
            window: units("window", self.window)?,
            cap: money("cap_kas", self.cap)?,
            max_fee: money("max_fee_kas", self.max_fee)?,
            min_remainder: money("min_remainder_kas", self.min_remainder)?,
            heir_delay: units("heir_delay", self.heir_delay)?,
        })
    }
    /// Refuses a set that cannot work: a clock at or below zero or past the lock domain, a money dial
    /// at or below zero, a fee allowance that swallows the minimum, or (with an heir) a silence clock
    /// under HEIR_DELAY_RATIO withdrawal delays. The testnet proof preset is the one exception to the
    /// last rule, on testnet only: its heir clock equals its delay so a whole round fits in minutes, and
    /// every open testnet vault runs on it. On real money the same numbers with an heir are refused.
    pub fn check(&self, has_heir: bool, mainnet: bool) -> Result<(), String> {
        for (name, v) in [("delay", self.delay), ("window", self.window), ("heir delay", self.heir_delay)] {
            if v <= 0 || v > CLOCK_LIMIT {
                return Err(format!("the {name} dial must be between 1 and {CLOCK_LIMIT} DAA ({v} given). Pick a value in that range."));
            }
        }
        for (name, v) in [("cap", self.cap), ("max fee", self.max_fee), ("min remainder", self.min_remainder)] {
            if v <= 0 {
                return Err(format!("the {name} dial must be above zero ({:.8} KAS given). Give it in KAS, for example --{}-kas 1", v as f64 / KAS as f64, name.replace(' ', "-")));
            }
        }
        if self.max_fee >= self.min_remainder {
            return Err(format!(
                "the max fee dial ({} KAS) must be below the min remainder dial ({} KAS). Lower the fee allowance or raise the minimum.",
                kas(self.max_fee as u64),
                kas(self.min_remainder as u64)
            ));
        }
        let proof_dials = !mainnet && *self == Dials::testnet();
        if has_heir && !proof_dials && self.heir_delay < self.delay.saturating_mul(HEIR_DELAY_RATIO) {
            return Err(format!(
                "The inactivity period before the heir can claim must be at least {HEIR_DELAY_RATIO} times the notice period, so funds that are ready to leave can never be claimed at the same moment. Lengthen the inactivity period or shorten the notice period."
            ));
        }
        Ok(())
    }
    /// One line per dial, in the app's on-screen names (notice period, limit, inactivity, claim), clocks as time only.
    pub fn describe(&self, daa_per_sec: f64, has_heir: bool) -> Vec<String> {
        let time = |daa: i64| human_time(daa as f64 / daa_per_sec.max(0.001));
        let mut lines = vec![
            format!("notice period before a large payment leaves: about {}", time(self.delay)),
            format!("the daily limit refreshes after: about {}", time(self.window)),
            format!("limit: up to {} KAS in total per {}", kas(self.cap as u64), time(self.window)),
            format!("network fee allowance per payment: {} KAS", kas(self.max_fee as u64)),
            format!("the account always keeps at least: {} KAS", kas(self.min_remainder as u64)),
        ];
        lines.push(if has_heir {
            format!("your heir can claim after: about {} of inactivity", time(self.heir_delay))
        } else {
            "heir: none".into()
        });
        lines
    }
}

/// Seconds as a short human span: "300 seconds", "5 minutes", "24.0 hours", "180 days".
pub fn human_time(secs: f64) -> String {
    if secs < 120.0 {
        format!("{secs:.0} seconds")
    } else if secs < 7_200.0 {
        format!("{:.0} minutes", secs / 60.0)
    } else if secs < 172_800.0 {
        format!("{:.1} hours", secs / 3_600.0)
    } else {
        format!("{:.0} days", secs / 86_400.0)
    }
}
/// Openings of the refusals `send` turns into a withdrawal with notice.
pub const OVER_CAP: &str = "this spend would pass the cap";
pub const LOW_REMAINDER: &str = "this spend would leave less than the minimum";
/// v6 hard-limit dial: the contract's `request` entry fails outright, so the client says so first.
pub const HARD_LIMIT: &str = "This account never lets a payment above the limit wait: it has the hard limit. Funds above the limit leave only by the emergency stop to your vault, or to your heir.";
/// Share of the heir period after which the watcher warns; the phone draws the same warning (status `heir_warn_share`).
pub const HEIR_WARN_SHARE: f64 = 0.9;

/// Where the heir's silence clock stands for a coin `age` DAA old against the heir period `delay`
/// (one function for the watcher and the phone service; moved here from watch.rs).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum HeirClock {
    Quiet,
    /// HEIR_WARN_SHARE (90%) of the period has passed.
    Warn,
    /// The period has passed: an inheritOpen would be accepted.
    Due,
}
pub fn heir_clock(age: i64, delay: i64) -> HeirClock {
    if age >= delay {
        HeirClock::Due
    } else if age.saturating_mul(10) >= delay.saturating_mul(9) {
        // 9/10 is HEIR_WARN_SHARE in exact integer math
        HeirClock::Warn
    } else {
        HeirClock::Quiet
    }
}
/// DAA left until the heir may claim a coin `age` DAA old: never below zero.
pub fn heir_daa_left(age: i64, delay: i64) -> i64 {
    delay.saturating_sub(age).max(0)
}

/// Name of the pinned contract copy inside a vault folder.
pub const SOURCE_FILE: &str = "vault.sil";
/// What the user reads when the pinned copy no longer matches state.json.
pub const RULES_CHANGED: &str = "The rules file of this account was changed. Restore";
/// The key under which state.json records the rule block code (`Public::sha256` of the `public`
/// block of keys.json at `fund`). It sits beside the vault's own fields, not in `Vault`, so a
/// state.json from before it still loads (and the watcher's and service's `Vault` literals stay).
pub const PUBLIC_ANCHOR: &str = "public_sha256";
/// What the user reads when keys.json's `public` block no longer hashes to the recorded code.
pub const RULE_BLOCK_CHANGED: &str = "The rule block in keys.json has changed since this vault was funded. Restore the original file or start a new vault.";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct State {
    pub stage: i64,
    pub dest_hash: Vec<u8>,
    pub amount: i64,
    pub window_start: i64,
    pub spent: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Vault {
    pub covenant_id: Hash,
    pub state: State,
    pub pending_to: Option<String>,
    pub history: Vec<String>,
    /// SHA-256 (hex) of `<dir>/vault.sil` at `fund` time; every later compile must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_sha256: Option<String>,
    /// The network this vault was opened on (`fund`): "testnet" or "mainnet". Files from before the
    /// word load as empty; the folder refuses to run under another `--network` (tx.rs try_load_vault).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub network: String,
    /// The six dials this vault was funded with (`fund`); files from before the field were all funded with the testnet preset.
    #[serde(default = "Dials::testnet")]
    pub dials: Dials,
}

pub type Coin = (TransactionOutpoint, UtxoEntry);

/// A stopped account: its last move sent the coin to the safe (stop) or to the heir. Its record stays until the
/// account opens again (`fund` moves it into `closed/`).
pub fn is_stopped(v: &Vault) -> bool {
    v.history.last().is_some_and(|h| h.starts_with("cancel") || h.starts_with("inherit"))
}

pub fn locked() -> State {
    State { stage: 0, dest_hash: vec![0; 32], amount: 0, window_start: 0, spent: 0 }
}

/// The state a `finish` writes back when something stays in the vault: Locked again, with the
/// pending destination wiped (contract v3, L5: `destHash` must be 32 zero bytes, no stale
/// destination in Locked). The `finish` command and the watcher's lineage check both call this
/// so they can never disagree on the continuation script.
pub fn after_finish(s: &State) -> State {
    State { stage: 0, amount: 0, dest_hash: vec![0; 32], ..s.clone() }
}

/// The state a `decline` (v5) writes back: Locked, destination wiped, the window counters
/// untouched (a declined request neither widens nor resets the card limit). The same shape as
/// `after_finish`; named apart so the decline command, the air-gap request and the watcher's
/// lineage check all point at the contract's `decline` entry.
pub fn after_decline(s: &State) -> State {
    after_finish(s)
}

/// The state a `spend` writes back. A new window opens when the transaction's lock time is at
/// least the window dial past both the current window start and the coin's own DAA score; otherwise the
/// outflow (input minus the continuation output, fee included) is added to the open window.
/// The `spend` command and the watcher's lineage check share this so they can never disagree.
pub fn after_spend(s: &State, window: i64, in_daa: i64, lock_time: i64, outflow: i64) -> State {
    let (window_start, spent) = if lock_time >= s.window_start + window && lock_time >= in_daa + window { (lock_time, outflow) } else { (s.window_start, s.spent + outflow) };
    State { window_start, spent, ..s.clone() }
}

pub fn spk_bytes(s: &ScriptPublicKey) -> Vec<u8> {
    let mut b = s.version().to_be_bytes().to_vec();
    b.extend_from_slice(s.script());
    b
}
pub fn spk_hash(s: &ScriptPublicKey) -> Vec<u8> {
    let b = spk_bytes(s);
    blake2b_simd::Params::new().hash_length(32).hash(&b).as_bytes().to_vec()
}
pub fn sompi(kas: f64) -> u64 {
    (kas * KAS as f64).round() as u64
}
pub fn kas(sompi: u64) -> f64 {
    sompi as f64 / KAS as f64
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    faster_hex::hex_string(&Sha256::digest(bytes))
}

/// The reviewed rules file, built into this program at build time; `fund` pins exactly these bytes
/// (item 19: the pin no longer trusts whatever sits in the repo folder at that moment).
pub const EMBEDDED_SOURCE: &str = include_str!("../../contracts/vault.sil");
/// Earlier reviewed versions (contracts/history at build time) still allowed to drive an open vault.
const HISTORY_SOURCES: [(&str, &str); 3] = [
    ("vault-v2.sil", include_str!("../../contracts/history/vault-v2.sil")),
    ("vault-v3.sil", include_str!("../../contracts/history/vault-v3.sil")),
    ("vault-v4.sil", include_str!("../../contracts/history/vault-v4.sil")),
];
/// The first published text of the v6 rules. Its code is the built-in source's, character for character; only
/// three comment lines differ (they were reworded before the source went public). Accounts opened under that text
/// recorded this hash, so it is trusted as an equivalent of the built-in source (`equivalent_of`): such a folder
/// keeps working, and `fund` re-pins it to the built-in text. Proven: both texts compile to identical scripts
/// (test `first_v6_text_is_the_built_in_code` checks the code with comments stripped).
pub const FIRST_V6_SHA: &str = "84df44844860a867af655bdeec333fdf1739a80ee3987386795efd85206ffc2b";
/// SHA-256 of the built-in source with every `//` comment and blank line removed, as the first v6 text gives it too.
#[cfg(test)]
const V6_CODE_SHA: &str = "9d720ad79bcf2560ae53c7e005f44e6c5011e51694a1fb80994adec7270beaa5";

/// The built-in source when `sha` names a text with the same code (the first v6 text), else None.
fn equivalent_of(sha: &str) -> Option<&'static str> {
    (sha == FIRST_V6_SHA).then_some(EMBEDDED_SOURCE)
}

/// The source with `//` comments and blank lines removed: what an equivalence between two texts is judged on.
pub fn code_only(source: &str) -> String {
    source.lines().map(|l| l.split("//").next().unwrap_or("").trim_end()).filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

/// What the user reads when a vault is pinned to rules this program was not built with.
pub const UNKNOWN_RULES: &str = "The rules this vault is pinned to are not a version this program knows";

/// SHA-256 (hex) of the built-in rules, computed once.
pub fn embedded_sha256() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| sha256_hex(EMBEDDED_SOURCE.as_bytes()))
}
/// Every rules hash this program accepts, as (name, sha): the built-in one first, then the history.
pub fn known_sha256s() -> &'static [(String, String)] {
    static H: OnceLock<Vec<(String, String)>> = OnceLock::new();
    H.get_or_init(|| {
        std::iter::once(("vault.sil (built in)".to_string(), embedded_sha256().to_string()))
            .chain(HISTORY_SOURCES.iter().map(|(n, s)| (format!("history/{n}"), sha256_hex(s.as_bytes()))))
            .chain(std::iter::once(("vault-v6 (first text)".to_string(), FIRST_V6_SHA.to_string())))
            .collect()
    })
}
pub fn is_known_source(sha: &str) -> bool {
    known_sha256s().iter().any(|(_, h)| h == sha)
}
fn unknown_rules(sha: &str) -> String {
    let head: String = sha.chars().take(8).filter(char::is_ascii_hexdigit).collect();
    format!(
        "{UNKNOWN_RULES} (sha {head}...). Use the program version that opened this vault, \
         or open a new vault in a new folder."
    )
}

/// The repository copy of the contract, read only to say when it drifts from the built-in rules.
fn repo_source_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../contracts/vault.sil")
}

/// `fund`: write the built-in rules into the vault folder and return their SHA-256. A folder already
/// pinned to different bytes is refused, never overwritten: one run folder per contract version
/// (an earlier version let a fund re-pin a folder that still held an open vault to a newer contract).
pub fn pin_source(dir: &Path) -> Result<String, String> {
    let bytes = EMBEDDED_SOURCE.as_bytes();
    if let Ok(repo) = std::fs::read(repo_source_path())
        && repo != bytes {
            println!(
                "note: contracts/vault.sil in the repo folder differs from the rules built into this program (sha {}... vs {}...); the vault is pinned to the built-in rules. Rebuild the program if the repo copy is the reviewed one.",
                &sha256_hex(&repo)[..8],
                &embedded_sha256()[..8]
            );
        }
    let pinned = dir.join(SOURCE_FILE);
    if let Ok(old) = std::fs::read(&pinned)
        && old != bytes
        && equivalent_of(&sha256_hex(&old)) != Some(EMBEDDED_SOURCE) {
            return Err(format!(
                "Refused: this folder is pinned to another version of the rules (sha {}... vs {}...). Open the new vault in a new folder.",
                &sha256_hex(&old)[..8],
                &embedded_sha256()[..8]
            ));
        }
    std::fs::write(&pinned, bytes).map_err(|e| format!("{}: {e}", pinned.display()))?;
    Ok(embedded_sha256().to_string())
}

/// `guard-import` (D6): put the pinned rules a second, keyless guard needs into `dir`, from the versions
/// built into this program, picked by `sha` (the vault's `contract_sha256`). A folder pinned to other
/// bytes is refused, never overwritten; a hash this program was not built with is refused.
pub fn restore_source(dir: &Path, sha: &str) -> Result<(), String> {
    let source = std::iter::once(EMBEDDED_SOURCE)
        .chain(HISTORY_SOURCES.iter().map(|(_, s)| *s))
        .find(|s| sha256_hex(s.as_bytes()) == sha)
        .or_else(|| equivalent_of(sha))
        .ok_or_else(|| unknown_rules(sha))?;
    let pinned = dir.join(SOURCE_FILE);
    if let Ok(old) = std::fs::read(&pinned)
        && old != source.as_bytes()
        && equivalent_of(&sha256_hex(&old)) != Some(source) {
            return Err(format!("Refused: this folder is pinned to another version of the rules (sha {}... vs {}...). Use a new folder for this guard.", &sha256_hex(&old)[..8], &sha[..sha.len().min(8)]));
        }
    std::fs::write(&pinned, source.as_bytes()).map_err(|e| format!("{}: {e}", pinned.display()))
}

/// The contract source for this folder. With a recorded hash (an open vault) it is `<dir>/vault.sil`
/// and must match, and the hash must be one this program knows; with none, `<dir>/vault.sil` if
/// present (fund has just pinned it; also checked against the known list), else the built-in rules.
pub fn load_source(dir: &Path, state: &Path, expected: Option<&str>) -> Result<String, String> {
    check_public_anchor(dir, state)?;
    let pinned = dir.join(SOURCE_FILE);
    match expected {
        Some(want) => {
            if !is_known_source(want) {
                return Err(unknown_rules(want));
            }
            let bytes = std::fs::read(&pinned).map_err(|_| format!("{RULES_CHANGED} {} (it is missing).", pinned.display()))?;
            let same_code = equivalent_of(want).is_some_and(|s| s.as_bytes() == bytes.as_slice());
            if sha256_hex(&bytes) != want && !same_code {
                return Err(format!("{RULES_CHANGED} {}.", pinned.display()));
            }
            String::from_utf8(bytes).map_err(|_| format!("{RULES_CHANGED} {} (not text).", pinned.display()))
        }
        None if pinned.exists() => {
            let bytes = std::fs::read(&pinned).map_err(|e| format!("{}: {e}", pinned.display()))?;
            let sha = sha256_hex(&bytes);
            if !is_known_source(&sha) {
                return Err(unknown_rules(&sha));
            }
            String::from_utf8(bytes).map_err(|_| format!("{RULES_CHANGED} {} (not text).", pinned.display()))
        }
        None => Ok(EMBEDDED_SOURCE.to_string()),
    }
}

/// The rule block code state.json records: None with no vault open, or a vault file from before the code.
pub fn recorded_public_sha256(state: &Path) -> Result<Option<String>, String> {
    let text = match std::fs::read_to_string(state) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("state.json: {e}"))?;
    Ok(v.get(PUBLIC_ANCHOR).and_then(|h| h.as_str()).map(String::from))
}

/// Writes state.json: the vault's fields with the rule block code beside them.
pub fn save_vault_anchored(state: &Path, v: &Vault, public_sha256: &str) -> Result<(), String> {
    let mut json = serde_json::to_value(v).map_err(|e| format!("state.json: {e}"))?;
    json[PUBLIC_ANCHOR] = serde_json::Value::String(public_sha256.to_string());
    let text = serde_json::to_string_pretty(&json).map_err(|e| format!("state.json: {e}"))?;
    // Written beside, then renamed over: a reader (a stop on a watched copy while its follower saves) sees the
    // old record or the new one, never half a file.
    let tmp = state.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("state.json: {e}"))?;
    std::fs::rename(&tmp, state).map_err(|e| format!("state.json: {e}"))?;
    stamp_history(state, &v.history); // after the money record, and never able to fail it
    Ok(())
}

/// When each history line was first saved (Statements dates): `<state>.times.json` beside
/// the account's state file, `[[unix secs, line], ...]` in history order. A line keeps its
/// stamp while the same text sits at the same place (Update now keeps the old lines and
/// appends); a new line gets now. Lines already there when the file is first made get 0
/// (unknown), never a made-up date.
pub fn history_times_path(state: &Path) -> PathBuf {
    state.with_extension("times.json")
}

pub fn history_times(state: &Path) -> Vec<(u64, String)> {
    let text = std::fs::read_to_string(history_times_path(state)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

fn stamp_history(state: &Path, history: &[String]) {
    let path = history_times_path(state);
    let first = !path.exists();
    let old = history_times(state);
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let now = since.map_or(0, |d| d.as_secs());
    let mut out: Vec<(u64, String)> = Vec::with_capacity(history.len());
    for (i, line) in history.iter().enumerate() {
        let kept = old.get(i).filter(|(_, t)| t == line).map(|(at, _)| *at);
        out.push((kept.unwrap_or(if first { 0 } else { now }), line.clone()));
    }
    if let Ok(text) = serde_json::to_string(&out) {
        let tmp = path.with_extension("tmp");
        let _ = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path));
    }
}

/// Every compile passes here (`load_source`): with a code recorded in state.json, the `public`
/// block of `<dir>/keys.json` as it is on disk must still hash to it. No code recorded: nothing
/// to hold the block to yet (a vault file from before the code, or no vault at all).
pub fn check_public_anchor(dir: &Path, state: &Path) -> Result<(), String> {
    let Some(want) = recorded_public_sha256(state)? else { return Ok(()) };
    match Keys::peek_public(&dir.join("keys.json"))? {
        Some(p) if p.sha256() == want => Ok(()),
        Some(_) => Err(RULE_BLOCK_CHANGED.into()),
        None => Err(format!("{RULE_BLOCK_CHANGED} (keys.json is missing from {}; put the vault's keys.json back.)", dir.display())),
    }
}

/// Compiles the vault for one state. Constructor order: spend, the three stop keys (an empty slot
/// repeats the first), cold hash, heir key + heir script hash + inherit delay (no heir: cancel key,
/// cold hash, far out), the auto-heir dial (v5), the hard-limit dial (v6, 20 arguments), dials, state.
pub fn compile(source: &str, public: &Public, dials: &Dials, s: &State) -> SilAbiArtifact {
    let cold_hash = spk_hash(&public.network().to_spk(&public.cold_address));
    let (heir_x, heir_hash, inherit_delay) = public.heir_slot(cold_hash.clone(), dials);
    let args = vec![
        V::Bytes(unhex(&public.spend_x)),
        V::Bytes(unhex(public.stop_x(0))),
        V::Bytes(unhex(public.stop_x(1))),
        V::Bytes(unhex(public.stop_x(2))),
        V::Bytes(cold_hash),
        V::Bytes(heir_x),
        V::Bytes(heir_hash),
        V::Int(inherit_delay),
        V::Int(if public.auto_heir { 1 } else { 0 }),
        V::Int(if public.hard_limit { 1 } else { 0 }), // v6
        V::Int(dials.delay),
        V::Int(dials.cap),
        V::Int(dials.window),
        V::Int(dials.max_fee),
        V::Int(dials.min_remainder),
        V::Int(s.stage),
        V::Bytes(s.dest_hash.clone()),
        V::Int(s.amount),
        V::Int(s.window_start),
        V::Int(s.spent),
    ];
    compile_to_sil_abi_artifact_with_options(source, &args, CompileOptions::default()).expect("vault compiles")
}
pub fn bytecode(a: &SilAbiArtifact) -> Vec<u8> {
    a.contracts.values().next().unwrap().compiled.bytecode.clone()
}
pub fn spk_of(a: &SilAbiArtifact) -> ScriptPublicKey {
    pay_to_script_hash_script(&bytecode(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dials_presets_and_the_heir_rule() {
        let t = Dials::testnet();
        assert_eq!((t.delay, t.window, t.cap, t.max_fee, t.min_remainder, t.heir_delay), (3_000, 1_200, 100_000_000, 5_000_000, 100_000_000, 3_000));
        assert_eq!(Dials::preset("testnet", 10.0).unwrap(), t);
        let m = Dials::preset("MainNet", 10.0).unwrap();
        assert_eq!((m.delay, m.window, m.heir_delay), (864_000, 864_000, 155_520_000));
        assert_eq!((m.cap, m.max_fee, m.min_remainder), (100 * KAS as i64, 5 * KAS as i64 / 100, KAS as i64));
        assert!(m.heir_delay < CLOCK_LIMIT);
        assert!(Dials::preset("devnet", 10.0).unwrap_err().contains("testnet (minutes"));
        assert!(t.check(true, false).is_ok() && m.check(true, true).is_ok() && m.check(false, true).is_ok());
        assert!(t.check(true, true).unwrap_err().contains("at least 10 times"), "the proof dials with an heir are refused on real money");
        assert!(t.check(false, true).is_ok(), "without an heir the short dials may run on real money (proof dials)");
        // the heir's silence must be at least ten withdrawal delays, unless there is no heir
        let short = Dials { delay: 3_000, heir_delay: 29_999, ..m.clone() };
        let e = short.check(true, false).unwrap_err();
        assert!(e.contains("at least 10 times the notice period"), "{e}");
        assert!(short.check(false, false).is_ok());
        assert!(Dials { delay: 0, ..t.clone() }.check(false, false).unwrap_err().contains("delay dial"));
        assert!(Dials { heir_delay: CLOCK_LIMIT + 1, ..t.clone() }.check(false, false).unwrap_err().contains("heir delay dial"));
        assert!(Dials { cap: 0, ..t.clone() }.check(false, false).unwrap_err().contains("--cap-kas"));
        assert!(Dials { max_fee: KAS as i64, ..t.clone() }.check(false, false).unwrap_err().contains("below the min remainder"));
        assert_eq!(t.floor(), 105_000_000);
        let words = m.describe(10.0, true).join("\n");
        assert!(words.contains("about 24.0 hours") && words.contains("about 180 days") && words.contains("up to 100 KAS"), "{words}");
        assert!(t.describe(10.0, false).join("\n").contains("heir: none"));
        assert_eq!(human_time(300.0), "5 minutes");
    }

    #[test]
    fn old_state_json_loads_with_the_testnet_dials_and_new_files_carry_theirs() {
        // vaults 11, 13, 16 and the v5 run: a state.json written before the dials field
        let v0 = Vault { covenant_id: Hash::from_bytes([5u8; 32]), state: locked(), pending_to: None, history: vec!["fund".into()], contract_sha256: None, network: String::new(), dials: Dials::mainnet(10.0) };
        let mut j = serde_json::to_value(&v0).unwrap();
        j.as_object_mut().unwrap().remove("dials");
        let v: Vault = serde_json::from_value(j).unwrap();
        assert_eq!(v.dials, Dials::testnet(), "a file without dials was funded with the testnet preset");
        let text = serde_json::to_string(&v).unwrap();
        assert!(text.contains("\"dials\":{\"delay\":3000,\"window\":1200,\"cap\":100000000,\"max_fee\":5000000,\"min_remainder\":100000000,\"heir_delay\":3000}"), "{text}");
        // a mainnet file keeps its own numbers through a round trip
        let again: Vault = serde_json::from_str(&serde_json::to_string(&v0).unwrap()).unwrap();
        assert_eq!(again.dials, Dials::mainnet(10.0));
        // the dials are constructor arguments: another set is another vault address
        let source = EMBEDDED_SOURCE.to_string();
        let p = crate::keys::Keys::fresh(&crate::network::Network::testnet()).public();
        assert_ne!(spk_of(&compile(&source, &p, &Dials::testnet(), &locked())), spk_of(&compile(&source, &p, &Dials::mainnet(10.0), &locked())));
    }

    #[test]
    fn state_json_anchor_refuses_a_changed_rule_block_and_loads_old_files() {
        let dir = std::env::temp_dir().join(format!("kv-anchor-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let keys = Keys::fresh(&crate::network::Network::testnet());
        let code = keys.public().sha256();
        let keys_path = dir.join("keys.json");
        std::fs::write(&keys_path, serde_json::to_string_pretty(&keys).unwrap()).unwrap();
        let v = Vault { covenant_id: Hash::from_bytes([5u8; 32]), state: locked(), pending_to: None, history: vec!["fund".into()], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        // no vault yet: nothing to hold the block to
        assert_eq!(recorded_public_sha256(&dir.join("state.json")), Ok(None));
        assert!(load_source(&dir, &dir.join("state.json"), None).is_ok());
        // a vault file from before the code loads and records nothing
        std::fs::write(dir.join("state.json"), serde_json::to_string_pretty(&v).unwrap()).unwrap();
        assert_eq!(recorded_public_sha256(&dir.join("state.json")), Ok(None));
        assert!(load_source(&dir, &dir.join("state.json"), None).is_ok());
        // fund records the code beside the vault's fields; the vault still reads back as before
        save_vault_anchored(&dir.join("state.json"), &v, &code).unwrap();
        assert_eq!(recorded_public_sha256(&dir.join("state.json")), Ok(Some(code.clone())));
        let back: Vault = serde_json::from_str(&std::fs::read_to_string(dir.join("state.json")).unwrap()).unwrap();
        assert_eq!((back.covenant_id, back.history), (v.covenant_id, v.history.clone()));
        assert!(load_source(&dir, &dir.join("state.json"), None).is_ok());
        // the block edited on disk (the dial flipped, the safe moved): every compile refuses in bank words
        let mut edited = Keys { public: Some(keys.public()), ..Default::default() };
        edited.public.as_mut().unwrap().auto_heir = true;
        std::fs::write(&keys_path, serde_json::to_string_pretty(&edited).unwrap()).unwrap();
        assert_eq!(load_source(&dir, &dir.join("state.json"), None), Err(RULE_BLOCK_CHANGED.to_string()));
        let mut edited = Keys { public: Some(keys.public()), ..Default::default() };
        edited.public.as_mut().unwrap().cold_address = keys.public().heir_address.clone();
        std::fs::write(&keys_path, serde_json::to_string_pretty(&edited).unwrap()).unwrap();
        assert_eq!(load_source(&dir, &dir.join("state.json"), None), Err(RULE_BLOCK_CHANGED.to_string()));
        // the file gone: refused too, with where to put it back
        std::fs::remove_file(&keys_path).unwrap();
        let err = load_source(&dir, &dir.join("state.json"), None).unwrap_err();
        assert!(err.starts_with(RULE_BLOCK_CHANGED) && err.contains("put the vault's keys.json back"), "{err}");
        // the original back: fine again, secrets or not
        std::fs::write(&keys_path, serde_json::to_string_pretty(&Keys { public: Some(keys.public()), ..Default::default() }).unwrap()).unwrap();
        assert!(load_source(&dir, &dir.join("state.json"), None).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_outside_safe_is_pinned_at_fund_like_every_rule() {
        let net = crate::network::Network::testnet();
        let dir = std::env::temp_dir().join(format!("kv-outside-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut keys = Keys::fresh(&net);
        let keys_path = dir.join("keys.json");
        std::fs::write(&keys_path, serde_json::to_string_pretty(&keys).unwrap()).unwrap();
        let outside = net.p2pk(&crate::keys::xonly(&crate::keys::keypair(&[42u8; 32]))).to_string();
        let other = net.p2pk(&crate::keys::xonly(&crate::keys::keypair(&[43u8; 32]))).to_string();
        // before fund the command sets it (plain file stays plain)
        crate::commands::do_set_cold_to(&dir, &outside, &net).unwrap();
        let text = std::fs::read_to_string(&keys_path).unwrap();
        keys = serde_json::from_str(&text).unwrap();
        assert!(keys.public().cold_external && keys.cold.is_none() && keys.public().cold_address == outside);
        assert!(crate::commands::do_set_cold_to(&dir, &outside, &crate::network::Network::mainnet()).is_err(), "the folder's network only");
        // fund records the block's code
        let v = Vault { covenant_id: Hash::from_bytes([5u8; 32]), state: locked(), pending_to: None, history: vec!["fund".into()], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        save_vault_anchored(&dir.join("state.json"), &v, &keys.public().sha256()).unwrap();
        assert!(load_source(&dir, &dir.join("state.json"), None).is_ok());
        // after fund the command refuses, and a hand edit (flag kept, address moved) is refused by the pin
        let err = crate::commands::do_set_cold_to(&dir, &other, &net).unwrap_err();
        assert!(err.contains("open account"), "{err}");
        for f in [|p: &mut Public, o: &str| p.cold_address = o.into(), |p: &mut Public, _: &str| p.cold_external = false] {
            let mut edited = Keys { public: Some(keys.public()), ..Default::default() };
            f(edited.public.as_mut().unwrap(), &other);
            std::fs::write(&keys_path, serde_json::to_string_pretty(&edited).unwrap()).unwrap();
            assert_eq!(load_source(&dir, &dir.join("state.json"), None), Err(RULE_BLOCK_CHANGED.to_string()));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn heir_clock_and_daa_left_agree() {
        assert_eq!(heir_clock(0, 3000), HeirClock::Quiet);
        assert_eq!(heir_clock(2699, 3000), HeirClock::Quiet, "just under 90%");
        assert_eq!(heir_clock(2700, 3000), HeirClock::Warn, "90% on the dot");
        assert_eq!(heir_clock(3000, 3000), HeirClock::Due);
        assert_eq!(heir_clock(i64::MAX, 4_000_000_000), HeirClock::Due, "no overflow at the far clock");
        assert_eq!((HEIR_WARN_SHARE * 10.0).round() as i64, 9, "the integer warn step and the published share agree");
        assert_eq!(heir_daa_left(0, 3000), 3000);
        assert_eq!(heir_daa_left(2700, 3000), 300);
        assert_eq!(heir_daa_left(3000, 3000), 0, "due: nothing left");
        assert_eq!(heir_daa_left(9000, 3000), 0, "long past due: still zero, never negative");
        assert_eq!(heir_daa_left(-5, i64::MAX), i64::MAX, "no overflow");
    }

    #[test]
    fn after_spend_matches_the_window_rule() {
        let s = State { window_start: 1_000, spent: 5, ..locked() };
        // lock time short of both bounds: same window, outflow added
        assert_eq!(after_spend(&s, WINDOW, 1_500, 2_000, 7), State { window_start: 1_000, spent: 12, ..locked() });
        // past the window start but not the coin's own age: still the same window
        assert_eq!(after_spend(&s, WINDOW, 1_500, 2_699, 7), State { window_start: 1_000, spent: 12, ..locked() });
        // past both: a new window starting at the lock time holding only this outflow
        assert_eq!(after_spend(&s, WINDOW, 1_500, 2_700, 7), State { window_start: 2_700, spent: 7, ..locked() });
        // stage, destination and amount ride along unchanged
        let p = State { stage: 1, dest_hash: vec![9; 32], amount: 3, window_start: 0, spent: 0 };
        let n = after_spend(&p, WINDOW, 0, 0, 1);
        assert_eq!((n.stage, n.dest_hash.clone(), n.amount, n.spent), (1, vec![9; 32], 3, 1));
    }

    #[test]
    fn twenty_args_compile_and_the_dials_move_the_address() {
        let source = EMBEDDED_SOURCE.to_string();
        let mut p = crate::keys::Keys::fresh(&crate::network::Network::testnet()).public();
        assert!(!p.auto_heir && !p.hard_limit, "keys writes both dials off");
        let off = spk_of(&compile(&source, &p, &Dials::testnet(), &locked()));
        p.auto_heir = true;
        let on = spk_of(&compile(&source, &p, &Dials::testnet(), &locked()));
        assert_ne!(off, on, "the dial is a constructor argument, so it changes the vault address");
        // v6: the hard-limit dial is the 10th argument and moves the address too
        let hard = spk_of(&compile(&source, &crate::keys::Public { hard_limit: true, auto_heir: false, ..p.clone() }, &Dials::testnet(), &locked()));
        assert!(hard != off && hard != on, "the hard-limit dial changes the vault address on its own");
        // the Pending state compiles too (decline and inherit-open from Pending start here)
        let pending = State { stage: 1, dest_hash: vec![9; 32], amount: 3 * KAS as i64, window_start: 0, spent: 0 };
        assert_ne!(spk_of(&compile(&source, &p, &Dials::testnet(), &pending)), on);
        assert_eq!(after_decline(&pending), State { window_start: 0, spent: 0, ..locked() });
    }

    #[test]
    fn pin_hash_check() {
        let dir = std::env::temp_dir().join(format!("kv-pin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sha = pin_source(&dir).unwrap();
        assert_eq!(sha, embedded_sha256());
        // the pinned copy compiles for the open vault and for a folder with no vault yet
        assert!(load_source(&dir, &dir.join("state.json"), Some(&sha)).is_ok());
        assert!(load_source(&dir, &dir.join("state.json"), None).is_ok());
        // one changed byte in the rules file: refused with the user's sentence
        let path = dir.join(SOURCE_FILE);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n// touched\n");
        std::fs::write(&path, text).unwrap();
        let err = load_source(&dir, &dir.join("state.json"), Some(&sha)).unwrap_err();
        assert!(err.starts_with(RULES_CHANGED) && err.contains("vault.sil"), "{err}");
        // a missing pinned copy is refused the same way; a wrong recorded hash too
        std::fs::remove_file(&path).unwrap();
        assert!(load_source(&dir, &dir.join("state.json"), Some(&sha)).unwrap_err().starts_with(RULES_CHANGED));
        pin_source(&dir).unwrap();
        assert!(load_source(&dir, &dir.join("state.json"), Some("00")).unwrap_err().starts_with(UNKNOWN_RULES), "a recorded hash this program was not built with");
        // an old state.json without the field still loads, with no hash
        let v: Vault = serde_json::from_str(r#"{"covenant_id":"0000000000000000000000000000000000000000000000000000000000000000","state":{"stage":0,"dest_hash":[],"amount":0,"window_start":0,"spent":0},"pending_to":null,"history":[]}"#).unwrap();
        assert!(v.contract_sha256.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pin_refuses_a_folder_pinned_to_other_rules() {
        let dir = std::env::temp_dir().join(format!("kv-repin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sha = pin_source(&dir).unwrap();
        assert_eq!(pin_source(&dir).unwrap(), sha, "identical bytes pin again");
        let path = dir.join(SOURCE_FILE);
        let old = format!("{}\n// v-old\n", std::fs::read_to_string(&path).unwrap());
        std::fs::write(&path, &old).unwrap();
        let err = pin_source(&dir).unwrap_err();
        assert!(err.starts_with("Refused: this folder is pinned to another version of the rules (sha "), "{err}");
        assert!(err.contains(&format!("{}... vs {}...", &sha256_hex(old.as_bytes())[..8], &sha[..8])) && err.ends_with("Open the new vault in a new folder."), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old, "the pinned copy was not touched");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_program_knows_its_own_rules_and_the_reviewed_history() {
        assert_eq!(embedded_sha256(), sha256_hex(EMBEDDED_SOURCE.as_bytes()));
        assert_eq!(known_sha256s().len(), 2 + HISTORY_SOURCES.len());
        assert!(is_known_source(embedded_sha256()) && !is_known_source("00"));
        for (_, s) in HISTORY_SOURCES {
            assert!(is_known_source(&sha256_hex(s.as_bytes())));
        }
        let dir = std::env::temp_dir().join(format!("kv-known-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        // fund pins the built-in bytes, whatever the repo folder holds
        assert_eq!(pin_source(&dir).unwrap(), embedded_sha256());
        assert_eq!(std::fs::read_to_string(dir.join(SOURCE_FILE)).unwrap(), EMBEDDED_SOURCE);
        // a folder pinned to rules nobody reviewed is refused, with and without a recorded hash
        let stranger = format!("{EMBEDDED_SOURCE}\n// edited by hand\n");
        std::fs::write(dir.join(SOURCE_FILE), &stranger).unwrap();
        let sha = sha256_hex(stranger.as_bytes());
        let err = load_source(&dir, &dir.join("state.json"), Some(&sha)).unwrap_err();
        assert!(err.starts_with(UNKNOWN_RULES) && err.contains(&sha[..8]) && err.contains("program version"), "{err}");
        assert!(load_source(&dir, &dir.join("state.json"), None).unwrap_err().starts_with(UNKNOWN_RULES));
        // a vault opened under a reviewed earlier version still drives
        let (_, v4) = HISTORY_SOURCES[2];
        std::fs::write(dir.join(SOURCE_FILE), v4).unwrap();
        assert_eq!(load_source(&dir, &dir.join("state.json"), Some(&sha256_hex(v4.as_bytes()))).unwrap(), v4);
        // no pin yet: the built-in rules
        std::fs::remove_file(dir.join(SOURCE_FILE)).unwrap();
        assert_eq!(load_source(&dir, &dir.join("state.json"), None).unwrap(), EMBEDDED_SOURCE);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The first v6 text and the built-in source differ in comments only: the built-in code hashes to V6_CODE_SHA,
    /// the value the first text gives; a folder pinned to the first text loads, and `fund` re-pins it.
    #[test]
    fn first_v6_text_is_the_built_in_code() {
        assert_eq!(sha256_hex(code_only(&EMBEDDED_SOURCE.replace("\r\n", "\n")).as_bytes()), V6_CODE_SHA);
        assert!(is_known_source(FIRST_V6_SHA));
        assert_eq!(equivalent_of(FIRST_V6_SHA), Some(EMBEDDED_SOURCE));
        assert_eq!(equivalent_of(embedded_sha256()), None);
        let dir = std::env::temp_dir().join(format!("kv-first-v6-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        // An account opened under the first text, whose folder already holds the built-in text: it still loads.
        std::fs::write(dir.join(SOURCE_FILE), EMBEDDED_SOURCE).unwrap();
        assert_eq!(load_source(&dir, &dir.join("state.json"), Some(FIRST_V6_SHA)).unwrap(), EMBEDDED_SOURCE);
        assert!(restore_source(&dir, FIRST_V6_SHA).is_ok());
        assert_eq!(pin_source(&dir).unwrap(), embedded_sha256());
    }
}
