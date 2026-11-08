//! Keys of one vault: the optional secrets (spend, three stop keys, heir, three cold sheets,
//! funding) and the `public` part every device needs to derive the vault's addresses.
//! On disk (`keys.json`) the secrets are locked with a passphrase while the `public` block stays
//! readable: PBKDF2-HMAC-SHA256 key stretching, an HMAC-SHA256 counter stream and an HMAC-SHA256
//! tag (encrypt-then-MAC). Those are the strongest blocks already in the lock file; no argon2,
//! scrypt or AES-GCM crate is there. `keys --plain` keeps the old plain form for tests; old plain
//! files still load, with one note. Scripts set `KASPAVAULT_PASSPHRASE` instead of typing.

use crate::contract::{Dials, INHERIT_DELAY_NONE, sha256_hex, spk_hash};
use crate::network::Network;
use kaspa_addresses::{Address, Prefix, Version};
use kaspa_txscript::script_builder::ScriptBuilder;
use kaspa_txscript::{extract_script_pub_key_address, multisig_redeem_script, pay_to_script_hash_script};
use secp256k1::rand::RngCore;
use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::OnceLock;
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::Zeroize;

/// The rule block form this program writes. 2: every rule is present and `fund` checks it (item 18).
pub const PUBLIC_VERSION: u32 = 2;
fn legacy_version() -> u32 {
    1
}
fn is_legacy_version(v: &u32) -> bool {
    *v < PUBLIC_VERSION
}
fn is_off(b: &bool) -> bool {
    !*b
}

/// What every device needs to know the vault's addresses, with no secret in it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Public {
    /// Form of this block. Blocks from before the field load as 1 and are written back without it,
    /// so their code does not move; `fund` refuses anything below PUBLIC_VERSION or incomplete.
    #[serde(default = "legacy_version", skip_serializing_if = "is_legacy_version")]
    pub v: u32,
    pub spend_x: String,
    pub cancel_x: String,
    /// The safe: P2SH of a 2-of-3 schnorr multisig over `cold_x` (or a legacy single pay-to-pubkey when `cold_x` is empty).
    pub cold_address: String,
    /// The three x-only cold keys in redeem-script order; empty on a legacy single-key safe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cold_x: Vec<String>,
    /// Second and third stop keys; an empty slot repeats `cancel_x` (vaults 1-6 have none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cancel2_x: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cancel3_x: String,
    /// The heir key and its pay-to-pubkey address; empty = no heir (the slot repeats the cancel
    /// key and the cold script, with a silence clock about 12.7 years out, INHERIT_DELAY_NONE, that
    /// no coin is expected to reach; `keys` always makes or takes a heir, so only old vaults and a phone that made
    /// everything itself without a heir (`fresh_phone_all`) have none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub heir_x: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub heir_address: String,
    /// v5: the auto-heir dial (constructor `autoHeir`). On, anyone (the watcher) may send the vault
    /// to the heir address once the silence has passed, with no key (`inherit-open`); off, only the
    /// heir's own key inherits. Set by `keys --auto-heir`; keys.json files from before v5 load as off.
    #[serde(default)]
    pub auto_heir: bool,
    /// v6: the hard-limit dial (constructor `hardLimit`). On, the contract's `request`
    /// entry fails outright, so no payment above the limit can ever wait out the notice period:
    /// money above the limit leaves only through a stop key (to the safe) or the heir. Set by
    /// `keys --hard-limit`; written only when on, so every keys.json from before v6 keeps its code.
    #[serde(default, skip_serializing_if = "is_off")]
    pub hard_limit: bool,
    /// The network every address in this block belongs to: "testnet" or "mainnet". Blocks from before
    /// the word load as empty (all testnet) and are written back without it, so their code does not move.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub network: String,
}

impl Public {
    /// The x-only key of stop slot `which` (0-based, as the contract counts); an empty slot repeats the first.
    pub fn stop_x(&self, which: usize) -> &str {
        let x = match which {
            1 => &self.cancel2_x,
            2 => &self.cancel3_x,
            _ => &self.cancel_x,
        };
        if x.is_empty() { &self.cancel_x } else { x }
    }
    /// The network this block's addresses are on (its own record; main refuses a block whose record
    /// does not match the `--network` flag before anything here runs).
    pub fn network(&self) -> Network {
        Network::of_record(&self.network)
    }
    /// The heir's silence clock as the script gets it: the vault's heir delay dial, or the 12.7-year INHERIT_DELAY_NONE when there is no heir.
    pub fn inherit_delay(&self, dials: &Dials) -> i64 {
        if self.heir_x.is_empty() { INHERIT_DELAY_NONE } else { dials.heir_delay }
    }
    /// The heir slot of the constructor: (key, script hash, silence clock). No heir: cancel key, cold hash, far out.
    pub fn heir_slot(&self, cold_hash: Vec<u8>, dials: &Dials) -> (Vec<u8>, Vec<u8>, i64) {
        if self.heir_x.is_empty() {
            (unhex(&self.cancel_x), cold_hash, self.inherit_delay(dials))
        } else {
            (unhex(&self.heir_x), spk_hash(&self.network().to_spk(&self.heir_address)), self.inherit_delay(dials))
        }
    }
    /// The 2-of-3 redeem script, or None on a legacy single-key safe.
    /// The rule block code: SHA-256 (hex) of this block as compact JSON in field order. The same
    /// block gives the same code on every device; any edit to a key, an address or the dial moves it.
    /// `keys`, `split-keys`, `fund` and `status` print it; state.json and the offline keys.json keep it.
    pub fn sha256(&self) -> String {
        sha256_hex(serde_json::to_string(self).expect("public block is plain JSON").as_bytes())
    }
    /// `fund` runs this before it locks money under the block: the current form, every rule present,
    /// no key doing two jobs. Returns the rules in bank words (also printed by `status`).
    pub fn check_complete(&self) -> Result<Vec<String>, String> {
        if self.v < PUBLIC_VERSION {
            return Err(format!(
                "The rule block in keys.json is an older form (version {}; this program writes {PUBLIC_VERSION}). Make a new vault with `keys`, or open this one with the program version that made it.",
                self.v
            ));
        }
        let mut missing = vec![];
        // No heir is a whole rule (both heir fields empty: `fresh_phone_all` without a heir); one of the two empty is a torn block.
        let no_heir = self.heir_x.is_empty() && self.heir_address.is_empty();
        for (name, value) in [("spend_x", &self.spend_x), ("cancel_x", &self.cancel_x), ("cancel2_x", &self.cancel2_x), ("cancel3_x", &self.cancel3_x), ("cold_address", &self.cold_address), ("heir_x", &self.heir_x), ("heir_address", &self.heir_address)] {
            if value.is_empty() && !(no_heir && name.starts_with("heir")) {
                missing.push(name.to_string());
            }
        }
        if self.cold_x.len() != 3 {
            missing.push(format!("cold_x (three sheets, found {})", self.cold_x.len()));
        }
        if !missing.is_empty() {
            return Err(format!("The rule block in keys.json is missing: {}. Restore the original keys.json, or make a new vault with `keys`.", missing.join(", ")));
        }
        let distinct: std::collections::BTreeSet<&str> = [&self.spend_x, &self.cancel_x, &self.cancel2_x, &self.cancel3_x, &self.heir_x].into_iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
        if distinct.len() != if no_heir { 4 } else { 5 } {
            return Err("The rule block in keys.json repeats a key: the spend key, the three stop keys and the heir key must all differ. Make a new vault with `keys`.".into());
        }
        Ok(self.rules())
    }
    /// The rules this block locks money under, one line each.
    pub fn rules(&self) -> Vec<String> {
        let stops: std::collections::BTreeSet<&str> = [&self.cancel_x, &self.cancel2_x, &self.cancel3_x].into_iter().filter(|x| !x.is_empty()).map(String::as_str).collect();
        vec![
            format!("rule block form {}", self.v),
            format!("stop keys: {} distinct{}", stops.len(), if stops.len() < 3 { " (an empty slot repeats stop key 1)" } else { "" }),
            format!("safe: {}", if self.cold_x.len() == 3 { "2-of-3 cold sheets" } else { "one cold key (legacy)" }),
            if self.heir_x.is_empty() { "heir: none (the silence clock never fires)".to_string() } else { format!("heir: yes, {} (after the silence set by the vault's heir delay dial)", self.heir_address) },
            format!("auto-heir: {}", if self.auto_heir { "on (after the silence anyone may send the vault to the heir)" } else { "off (only the heir's own key inherits)" }),
            format!("hard limit: {}", if self.hard_limit { "on (no payment above the limit can wait; above the limit only the stop keys or the heir move money)" } else { "off (a payment above the limit may wait out the notice period)" }),
        ]
    }
    pub fn cold_redeem(&self) -> Option<Vec<u8>> {
        (self.cold_x.len() == 3).then(|| cold_redeem_script(&self.cold_x.iter().map(|x| unhex(x)).collect::<Vec<_>>()))
    }
}

/// 2-of-3 redeem script over the three x-only cold keys, in key order (`2 <k1> <k2> <k3> 3 OP_CHECKMULTISIG`).
pub fn cold_redeem_script(xs: &[Vec<u8>]) -> Vec<u8> {
    let keys: Vec<[u8; 32]> = xs.iter().map(|x| <[u8; 32]>::try_from(x.as_slice()).expect("x-only key")).collect();
    multisig_redeem_script(keys.iter(), 2).expect("redeem script")
}
pub fn p2sh_address(prefix: Prefix, redeem: &[u8]) -> Address {
    extract_script_pub_key_address(&pay_to_script_hash_script(redeem), prefix).expect("p2sh address")
}
/// P2SH signature script: the signatures in redeem-script key order, then the redeem script as one push
/// (kaspa-txscript standard/multisig.rs tests: OpData65 sig pushes, then `add_data(redeem)`; OP_CHECKMULTISIG
/// walks the keys forward, so a signature out of key order fails).
pub fn multisig_sigscript(sigs: &[Vec<u8>], redeem: &[u8]) -> Vec<u8> {
    let mut b = ScriptBuilder::new();
    for s in sigs {
        b.add_data(s).unwrap();
    }
    b.add_data(redeem).unwrap();
    b.drain()
}

/// Secrets are optional so a device can hold only the keys it needs: the online side
/// keeps spend and funding, the offline side keeps cancel and cold. `public` lets a side
/// without a secret still derive every vault address.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Keys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<[u8; 32]>,
    /// Second and third stop keys (contract slots 1 and 2); missing = that slot repeats `cancel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel2: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel3: Option<[u8; 32]>,
    /// The heir's key. For this testnet run it stays in the online file so `inherit` can be
    /// driven from here; a real heir keeps their own key on their own device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heir: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold: Option<[u8; 32]>,
    /// Second and third cold sheets of the 2-of-3 safe; both missing = legacy single-key safe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold2: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cold3: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<Public>,
    /// The rule block code of `public` as `split-keys` wrote it (see `Public::sha256`). On the offline
    /// device `sign` refuses a block that no longer hashes to it; files from before it load as none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_sha256: Option<String>,
    /// The funding (top-up) key's public half on a device that keeps the secret elsewhere (the phone's
    /// signer store): `Ctx::funding_addr` derives the top-up address from it. Outside `Public`, so the
    /// rule block code does not move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funding_x: Option<String>,
}

impl std::fmt::Debug for Keys {
    /// Names the secrets that are here and shows the public block; never a secret byte.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = [("spend", &self.spend), ("cancel", &self.cancel), ("cancel2", &self.cancel2), ("cancel3", &self.cancel3), ("heir", &self.heir), ("cold", &self.cold), ("cold2", &self.cold2), ("cold3", &self.cold3), ("funding", &self.funding)];
        let present: Vec<&str> = names.iter().filter(|(_, s)| s.is_some()).map(|(n, _)| *n).collect();
        f.debug_struct("Keys").field("secrets_present", &present).field("public", &self.public).field("public_sha256", &self.public_sha256).finish()
    }
}

/// The non-secret part of a keys.json: the `public` block, its code and the funding key's public half;
/// what the networked watcher and the phone (whose keys.json holds nothing else) read.
#[derive(Deserialize)]
struct PublicOnly {
    public: Option<Public>,
    #[serde(default)]
    public_sha256: Option<String>,
    #[serde(default)]
    funding_x: Option<String>,
}

/// Secret names of the three stop slots, in contract order.
const STOP_NAMES: [&str; 3] = ["cancel", "cancel2", "cancel3"];

/// What `keys --offline` hands the online device: the public side of the stop keys and the safe, no secret.
#[derive(Serialize, Deserialize, Debug)]
pub struct OfflineExport {
    pub kind: String,
    pub v: u32,
    pub cancel_x: String,
    pub cancel2_x: String,
    pub cancel3_x: String,
    pub cold_x: Vec<String>,
    pub cold_address: String,
    /// The network the offline device was started for; the online device refuses another.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub network: String,
}
pub const OFFLINE_EXPORT_KIND: &str = "offline-public";

/// What `fresh_phone_all` hands the phone: the public-only keys, the 192 secret bytes to seal, the three safe sheets as words.
pub type PhoneBorn = (Keys, [u8; 192], [[String; 24]; 3]);

/// What the online device hands back after `keys --import-public`: the whole rule block with its code.
#[derive(Serialize, Deserialize, Debug)]
pub struct PublicExport {
    pub kind: String,
    pub public: Public,
    pub public_sha256: String,
}
pub const PUBLIC_EXPORT_KIND: &str = "vault-public";

/// Environment variable scripts set instead of typing the passphrase on the console.
pub const PASSPHRASE_ENV: &str = "KASPAVAULT_PASSPHRASE";
/// Key stretching rounds (PBKDF2-HMAC-SHA256) for a new locked keys.json; the file records its own count.
pub const KEY_STRETCH_ROUNDS: u32 = 600_000;
/// Fewest rounds a locked file may claim; fewer and the file is refused as tampered.
pub const KEY_STRETCH_FLOOR: u32 = 1_000;
const LOCK_FORM: &str = "kv-lock-1";

/// A locked keys.json: the secrets sealed under the passphrase, the public block beside them in the
/// clear (the watcher, `peek_public` and the anchor check read it with no passphrase). The tag covers
/// the public block too, so a swapped block is caught the moment the file is unlocked.
#[derive(Serialize, Deserialize)]
struct Locked {
    locked: String,
    rounds: u32,
    salt: String,
    nonce: String,
    secrets: String,
    tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public: Option<Public>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_sha256: Option<String>,
}

/// HMAC-SHA256 over the concatenation of `parts` (RFC 2104).
fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&ipad);
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA256 (RFC 8018), two 32-byte blocks: the cipher key and the tag key.
fn stretch(pass: &[u8], salt: &[u8], rounds: u32) -> ([u8; 32], [u8; 32]) {
    let block = |i: u32| {
        let mut u = hmac_sha256(pass, &[salt, &i.to_be_bytes()]);
        let mut t = u;
        for _ in 1..rounds {
            u = hmac_sha256(pass, &[&u]);
            for (t, u) in t.iter_mut().zip(u.iter()) {
                *t ^= u;
            }
        }
        t
    };
    (block(1), block(2))
}

/// Counter-mode stream from HMAC-SHA256(key, nonce || counter), XORed over `data` in place.
fn keystream_xor(key: &[u8; 32], nonce: &[u8], data: &mut [u8]) {
    for (i, chunk) in data.chunks_mut(32).enumerate() {
        let block = hmac_sha256(key, &[nonce, &(i as u32).to_be_bytes()]);
        for (d, k) in chunk.iter_mut().zip(block.iter()) {
            *d ^= k;
        }
    }
}

fn seal_tag(mac_key: &[u8; 32], salt: &[u8], nonce: &[u8], aad: &[u8], ct: &[u8]) -> [u8; 32] {
    hmac_sha256(mac_key, &[LOCK_FORM.as_bytes(), salt, nonce, &(aad.len() as u64).to_be_bytes(), aad, ct])
}

fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

static PASSPHRASE: OnceLock<String> = OnceLock::new();

/// The passphrase for this run: PASSPHRASE_ENV, else typed on the console (twice when `confirm`,
/// for a new file). Asked once per run, so split-keys locks both halves with the same one.
fn passphrase(prompt: &str, confirm: bool) -> Result<String, String> {
    if let Some(p) = PASSPHRASE.get() {
        return Ok(p.clone());
    }
    let p = match std::env::var(PASSPHRASE_ENV) {
        Ok(p) if !p.is_empty() => p,
        _ => {
            let first = read_hidden(prompt)?;
            if first.is_empty() {
                return Err(format!("An empty passphrase is not allowed. Type one, or set {PASSPHRASE_ENV} for scripts."));
            }
            if confirm && read_hidden("Type it again: ")? != first {
                return Err("The two passphrases differ. Run the command again.".into());
            }
            first
        }
    };
    Ok(PASSPHRASE.get_or_init(|| p).clone())
}

/// One line from the console with the typing hidden where the system lets us (PowerShell's secure
/// read on Windows, `stty -echo` elsewhere); when that is not possible it says so and reads plainly.
fn read_hidden(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let (program, args): (&str, &[&str]) = if cfg!(windows) {
        ("powershell", &["-NoProfile", "-Command", "$s = Read-Host -AsSecureString; $b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($s); [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b)"])
    } else {
        ("sh", &["-c", "stty -echo 2>/dev/null; IFS= read -r p; stty echo 2>/dev/null; printf '%s' \"$p\""])
    };
    let hidden = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end_matches(['\r', '\n']).to_string());
    println!();
    match hidden {
        Some(p) => Ok(p),
        None => {
            println!("(this console cannot hide the typing; what you type will show)");
            let mut s = String::new();
            std::io::stdin().read_line(&mut s).map_err(|e| format!("could not read the passphrase: {e}"))?;
            Ok(s.trim_end_matches(['\r', '\n']).to_string())
        }
    }
}

/// An x-only public key from 64 hex characters, or a refusal naming `what`.
fn x_only_hex(x: &str, what: &str) -> Result<Vec<u8>, String> {
    let mut v = vec![0u8; x.len() / 2];
    let ok = x.len() == 64 && faster_hex::hex_decode(x.as_bytes(), &mut v).is_ok() && XOnlyPublicKey::from_slice(&v).is_ok();
    if ok { Ok(v) } else { Err(format!("the {what} key must be an x-only public key: 64 hex characters (got {} characters)", x.len())) }
}

/// `--which 1|2|3` on the command line to the contract's 0-based stop slot.
pub fn stop_slot(which: u8) -> Result<usize, String> {
    (1..=3).contains(&which).then(|| which as usize - 1).ok_or_else(|| format!("--which must be 1, 2 or 3 (got {which})"))
}

impl Keys {
    /// Nine fresh secrets with their public part filled in, on `net`.
    pub fn fresh(net: &Network) -> Keys {
        let secp = Secp256k1::new();
        let fresh = || Some(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
        let mut keys = Keys { spend: fresh(), cancel: fresh(), cancel2: fresh(), cancel3: fresh(), heir: fresh(), cold: fresh(), cold2: fresh(), cold3: fresh(), funding: fresh(), public: None, public_sha256: None, funding_x: None };
        keys.public = Some(keys.derive_public(net));
        keys
    }
    /// Offline device (item 17): only the three stop keys and the three cold sheets are born here.
    /// No public block yet; `offline_export` goes to the online device and the block comes back.
    pub fn fresh_offline() -> Keys {
        let secp = Secp256k1::new();
        let fresh = || Some(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
        Keys { cancel: fresh(), cancel2: fresh(), cancel3: fresh(), cold: fresh(), cold2: fresh(), cold3: fresh(), ..Default::default() }
    }
    /// Online device (item 17): the spend and funding keys are born here; the stop keys and the safe
    /// come from the offline export. The heir's own public key goes in when given; without it a heir
    /// key is made here (testnet only; a real heir keeps their key on their own device).
    pub fn fresh_online(net: &Network, export: &OfflineExport, heir_x: Option<&str>, auto_heir: bool, hard_limit: bool) -> Result<Keys, String> {
        Self::check_export(net, export)?;
        x_only_hex(&export.cancel_x, "cancel_x")?;
        let secp = Secp256k1::new();
        let fresh = || Some(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
        let mut keys = Keys { spend: fresh(), funding: fresh(), ..Default::default() };
        let (heir_x, heir_address) = match heir_x {
            Some(x) => (x.to_ascii_lowercase(), p2pk_address_x(net.prefix, x)?.to_string()),
            None => {
                keys.heir = fresh();
                let k = keypair(keys.heir.as_ref().unwrap());
                (faster_hex::hex_string(&xonly(&k)), p2pk_address(net.prefix, &k).to_string())
            }
        };
        let public = Public {
            v: PUBLIC_VERSION,
            spend_x: faster_hex::hex_string(&xonly(&keys.spend_key())),
            cancel_x: export.cancel_x.clone(),
            cold_address: export.cold_address.clone(),
            cold_x: export.cold_x.clone(),
            cancel2_x: export.cancel2_x.clone(),
            cancel3_x: export.cancel3_x.clone(),
            heir_x,
            heir_address,
            auto_heir,
            hard_limit,
            network: net.name.into(),
        };
        keys.public_sha256 = Some(public.sha256());
        keys.public = Some(public);
        Ok(keys)
    }
    /// What every export from `keys --offline` must get right before a key is made against it: its kind,
    /// its network, three valid cold sheets making its cold address, valid stop keys 2 and 3.
    fn check_export(net: &Network, export: &OfflineExport) -> Result<(), String> {
        if export.kind != OFFLINE_EXPORT_KIND {
            return Err(format!("this file is not what `keys --offline` writes (kind `{}`); copy offline-public.json from the offline device", export.kind));
        }
        net.check_recorded(&export.network, "offline-public.json")?;
        if export.cold_x.len() != 3 {
            return Err(format!("the offline export has {} cold sheets, not three; run `keys --offline` again on the offline device", export.cold_x.len()));
        }
        for (name, x) in [("cancel2_x", &export.cancel2_x), ("cancel3_x", &export.cancel3_x), ("cold_x", &export.cold_x[0]), ("cold_x", &export.cold_x[1]), ("cold_x", &export.cold_x[2])] {
            x_only_hex(x, name)?;
        }
        if p2sh_address(net.prefix, &cold_redeem_script(&export.cold_x.iter().map(|x| unhex(x)).collect::<Vec<_>>())).to_string() != export.cold_address {
            return Err("the offline export's cold address does not match its three cold sheets; run `keys --offline` again on the offline device".into());
        }
        Ok(())
    }
    /// The drawer device of a phone wallet (D4): stop keys 2 and 3 and the three cold sheets only. Stop
    /// key 1 is born on the phone, so `offline_export` carries an empty `cancel_x` and `fresh_phone` fills it.
    pub fn fresh_offline_for_phone() -> Keys {
        let mut keys = Keys::fresh_offline();
        keys.cancel.zeroize();
        keys
    }
    /// The phone (D4): the card (spend), emergency stop key 1 (cancel) and the top-up key (funding) are
    /// born here; stop keys 2 and 3 and the safe come from the drawer device's export, which must leave
    /// stop key 1 empty (`keys --offline --for-phone`); the heir's public key is required (no heir secret
    /// is ever made on a phone). Returns the keys with no secret in them (the rule block, its code and the
    /// funding key's public half) and the three secrets as one block, [spend 32][stop 32][funding 32],
    /// for the caller to seal and zero.
    pub fn fresh_phone(net: &Network, export: &OfflineExport, heir_x: &str, auto_heir: bool, hard_limit: bool) -> Result<(Keys, [u8; 96]), String> {
        Self::check_export(net, export)?;
        if !export.cancel_x.is_empty() {
            return Err("The drawer device's file already holds a stop key 1. On the drawer device run `keys --offline --for-phone`, which leaves stop key 1 to be made on this phone.".into());
        }
        let (heir_x, heir_address) = Self::heir_of(net, heir_x)?;
        let secp = Secp256k1::new();
        let fresh = || Some(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
        let keys = Keys { spend: fresh(), cancel: fresh(), funding: fresh(), ..Default::default() };
        Self::finish_phone(keys, net, export, heir_x, heir_address, auto_heir, hard_limit)
    }
    /// Everything on this phone (no drawer device): the card, the three stop keys, the top-up key and the three
    /// cold sheets of the 2-of-3 safe are all born here, the way `keys` makes them on one laptop. The heir's own
    /// public key is taken when given; empty means no heir: both heir fields stay empty and the heir slot repeats
    /// stop key 1 to the safe behind the 12.7-year INHERIT_DELAY_NONE silence clock, as a vault with no heir has
    /// always been built (no heir secret is ever made on a phone). Returns the keys with no secret in them, the
    /// six phone secrets as one block for the caller to seal ([spend 32][stop 32][funding 32][cold 32][cold2 32][cold3 32];
    /// the phone keeps the safe's keys too, since a first-time user does not write the sheets down) and the
    /// three cold sheets as the same secrets in 24 words each (`sheet_words`): the caller shows them once for paper and zeroes them.
    pub fn fresh_phone_all(net: &Network, heir_x: &str, auto_heir: bool, hard_limit: bool) -> Result<PhoneBorn, String> {
        let (heir_x, heir_address) = if heir_x.trim().is_empty() { (String::new(), String::new()) } else { Self::heir_of(net, heir_x.trim())? };
        let secp = Secp256k1::new();
        let fresh = || Some(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
        let mut keys = Keys::fresh_offline();
        keys.spend = fresh();
        keys.funding = fresh();
        let export = keys.offline_export(net)?;
        let sheets = [&keys.cold, &keys.cold2, &keys.cold3].map(|s| sheet_words(s.as_ref().expect("made just above")));
        let mut block = [0u8; 192];
        for (i, s) in [&keys.cold, &keys.cold2, &keys.cold3].into_iter().enumerate() {
            block[96 + i * 32..128 + i * 32].copy_from_slice(s.as_ref().expect("made just above"));
        }
        let (keys, mut secrets) = Self::finish_phone(keys, net, &export, heir_x, heir_address, auto_heir, hard_limit)?;
        block[..96].copy_from_slice(&secrets);
        secrets.zeroize();
        Ok((keys, block, sheets))
    }
    /// The heir's own public key, checked and lower-cased, with its pay-to-pubkey address.
    pub(crate) fn heir_of(net: &Network, heir_x: &str) -> Result<(String, String), String> {
        x_only_hex(heir_x, "heir")?;
        let heir_x = heir_x.to_ascii_lowercase();
        let heir_address = p2pk_address_x(net.prefix, &heir_x)?.to_string();
        Ok((heir_x, heir_address))
    }
    /// The rule block of a phone's keys: stop keys 2 and 3 and the safe from `export`, the spend, stop 1 and
    /// funding keys from the secrets in `keys`. Then every secret in `keys` is wiped and the three phone secrets
    /// go out as one block.
    fn finish_phone(mut keys: Keys, net: &Network, export: &OfflineExport, heir_x: String, heir_address: String, auto_heir: bool, hard_limit: bool) -> Result<(Keys, [u8; 96]), String> {
        let public = Public {
            v: PUBLIC_VERSION,
            spend_x: faster_hex::hex_string(&xonly(&keys.spend_key())),
            cancel_x: faster_hex::hex_string(&xonly(&keys.cancel_key())),
            cold_address: export.cold_address.clone(),
            cold_x: export.cold_x.clone(),
            cancel2_x: export.cancel2_x.clone(),
            cancel3_x: export.cancel3_x.clone(),
            heir_x,
            heir_address,
            auto_heir,
            hard_limit,
            network: net.name.into(),
        };
        public.check_complete()?;
        keys.public_sha256 = Some(public.sha256());
        keys.public = Some(public);
        keys.funding_x = Some(faster_hex::hex_string(&xonly(&keys.funding_key())));
        let mut secrets = [0u8; 96];
        for (i, s) in [&keys.spend, &keys.cancel, &keys.funding].into_iter().enumerate() {
            secrets[i * 32..(i + 1) * 32].copy_from_slice(s.as_ref().expect("made just above"));
        }
        keys.wipe();
        Ok((keys, secrets))
    }
    /// Zeroes and drops every secret here; the public block, its code and `funding_x` stay. The phone's
    /// library calls this on every short-lived copy that signed (there is no `Drop`: construction with
    /// `..Default::default()`, used all over, is forbidden on a Drop type).
    pub fn wipe(&mut self) {
        for s in [&mut self.spend, &mut self.cancel, &mut self.cancel2, &mut self.cancel3, &mut self.heir, &mut self.cold, &mut self.cold2, &mut self.cold3, &mut self.funding] {
            s.zeroize();
        }
    }
    /// The public side of the stop keys and the safe on this (offline) device, for the online side. An
    /// absent stop key 1 (`fresh_offline_for_phone`) exports as an empty `cancel_x` for the phone to fill.
    pub fn offline_export(&self, net: &Network) -> Result<OfflineExport, String> {
        let hex_of = |s: &Option<[u8; 32]>, name: &str| s.as_ref().map(|s| faster_hex::hex_string(&xonly(&keypair(s)))).ok_or_else(|| format!("the {name} key is not on this device"));
        let cold_x = vec![hex_of(&self.cold, "cold")?, hex_of(&self.cold2, "cold2")?, hex_of(&self.cold3, "cold3")?];
        let cold_address = p2sh_address(net.prefix, &cold_redeem_script(&cold_x.iter().map(|x| unhex(x)).collect::<Vec<_>>())).to_string();
        Ok(OfflineExport { kind: OFFLINE_EXPORT_KIND.into(), v: PUBLIC_VERSION, cancel_x: self.cancel.as_ref().map(|s| faster_hex::hex_string(&xonly(&keypair(s)))).unwrap_or_default(), cancel2_x: hex_of(&self.cancel2, "cancel2")?, cancel3_x: hex_of(&self.cancel3, "cancel3")?, cold_x, cold_address, network: net.name.into() })
    }
    /// Takes the rule block the online device sent back; refused unless its code matches and the
    /// secrets on this device make it (check_self). The old block stays on a refusal.
    pub fn adopt_public(&mut self, e: &PublicExport) -> Result<(), String> {
        if e.kind != PUBLIC_EXPORT_KIND {
            return Err(format!("this file is not what the online device's `keys --import-public` writes (kind `{}`); copy vault-public.json from there", e.kind));
        }
        if e.public.sha256() != e.public_sha256 {
            return Err("The rule block in this file does not match the code written in it. Copy vault-public.json again from the online device.".into());
        }
        let keep = (self.public.take(), self.public_sha256.take());
        self.public = Some(e.public.clone());
        self.public_sha256 = Some(e.public_sha256.clone());
        if let Err(err) = self.check_self() {
            (self.public, self.public_sha256) = keep;
            return Err(err);
        }
        Ok(())
    }
    /// True when any secret is here (a public-only file has none).
    pub fn has_secret(&self) -> bool {
        [&self.spend, &self.cancel, &self.cancel2, &self.cancel3, &self.heir, &self.cold, &self.cold2, &self.cold3, &self.funding].iter().any(|s| s.is_some())
    }
    /// The locked form of this file: secrets sealed under `pass`, the public block in the clear.
    pub fn lock(&self, pass: &str, rounds: u32) -> Result<String, String> {
        let secrets = Keys { public: None, public_sha256: None, ..self.clone() };
        let mut ct = serde_json::to_string(&secrets).map_err(|e| format!("keys: {e}"))?.into_bytes();
        let (mut salt, mut nonce) = ([0u8; 16], [0u8; 16]);
        secp256k1::rand::thread_rng().fill_bytes(&mut salt);
        secp256k1::rand::thread_rng().fill_bytes(&mut nonce);
        let (enc_key, mac_key) = stretch(pass.as_bytes(), &salt, rounds);
        keystream_xor(&enc_key, &nonce, &mut ct);
        let aad = self.public.as_ref().map(|p| serde_json::to_string(p).expect("public block is plain JSON")).unwrap_or_default();
        let tag = seal_tag(&mac_key, &salt, &nonce, aad.as_bytes(), &ct);
        let l = Locked {
            locked: LOCK_FORM.into(),
            rounds,
            salt: faster_hex::hex_string(&salt),
            nonce: faster_hex::hex_string(&nonce),
            secrets: faster_hex::hex_string(&ct),
            tag: faster_hex::hex_string(&tag),
            public: self.public.clone(),
            public_sha256: self.public_sha256.clone(),
        };
        serde_json::to_string_pretty(&l).map_err(|e| format!("keys: {e}"))
    }
    fn unlock(l: &Locked, pass: &str) -> Result<Keys, String> {
        if l.rounds < KEY_STRETCH_FLOOR {
            return Err("rounds".into());
        }
        let salt = unhex(&l.salt);
        let nonce = unhex(&l.nonce);
        let mut ct = unhex(&l.secrets);
        let (enc_key, mac_key) = stretch(pass.as_bytes(), &salt, l.rounds);
        let aad = l.public.as_ref().map(|p| serde_json::to_string(p).expect("public block is plain JSON")).unwrap_or_default();
        if !same_bytes(&seal_tag(&mac_key, &salt, &nonce, aad.as_bytes(), &ct), &unhex(&l.tag)) {
            return Err("tag".into());
        }
        keystream_xor(&enc_key, &nonce, &mut ct);
        let mut keys: Keys = serde_json::from_slice(&ct).map_err(|_| "json".to_string())?;
        keys.public = l.public.clone();
        keys.public_sha256 = l.public_sha256.clone();
        Ok(keys)
    }
    /// True when the file on disk is in the locked form (false for a plain or missing file).
    pub fn is_locked(path: &Path) -> bool {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).is_some_and(|v| v.get("locked").is_some())
    }
    /// Writes this keys.json: locked with the passphrase (asked twice on the console, or
    /// PASSPHRASE_ENV) unless `plain`, the old form kept for tests.
    pub fn save(&self, path: &Path, plain: bool) -> Result<(), String> {
        let text = if plain {
            serde_json::to_string_pretty(self).map_err(|e| format!("keys: {e}"))?
        } else {
            let pass = passphrase("Choose a passphrase for keys.json: ", true)?;
            self.lock(&pass, KEY_STRETCH_ROUNDS)?
        };
        write_private(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    }
    /// The `public` block of a keys.json on disk and nothing else: Ok(None) when there is no such
    /// file or no block in it. The compile path hashes this against the code state.json recorded.
    pub fn peek_public(path: &Path) -> Result<Option<Public>, String> {
        Ok(Self::peek(path)?.and_then(|p| p.public))
    }
    /// The non-secret fields of a keys.json on disk; Ok(None) when there is no such file.
    fn peek(path: &Path) -> Result<Option<PublicOnly>, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
    }
    /// What this device can check about its own `public` block: a code written in the file must
    /// still match the block, and every key whose secret is here must derive to the block's entry
    /// (stop keys, spend, heir, the cold sheets and the safe address they make). The offline
    /// device runs this before it signs, so an edited block never gets a stop or cold signature.
    pub fn check_self(&self) -> Result<(), String> {
        let Some(p) = &self.public else { return Ok(()) };
        if let Some(code) = &self.public_sha256
            && *code != p.sha256() {
                return Err("The rule block in keys.json does not match the code written in it. Restore the original keys.json.".into());
            }
        let prefix = p.network().prefix;
        let hex_of = |k: &Keypair| faster_hex::hex_string(&xonly(k));
        let same = |secret: &Option<[u8; 32]>, stored: &str| secret.as_ref().is_none_or(|s| hex_of(&keypair(s)) == stored);
        let mut ok = same(&self.spend, &p.spend_x)
            && same(&self.cancel, &p.cancel_x)
            && same(&self.cancel2, p.stop_x(1))
            && same(&self.cancel3, p.stop_x(2))
            && same(&self.heir, &p.heir_x)
            && self.heir.as_ref().is_none_or(|s| p2pk_address(prefix, &keypair(s)).to_string() == p.heir_address);
        for (i, k) in self.cold_keys() {
            ok &= match p.cold_x.get(i - 1) {
                Some(x) => *x == hex_of(&k),
                None => p.cold_x.is_empty() && i == 1 && p2pk_address(prefix, &k).to_string() == p.cold_address,
            };
        }
        if p.cold_x.len() == 3 {
            ok &= p2sh_address(prefix, &cold_redeem_script(&p.cold_x.iter().map(|x| unhex(x)).collect::<Vec<_>>())).to_string() == p.cold_address;
        } else if !p.cold_x.is_empty() {
            ok = false;
        }
        if ok { Ok(()) } else { Err("The keys on this device do not match the rule block in its keys.json. Restore the original keys.json.".into()) }
    }
    /// The whole file, secrets included (every command but `watch`). A locked file takes the
    /// passphrase from PASSPHRASE_ENV or the console; a plain one loads with a note.
    pub fn load(path: &Path) -> Result<Keys, String> {
        Self::try_load_with(path, None)
    }
    /// `load` with the passphrase given (tests), or asked for when None.
    pub fn try_load_with(path: &Path, pass: Option<&str>) -> Result<Keys, String> {
        let text = std::fs::read_to_string(path).map_err(|_| format!("{} is missing; run `keys` first.", path.display()))?;
        if Self::is_locked(path) {
            let l: Locked = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if l.locked != LOCK_FORM {
                return Err(format!("{} is locked in a form this program does not know (`{}`). Use a newer program.", path.display(), l.locked));
            }
            let pass = match pass {
                Some(p) => p.to_string(),
                None => passphrase("Passphrase for keys.json: ", false)?,
            };
            return Self::unlock(&l, &pass).map_err(|_| format!("The passphrase is wrong, or {} was changed. Try again, or restore the original file.", path.display()));
        }
        // A fixed message: the parser's own would echo the field it tripped on, secrets included.
        let keys: Keys = serde_json::from_str(&text).map_err(|_| format!("{} is not a keys.json this program can read. Restore the original file.", path.display()))?;
        if keys.has_secret() {
            eprintln!("note: {} keeps its secrets in plain text. A new vault made with `keys` (without --plain) locks them with a passphrase.", path.display());
        }
        Ok(keys)
    }
    /// Only the `public` block; the secrets never leave the file (no passphrase needed). Refuses a keys.json without one.
    pub fn load_public(path: &Path) -> Result<Keys, String> {
        match Self::peek(path)? {
            Some(PublicOnly { public: Some(p), public_sha256, funding_x }) => Ok(Keys { public: Some(p), public_sha256, funding_x, ..Default::default() }),
            Some(_) => Err(format!("{} has no `public` part; run `keys --import-public` or split-keys first.", path.display())),
            None => Err(format!("{} is missing; run `keys` first.", path.display())),
        }
    }
    fn key(secret: &Option<[u8; 32]>, name: &str) -> Keypair {
        keypair(secret.as_ref().unwrap_or_else(|| panic!("the {name} key is not on this device")))
    }
    /// The secret of stop slot `which` (0-based): cancel, cancel2 or cancel3.
    pub fn stop_secret(&self, which: usize) -> &Option<[u8; 32]> {
        match which {
            1 => &self.cancel2,
            2 => &self.cancel3,
            _ => &self.cancel,
        }
    }
    /// Stop key `which` (0-based), or a clear refusal naming the missing secret.
    pub fn try_stop_key(&self, which: usize) -> Result<Keypair, String> {
        self.stop_secret(which).as_ref().map(keypair).ok_or_else(|| format!("stop key {} (secret `{}`) is not on this device", which + 1, STOP_NAMES[which.min(2)]))
    }
    fn spend_key(&self) -> Keypair {
        Self::key(&self.spend, "spend")
    }
    fn cancel_key(&self) -> Keypair {
        Self::key(&self.cancel, "cancel")
    }
    pub fn cold_key(&self) -> Keypair {
        Self::key(&self.cold, "cold")
    }
    pub fn funding_key(&self) -> Keypair {
        Self::key(&self.funding, "funding")
    }
    /// The cold sheets present on this device, numbered 1..=3 in redeem-script order.
    pub fn cold_keys(&self) -> Vec<(usize, Keypair)> {
        [&self.cold, &self.cold2, &self.cold3].iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|s| (i + 1, keypair(s)))).collect()
    }
    /// True when this is the old one-key safe (no cold2/cold3 anywhere, stored or secret).
    pub fn legacy_safe(&self) -> bool {
        self.public().cold_x.is_empty()
    }
    /// The public part: stored, or derived from the secrets when all of them are present.
    pub fn public(&self) -> Public {
        if let Some(p) = &self.public {
            return p.clone();
        }
        // A keys.json without a rule block is from before the network was recorded; all of those were testnet.
        self.derive_public(&Network::of_record(""))
    }
    /// The rule block the secrets on this device make, with its addresses on `net`.
    fn derive_public(&self, net: &Network) -> Public {
        let cold_x: Vec<String> = match (&self.cold2, &self.cold3) {
            (Some(_), Some(_)) => self.cold_keys().iter().map(|(_, k)| faster_hex::hex_string(&xonly(k))).collect(),
            _ => vec![],
        };
        let cold_address = match cold_x.len() {
            3 => p2sh_address(net.prefix, &cold_redeem_script(&cold_x.iter().map(|x| unhex(x)).collect::<Vec<_>>())).to_string(),
            _ => p2pk_address(net.prefix, &self.cold_key()).to_string(),
        };
        let hex_of = |s: &Option<[u8; 32]>| s.as_ref().map(|s| faster_hex::hex_string(&xonly(&keypair(s)))).unwrap_or_default();
        let heir_address = self.heir.as_ref().map(|s| p2pk_address(net.prefix, &keypair(s)).to_string()).unwrap_or_default();
        Public {
            v: PUBLIC_VERSION,
            spend_x: faster_hex::hex_string(&xonly(&self.spend_key())),
            cancel_x: faster_hex::hex_string(&xonly(&self.cancel_key())),
            cold_address,
            cold_x,
            cancel2_x: hex_of(&self.cancel2),
            cancel3_x: hex_of(&self.cancel3),
            heir_x: hex_of(&self.heir),
            heir_address,
            auto_heir: false,
            hard_limit: false,
            network: net.name.into(),
        }
    }
}

/// Writes a file only its owner may read: mode 600 on Unix, set on create and again on an existing
/// file. Windows has no mode bits; a file under the user's own profile is already closed to other accounts.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(bytes)?;
        return f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    std::fs::write(path, bytes)
}

pub fn unhex(s: &str) -> Vec<u8> {
    let mut v = vec![0u8; s.len() / 2];
    faster_hex::hex_decode(s.as_bytes(), &mut v).expect("hex");
    v
}
pub fn keypair(secret: &[u8; 32]) -> Keypair {
    Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(secret).unwrap())
}
pub fn xonly(k: &Keypair) -> Vec<u8> {
    k.x_only_public_key().0.serialize().to_vec()
}
pub fn p2pk_address(prefix: Prefix, k: &Keypair) -> Address {
    Address::new(prefix, Version::PubKey, &xonly(k))
}
/// Pay-to-pubkey address of an x-only key given as hex (the heir's own key).
pub fn p2pk_address_x(prefix: Prefix, x: &str) -> Result<Address, String> {
    Ok(Address::new(prefix, Version::PubKey, &x_only_hex(x, "heir")?))
}

/// The BIP-39 English list, one word per line (2048 lines; sha256 2f5eed53...24dbda). The safe sheets a phone
/// makes for itself are written in it; the laptop service's check words come from the same list.
pub(crate) const BIP39_WORDS_TEXT: &str = include_str!("bip39-english.txt");

/// The 2048 words in list order.
pub(crate) fn bip39_words() -> Vec<&'static str> {
    let words: Vec<&str> = BIP39_WORDS_TEXT.lines().collect();
    assert_eq!(words.len(), 2048, "bip39-english.txt");
    words
}

/// A cold sheet as 24 words for paper: the 32 secret bytes and their 8-bit check (the first byte of their
/// SHA-256) as 24 groups of 11 bits, each the index of a word. That is a BIP-39 mnemonic of 256 bits, so
/// `sheet_from_words` and any BIP-39 tool read it back to the same 32 bytes.
pub fn sheet_words(secret: &[u8; 32]) -> [String; 24] {
    let words = bip39_words();
    let mut bits = [0u8; 33];
    bits[..32].copy_from_slice(secret);
    bits[32] = Sha256::digest(secret)[0];
    let mut out: [String; 24] = std::array::from_fn(|_| String::new());
    for (i, w) in out.iter_mut().enumerate() {
        let idx = (0..11).fold(0usize, |acc, b| {
            let at = i * 11 + b;
            (acc << 1) | ((bits[at / 8] >> (7 - at % 8)) & 1) as usize
        });
        *w = words[idx].to_string();
    }
    bits.zeroize();
    out
}

/// The 32 secret bytes a 24-word sheet holds. Case and spaces around a word do not matter. Refused in plain
/// words, never echoing a word, when there are not 24, a word is not on the list, or the check does not come
/// out (a word mistyped from the paper).
pub fn sheet_from_words<S: AsRef<str>>(words: &[S]) -> Result<[u8; 32], String> {
    if words.len() != 24 {
        return Err(format!("A sheet has 24 words; {} were given.", words.len()));
    }
    let list = bip39_words();
    let mut bits = [0u8; 33];
    for (i, w) in words.iter().enumerate() {
        let w = w.as_ref().trim().to_ascii_lowercase();
        let idx = list.iter().position(|x| *x == w).ok_or_else(|| format!("Word {} is not a sheet word. Check its spelling on the paper.", i + 1))?;
        for b in 0..11 {
            if (idx >> (10 - b)) & 1 == 1 {
                let at = i * 11 + b;
                bits[at / 8] |= 1 << (7 - at % 8);
            }
        }
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bits[..32]);
    let checks = Sha256::digest(secret)[0] == bits[32];
    bits.zeroize();
    if !checks {
        secret.zeroize();
        return Err("The words do not check out: one of them is wrong. Compare each word with the paper.".into());
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{INHERIT_DELAY_TESTNET, KAS, spk_hash};
    use crate::tx::Ctx;
    use kaspa_consensus_core::Hash;
    use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
    use kaspa_consensus_core::tx::{Transaction, TransactionInput, TransactionOutpoint, TransactionOutput, UtxoEntry};
    use kaspa_txscript::pay_to_address_script;
    use std::path::PathBuf;

    fn fresh() -> Keypair {
        Keypair::new(&Secp256k1::new(), &mut secp256k1::rand::thread_rng())
    }

    /// Spends a fake P2SH 2-of-3 coin held by `safe` with `signers` (in the order given), through the
    /// same sign-measure-budget-sign path the client uses, and the engine check it ends with.
    fn try_spend(safe: &[Keypair; 3], signers: &[&Keypair]) -> Result<(), String> {
        let xs: Vec<Vec<u8>> = safe.iter().map(xonly).collect();
        let redeem = cold_redeem_script(&xs);
        let spk = pay_to_script_hash_script(&redeem);
        assert_eq!(p2sh_address(Prefix::Testnet, &redeem).version, Version::ScriptHash);
        let ctx = Ctx { dir: PathBuf::from("."), keys: Keys::default(), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        let outpoint = TransactionOutpoint::new(Hash::from_bytes([7u8; 32]), 0);
        let entries = vec![UtxoEntry::new(10 * KAS, spk, 1_000, false, None)];
        let inputs = vec![TransactionInput::new_with_compute_budget(outpoint, vec![], 0, 0)];
        let outputs = vec![TransactionOutput::new(9 * KAS, pay_to_address_script(&p2pk_address(Prefix::Testnet, &safe[0])))];
        let mut tx = Transaction::new(1, inputs, outputs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
        ctx.try_sign_and_budget(&mut tx, &entries, &|_, t| {
            let sigs: Vec<Vec<u8>> = signers.iter().map(|k| ctx.sign_input(t, &entries, 0, k)).collect();
            multisig_sigscript(&sigs, &redeem)
        })
    }

    #[test]
    fn two_of_three_safe() {
        let safe = [fresh(), fresh(), fresh()];
        let [k1, k2, k3] = &safe;
        let wrong = fresh();
        // any two of three, in key order
        try_spend(&safe, &[k1, k2]).expect("sheets 1,2");
        try_spend(&safe, &[k1, k3]).expect("sheets 1,3");
        try_spend(&safe, &[k2, k3]).expect("sheets 2,3");
        // one signature only
        assert!(try_spend(&safe, &[k1]).is_err(), "one signature must fail");
        assert!(try_spend(&safe, &[k3]).is_err(), "one signature must fail");
        // a stranger's key beside a real one
        assert!(try_spend(&safe, &[k1, &wrong]).is_err(), "wrong key must fail");
        assert!(try_spend(&safe, &[&wrong, k2]).is_err(), "wrong key must fail");
        // the same sheet twice
        assert!(try_spend(&safe, &[k2, k2]).is_err(), "one sheet twice must fail");
        // right sheets, wrong order: OP_CHECKMULTISIG walks the keys forward
        assert!(try_spend(&safe, &[k2, k1]).is_err(), "out-of-order signatures must fail");
    }

    #[test]
    fn legacy_and_new_public() {
        let secp = Secp256k1::new();
        let fresh = || secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes();
        let old = Keys { spend: Some(fresh()), cancel: Some(fresh()), cold: Some(fresh()), funding: Some(fresh()), ..Default::default() };
        assert!(old.legacy_safe());
        assert_eq!(Address::try_from(old.public().cold_address.as_str()).unwrap().version, Version::PubKey);
        let new = Keys { cold2: Some(fresh()), cold3: Some(fresh()), ..old };
        assert!(!new.legacy_safe());
        let p = new.public();
        assert_eq!(p.cold_x.len(), 3);
        assert_eq!(Address::try_from(p.cold_address.as_str()).unwrap().version, Version::ScriptHash);
        assert_eq!(p2sh_address(Prefix::Testnet, &p.cold_redeem().unwrap()).to_string(), p.cold_address);
        // an old keys.json without cold_x still loads as legacy
        let json = r#"{"public":{"spend_x":"00","cancel_x":"00","cold_address":"kaspatest:qpgjqaw6x7x826krf49calxygwjw7qx0hlwdd8g9zhqr3ja3xn9gush48satx"}}"#;
        let k: Keys = serde_json::from_str(json).unwrap();
        assert!(k.legacy_safe() && k.public().cold_redeem().is_none());
    }

    #[test]
    fn old_and_new_keys_json() {
        // vaults 1-6: no stop keys 2/3 and no heir; every fallback is the cancel key and the far-out clock
        let old = r#"{"spend":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"public":{"spend_x":"aa","cancel_x":"bb","cold_address":"kaspatest:qpgjqaw6x7x826krf49calxygwjw7qx0hlwdd8g9zhqr3ja3xn9gush48satx"}}"#;
        let k: Keys = serde_json::from_str(old).unwrap();
        assert!(k.spend.is_some() && k.cancel2.is_none() && k.heir.is_none());
        let p = k.public();
        assert!(p.cancel2_x.is_empty() && p.cancel3_x.is_empty() && p.heir_x.is_empty() && p.heir_address.is_empty());
        assert_eq!((p.stop_x(0), p.stop_x(1), p.stop_x(2)), ("bb", "bb", "bb"));
        assert_eq!(p.heir_slot(vec![9], &Dials::testnet()), (unhex("bb"), vec![9], INHERIT_DELAY_NONE));
        assert!(k.try_stop_key(1).unwrap_err().contains("cancel2"));
        // written back, the empty slots stay out of the file
        let json = serde_json::to_string(&k).unwrap();
        assert!(!json.contains("cancel2") && !json.contains("\"heir"), "{json}");

        // new shape: three stop secrets and an heir derive their public part
        let new = Keys::fresh(&Network::testnet());
        let p = new.public();
        assert_eq!((p.stop_x(1), p.stop_x(2)), (p.cancel2_x.as_str(), p.cancel3_x.as_str()));
        assert_ne!(p.stop_x(1), p.cancel_x);
        assert_eq!(Address::try_from(p.heir_address.as_str()).unwrap().version, Version::PubKey);
        assert_eq!(p.heir_slot(vec![], &Dials::testnet()), (unhex(&p.heir_x), spk_hash(&Network::testnet().to_spk(&p.heir_address)), INHERIT_DELAY_TESTNET));
        assert!(new.try_stop_key(2).is_ok());
        // round trip through JSON keeps every new field
        let back: Keys = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert!(back.cancel2.is_some() && back.cancel3.is_some() && back.heir.is_some());
        assert_eq!(back.public().heir_x, p.heir_x);
        // --which on the command line is 1-based, the contract's slot 0-based
        assert_eq!((stop_slot(1).unwrap(), stop_slot(3).unwrap()), (0, 2));
        assert!(stop_slot(0).is_err() && stop_slot(4).is_err());
    }

    #[test]
    fn auto_heir_dial_defaults_off() {
        // a keys.json from before v5 has no dial: it loads as off
        let old = r#"{"public":{"spend_x":"aa","cancel_x":"bb","cold_address":"kaspatest:qpgjqaw6x7x826krf49calxygwjw7qx0hlwdd8g9zhqr3ja3xn9gush48satx"}}"#;
        let k: Keys = serde_json::from_str(old).unwrap();
        assert!(!k.public().auto_heir);
        // `keys` writes it off; turned on, it rides through the file
        let mut new = Keys::fresh(&Network::testnet());
        assert!(!new.public().auto_heir);
        assert!(serde_json::to_string(&new).unwrap().contains("\"auto_heir\":false"));
        new.public.as_mut().unwrap().auto_heir = true;
        let back: Keys = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert!(back.public().auto_heir);
        // the watcher's public-only view keeps the dial too
        let dir = std::env::temp_dir().join(format!("kv-keys-dial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keys.json"), serde_json::to_string(&new).unwrap()).unwrap();
        assert!(Keys::load_public(&dir.join("keys.json")).unwrap().public().auto_heir);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// v6: the hard-limit dial loads off from every older file, is written only when
    /// on (so no open vault's rule block code moves), and moves the code when on.
    #[test]
    fn hard_limit_dial_defaults_off_and_keeps_old_codes() {
        let old = r#"{"spend_x":"aa","cancel_x":"bb","cold_address":"kaspatest:q","auto_heir":false}"#;
        let p: Public = serde_json::from_str(old).unwrap();
        assert!(!p.hard_limit);
        assert_eq!(serde_json::to_string(&p).unwrap(), old, "off is not written: the code of an open vault stays");
        let k = Keys::fresh(&Network::testnet());
        assert!(!k.public().hard_limit);
        assert!(!serde_json::to_string(&k).unwrap().contains("hard_limit"));
        let code = k.public().sha256();
        let mut on = k.public();
        on.hard_limit = true;
        assert!(serde_json::to_string(&on).unwrap().contains("\"hard_limit\":true"));
        assert_ne!(on.sha256(), code, "the dial is part of the rule block code");
        let back: Public = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(back.hard_limit);
        assert!(on.rules().iter().any(|r| r.starts_with("hard limit: on")), "{:?}", on.rules());
        assert!(k.public().rules().iter().any(|r| r.starts_with("hard limit: off")));
    }

    #[test]
    fn rule_block_code_is_stable_and_moves_with_the_block() {
        let k = Keys::fresh(&Network::testnet());
        let code = k.public().sha256();
        assert_eq!(code.len(), 64);
        // the same block read back from disk gives the same code; a public-only copy too
        let back: Keys = serde_json::from_str(&serde_json::to_string_pretty(&k).unwrap()).unwrap();
        assert_eq!(back.public().sha256(), code);
        let only = Keys { public: Some(k.public()), ..Default::default() };
        assert_eq!(only.public().sha256(), code);
        // any edit to the block moves the code: the dial, the safe, a stop key
        let mut p = k.public();
        p.auto_heir = true;
        assert_ne!(p.sha256(), code);
        let mut p = k.public();
        p.cold_address = p2pk_address(Prefix::Testnet, &fresh()).to_string();
        assert_ne!(p.sha256(), code);
        let mut p = k.public();
        p.cancel_x = faster_hex::hex_string(&xonly(&fresh()));
        assert_ne!(p.sha256(), code);
        // peek reads only the block, and says nothing when there is no file
        let dir = std::env::temp_dir().join(format!("kv-keys-code-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        std::fs::remove_file(&path).ok();
        assert_eq!(Keys::peek_public(&path).unwrap().map(|p| p.sha256()), None);
        std::fs::write(&path, serde_json::to_string_pretty(&k).unwrap()).unwrap();
        assert_eq!(Keys::peek_public(&path).unwrap().map(|p| p.sha256()), Some(code));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn offline_keys_refuse_a_public_block_their_secrets_disagree_with() {
        let full = Keys::fresh(&Network::testnet());
        assert_eq!(full.check_self(), Ok(()));
        let public = full.public();
        let code = public.sha256();
        let off = Keys { cancel: full.cancel, cancel2: full.cancel2, cancel3: full.cancel3, cold: full.cold, cold2: full.cold2, cold3: full.cold3, public: Some(public.clone()), public_sha256: Some(code.clone()), ..Default::default() };
        assert_eq!(off.check_self(), Ok(()));
        let edited = |edit: &dyn Fn(&mut Public)| {
            let mut p = public.clone();
            edit(&mut p);
            let code = p.sha256();
            Keys { public: Some(p), public_sha256: Some(code), ..off_clone(&off) }
        };
        // the safe moved: the cold sheets here do not make that address
        let err = edited(&|p| p.cold_address = p2pk_address(Prefix::Testnet, &fresh()).to_string()).check_self().unwrap_err();
        assert!(err.contains("do not match"), "{err}");
        // one cold sheet swapped in the list
        let err = edited(&|p| p.cold_x[1] = faster_hex::hex_string(&xonly(&fresh()))).check_self().unwrap_err();
        assert!(err.contains("do not match"), "{err}");
        // a stop key swapped
        let err = edited(&|p| p.cancel3_x = faster_hex::hex_string(&xonly(&fresh()))).check_self().unwrap_err();
        assert!(err.contains("do not match"), "{err}");
        // the block edited but the written code left alone
        let mut stale = off_clone(&off);
        stale.public.as_mut().unwrap().auto_heir = true;
        let err = stale.check_self().unwrap_err();
        assert!(err.contains("code written in it"), "{err}");
        // the block must agree with itself even where no secret is here: a lone swapped safe address is caught online too
        let online = Keys { spend: full.spend, funding: full.funding, public: Some(edited(&|p| p.cold_address = p2pk_address(Prefix::Testnet, &fresh()).to_string()).public()), ..Default::default() };
        assert!(online.check_self().unwrap_err().contains("do not match"));
        // but a whole safe replaced (sheets and address together) passes online, where no cold secret is: fund's anchor is the guard there
        let other: Vec<Vec<u8>> = (0..3).map(|_| xonly(&fresh())).collect();
        let swapped = edited(&|p| {
            p.cold_x = other.iter().map(|x| faster_hex::hex_string(x)).collect();
            p.cold_address = p2sh_address(Prefix::Testnet, &cold_redeem_script(&other)).to_string();
        });
        assert_eq!(Keys { spend: full.spend, funding: full.funding, public: swapped.public.clone(), ..Default::default() }.check_self(), Ok(()));
        assert!(swapped.check_self().is_err(), "the offline side, holding the sheets, refuses it");
        // but a swapped spend key is caught there
        let mut p = public.clone();
        p.spend_x = faster_hex::hex_string(&xonly(&fresh()));
        assert!(Keys { spend: full.spend, public: Some(p), ..Default::default() }.check_self().is_err());
        // a legacy one-key safe checks its pay-to-pubkey address
        let one = Keys { cold: full.cold, public: Some(Public { cold_x: vec![], cold_address: p2pk_address(Prefix::Testnet, &keypair(full.cold.as_ref().unwrap())).to_string(), ..public.clone() }), ..Default::default() };
        assert_eq!(one.check_self(), Ok(()));
    }

    fn off_clone(k: &Keys) -> Keys {
        Keys { cancel: k.cancel, cancel2: k.cancel2, cancel3: k.cancel3, cold: k.cold, cold2: k.cold2, cold3: k.cold3, public: k.public.clone(), public_sha256: k.public_sha256.clone(), ..Default::default() }
    }

    #[test]
    fn watcher_reads_only_the_public_part() {
        let dir = std::env::temp_dir().join(format!("kv-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        let full = Keys::fresh(&Network::testnet());
        std::fs::write(&path, serde_json::to_string(&full).unwrap()).unwrap();
        let k = Keys::load_public(&path).unwrap();
        assert!(k.spend.is_none() && k.cancel.is_none() && k.cold.is_none() && k.funding.is_none() && k.heir.is_none());
        assert_eq!(k.public().spend_x, full.public().spend_x);
        assert!(!serde_json::to_string(&k).unwrap().contains("\"spend\":"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn locked_keys_json_round_trips_and_refuses_a_wrong_passphrase() {
        let k = Keys::fresh(&Network::testnet());
        let dir = std::env::temp_dir().join(format!("kv-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        let text = k.lock("open sesame", KEY_STRETCH_FLOOR).unwrap();
        // no secret in the clear; the public block stays readable
        assert!(!text.contains(&serde_json::to_string(&k.spend.unwrap()).unwrap()));
        assert!(text.contains("\"locked\"") && text.contains(&k.public().spend_x), "{text}");
        std::fs::write(&path, &text).unwrap();
        assert!(Keys::is_locked(&path));
        let back = Keys::try_load_with(&path, Some("open sesame")).unwrap();
        assert_eq!((back.spend, back.cold3, back.funding), (k.spend, k.cold3, k.funding));
        assert_eq!(back.public().sha256(), k.public().sha256());
        let err = Keys::try_load_with(&path, Some("open sesamee")).unwrap_err();
        assert!(err.contains("passphrase is wrong"), "{err}");
        // the readers that need no passphrase still see the block
        assert_eq!(Keys::peek_public(&path).unwrap().unwrap().sha256(), k.public().sha256());
        assert_eq!(Keys::load_public(&path).unwrap().public().sha256(), k.public().sha256());
        // a changed public block is caught by the tag (the secrets were sealed over it)
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["public"]["auto_heir"] = serde_json::Value::Bool(!k.public().auto_heir);
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        assert!(Keys::try_load_with(&path, Some("open sesame")).unwrap_err().contains("was changed"));
        // a file claiming too few rounds is refused the same way
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["rounds"] = serde_json::Value::from(1);
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        assert!(Keys::try_load_with(&path, Some("open sesame")).is_err());
        // an old plain file still loads
        std::fs::write(&path, serde_json::to_string_pretty(&k).unwrap()).unwrap();
        assert!(!Keys::is_locked(&path));
        assert_eq!(Keys::try_load_with(&path, Some("unused")).unwrap().spend, k.spend);
        assert!(Keys::load(&dir.join("nothing.json")).unwrap_err().contains("run `keys` first"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hmac_and_stretch_match_the_published_vectors() {
        // RFC 4231 case 2
        let t = hmac_sha256(b"Jefe", &[b"what do ya want for nothing?"]);
        assert_eq!(faster_hex::hex_string(&t), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // PBKDF2-HMAC-SHA256("password", "salt", 1 round), first 32 bytes (RFC 7914 section 11)
        let (k1, k2) = stretch(b"password", b"salt", 1);
        assert_eq!(faster_hex::hex_string(&k1), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        assert_ne!(k1, k2);
        // the stream undoes itself and a tag mismatch is seen
        let mut data = b"nine secrets".to_vec();
        keystream_xor(&k1, b"n", &mut data);
        assert_ne!(data, b"nine secrets");
        keystream_xor(&k1, b"n", &mut data);
        assert_eq!(data, b"nine secrets");
        assert!(same_bytes(&[1, 2], &[1, 2]) && !same_bytes(&[1, 2], &[1, 3]) && !same_bytes(&[1], &[1, 0]));
    }

    #[test]
    fn rule_block_v2_is_complete_and_legacy_blocks_keep_their_code() {
        // a block from before the version: v = 1, nothing new written back, so its code does not move
        let old = r#"{"spend_x":"aa","cancel_x":"bb","cold_address":"kaspatest:q","auto_heir":false}"#;
        let p: Public = serde_json::from_str(old).unwrap();
        assert_eq!(p.v, 1);
        assert_eq!(serde_json::to_string(&p).unwrap(), old);
        assert_eq!(p.sha256(), sha256_hex(old.as_bytes()));
        let err = p.check_complete().unwrap_err();
        assert!(err.contains("older form") && err.contains("`keys`"), "{err}");
        assert!(p.rules().iter().any(|r| r.contains("1 distinct")), "{:?}", p.rules());
        // fresh keys write form 2 and every rule
        let full = Keys::fresh(&Network::testnet()).public();
        assert!(serde_json::to_string(&full).unwrap().starts_with("{\"v\":2,"));
        let rules = full.check_complete().unwrap();
        assert!(rules.iter().any(|r| r == "stop keys: 3 distinct") && rules.iter().any(|r| r.starts_with("heir: yes")), "{rules:?}");
        // a missing field refuses and names it
        let mut p = full.clone();
        p.cancel2_x.clear();
        assert!(p.check_complete().unwrap_err().contains("cancel2_x"));
        let mut p = full.clone();
        p.cold_x.pop();
        assert!(p.check_complete().unwrap_err().contains("cold_x"));
        let mut p = full.clone();
        p.heir_x.clear();
        assert!(p.check_complete().unwrap_err().contains("heir_x"), "one heir field empty is a torn block");
        p.heir_address.clear();
        assert!(p.check_complete().unwrap().iter().any(|r| r.starts_with("heir: none")), "both empty is a vault with no heir");
        // a repeated key refuses
        let mut p = full.clone();
        p.cancel3_x = p.cancel_x.clone();
        assert!(p.check_complete().unwrap_err().contains("repeats a key"));
    }

    #[test]
    fn offline_keys_and_the_online_import_make_one_block() {
        let off = Keys::fresh_offline();
        assert!(off.spend.is_none() && off.funding.is_none() && off.heir.is_none() && off.public.is_none());
        let export = off.offline_export(&Network::testnet()).unwrap();
        assert_eq!((export.kind.as_str(), export.v, export.cold_x.len()), (OFFLINE_EXPORT_KIND, PUBLIC_VERSION, 3));
        let heir = fresh();
        let heir_x = faster_hex::hex_string(&xonly(&heir));
        let on = Keys::fresh_online(&Network::testnet(), &export, Some(&heir_x), true, true).unwrap();
        assert!(on.heir.is_none() && on.spend.is_some() && on.funding.is_some() && on.cancel.is_none() && on.cold.is_none());
        let public = on.public();
        assert_eq!((public.v, public.heir_x.as_str(), public.auto_heir, public.hard_limit, public.cold_address.as_str()), (PUBLIC_VERSION, heir_x.as_str(), true, true, export.cold_address.as_str()));
        assert_eq!(public.heir_address, p2pk_address(Prefix::Testnet, &heir).to_string());
        public.check_complete().unwrap();
        assert_eq!(on.check_self(), Ok(()));
        assert_eq!(on.public_sha256, Some(public.sha256()));
        // without --heir-x a heir key is made online
        assert!(Keys::fresh_online(&Network::testnet(), &export, None, false, false).unwrap().heir.is_some());
        // the block carried back is checked against the offline secrets
        let carry = PublicExport { kind: PUBLIC_EXPORT_KIND.into(), public: public.clone(), public_sha256: public.sha256() };
        let mut off2 = off.clone();
        off2.adopt_public(&carry).unwrap();
        assert_eq!(off2.check_self(), Ok(()));
        assert_eq!(off2.public_sha256, Some(public.sha256()));
        let mut other = Keys::fresh_offline();
        let err = other.adopt_public(&carry).unwrap_err();
        assert!(err.contains("do not match") && other.public.is_none(), "{err}");
        let mut off3 = off.clone();
        let bad = PublicExport { kind: PUBLIC_EXPORT_KIND.into(), public: public.clone(), public_sha256: "00".into() };
        assert!(off3.adopt_public(&bad).unwrap_err().contains("code written in it"));
        // bad inputs are refused in words
        assert!(Keys::fresh_online(&Network::testnet(), &export, Some("zz"), false, false).unwrap_err().contains("heir"));
        let mut wrong = off.offline_export(&Network::testnet()).unwrap();
        wrong.cold_address = "kaspatest:q".into();
        assert!(Keys::fresh_online(&Network::testnet(), &wrong, None, false, false).unwrap_err().contains("cold address"));
        assert!(Keys { cancel: off.cancel, ..Default::default() }.offline_export(&Network::testnet()).unwrap_err().contains("cold"));
    }

    #[test]
    fn secrets_file_is_owner_only_and_a_bad_one_is_not_echoed() {
        let dir = std::env::temp_dir().join(format!("kv-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        std::fs::write(&path, "old").unwrap();
        write_private(&path, b"{\"spend\": [300]}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"spend\": [300]}", "an existing file is replaced whole");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let err = Keys::try_load_with(&path, None).unwrap_err();
        assert!(err.contains("Restore the original file"), "{err}");
        assert!(!err.contains("300"), "the bad field is not echoed: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// D4: the drawer leaves stop key 1 empty; the phone makes the card, stop key 1 and the top-up key,
    /// hands the three secrets out as one 96-byte block and keeps a file with no secret in it whose code
    /// the drawer can check and adopt.
    #[test]
    fn phone_keys_are_born_public_only_with_the_three_secrets_handed_out() {
        let net = Network::testnet();
        let mut drawer = Keys::fresh_offline_for_phone();
        assert!(drawer.cancel.is_none() && drawer.cancel2.is_some() && drawer.cold3.is_some());
        let export = drawer.offline_export(&net).unwrap();
        assert_eq!(export.cancel_x, "", "stop key 1 is left to the phone");
        let heir_x = faster_hex::hex_string(&xonly(&fresh()));
        let (keys, secrets) = Keys::fresh_phone(&net, &export, &heir_x, true, false).unwrap();
        assert!(!keys.has_secret(), "{keys:?}");
        let p = keys.public();
        assert_eq!(keys.public_sha256.as_deref(), Some(p.sha256().as_str()));
        let x_of = |b: &[u8]| faster_hex::hex_string(&xonly(&keypair(b.try_into().unwrap())));
        assert_eq!(p.spend_x, x_of(&secrets[..32]));
        assert_eq!(p.cancel_x, x_of(&secrets[32..64]));
        assert_eq!(keys.funding_x.as_deref(), Some(x_of(&secrets[64..]).as_str()));
        assert_eq!((p.cancel2_x.as_str(), p.cold_address.as_str(), p.heir_x.as_str(), p.auto_heir), (export.cancel2_x.as_str(), export.cold_address.as_str(), heir_x.as_str(), true));
        p.check_complete().unwrap();
        // The drawer adopts the block: its absent stop secret 1 passes against the phone-born cancel_x, slots 2 and 3 still sign.
        drawer.adopt_public(&PublicExport { kind: PUBLIC_EXPORT_KIND.into(), public: p.clone(), public_sha256: p.sha256() }).unwrap();
        assert!(drawer.try_stop_key(0).unwrap_err().contains("stop key 1"));
        assert!(drawer.try_stop_key(1).is_ok());
        // The phone's file round-trips with its code and the funding half.
        let dir = std::env::temp_dir().join(format!("kv-keys-phone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        keys.save(&path, true).unwrap();
        let back = Keys::load_public(&path).unwrap();
        assert_eq!((&back.public_sha256, &back.funding_x), (&keys.public_sha256, &keys.funding_x));
        assert!(!back.has_secret());
        std::fs::remove_dir_all(&dir).ok();
        // A drawer export that already holds stop key 1 is refused, as is a missing heir.
        let full = Keys::fresh_offline().offline_export(&net).unwrap();
        assert!(Keys::fresh_phone(&net, &full, &heir_x, false, false).unwrap_err().contains("--for-phone"));
        assert!(Keys::fresh_phone(&net, &export, "", false, false).unwrap_err().contains("heir"));
    }

    #[test]
    fn wipe_leaves_no_secret_and_keeps_the_public_block() {
        let mut keys = Keys::fresh(&Network::testnet());
        keys.funding_x = Some("ab".into());
        let code = keys.public().sha256();
        keys.wipe();
        assert!(!keys.has_secret());
        for s in [&keys.spend, &keys.cancel, &keys.cancel2, &keys.cancel3, &keys.heir, &keys.cold, &keys.cold2, &keys.cold3, &keys.funding] {
            assert!(s.is_none());
        }
        assert_eq!(keys.public().sha256(), code);
        assert_eq!(keys.funding_x.as_deref(), Some("ab"));
    }

    /// Sheets: 24 BIP-39 words carry the 32 bytes and an 8-bit check and read back to the same secret.
    #[test]
    fn sheet_words_round_trip_and_refuse_a_wrong_word() {
        let zero = sheet_words(&[0u8; 32]);
        assert!(zero[..23].iter().all(|w| w == "abandon") && zero[23] == "art", "BIP-39 vector for 32 zero bytes: {zero:?}");
        assert_eq!(sheet_from_words(&zero).unwrap(), [0u8; 32]);
        let secret = fresh().secret_bytes();
        let words = sheet_words(&secret);
        assert_eq!(sheet_from_words(&words).unwrap(), secret);
        let loud: Vec<String> = words.iter().map(|w| format!(" {} ", w.to_ascii_uppercase())).collect();
        assert_eq!(sheet_from_words(&loud).unwrap(), secret, "case and spaces do not matter");
        let mut wrong = words.clone();
        wrong[5] = if wrong[5] == "zoo" { "abandon".into() } else { "zoo".into() };
        assert!(sheet_from_words(&wrong).unwrap_err().contains("check"));
        let mut bad = words.clone();
        bad[0] = "kaspa".into();
        let err = sheet_from_words(&bad).unwrap_err();
        assert!(err.contains("Word 1") && !err.contains("kaspa"), "{err}");
        assert!(sheet_from_words(&words[..23]).unwrap_err().contains("24 words"));
    }

    /// Everything on this phone: no drawer file; the safe's three keys leave only as words that read back to
    /// the cold keys of the rule block; nothing secret stays in the keys; no heir means the no-heir slot.
    #[test]
    fn phone_makes_everything_itself_and_hands_the_safe_out_as_words() {
        let net = Network::testnet();
        let heir_x = faster_hex::hex_string(&xonly(&fresh()));
        let (keys, secrets, sheets) = Keys::fresh_phone_all(&net, &heir_x, true, true).unwrap();
        assert!(!keys.has_secret(), "{keys:?}");
        let p = keys.public();
        assert_eq!(keys.public_sha256.as_deref(), Some(p.sha256().as_str()));
        let x_of = |b: &[u8]| faster_hex::hex_string(&xonly(&keypair(b.try_into().unwrap())));
        assert_eq!(p.spend_x, x_of(&secrets[..32]));
        assert_eq!(p.cancel_x, x_of(&secrets[32..64]));
        assert_eq!(keys.funding_x.as_deref(), Some(x_of(&secrets[64..96]).as_str()));
        for i in 0..3 {
            assert_eq!(p.cold_x[i], x_of(&secrets[96 + i * 32..128 + i * 32]), "cold secret {} rides in the block", i + 1);
        }
        assert_eq!((p.heir_x.as_str(), p.auto_heir, p.hard_limit), (heir_x.as_str(), true, true));
        assert!(!p.cancel2_x.is_empty() && !p.cancel3_x.is_empty() && p.cold_x.len() == 3);
        p.check_complete().unwrap();
        // the words are the cold keys, in redeem-script order, and make the safe address
        let colds: Vec<[u8; 32]> = sheets.iter().map(|s| sheet_from_words(s).unwrap()).collect();
        for (i, c) in colds.iter().enumerate() {
            assert_eq!(p.cold_x[i], x_of(c), "sheet {}", i + 1);
        }
        let xs: Vec<Vec<u8>> = colds.iter().map(|c| xonly(&keypair(c))).collect();
        assert_eq!(p.cold_address, p2sh_address(net.prefix, &cold_redeem_script(&xs)).to_string());
        assert_eq!(p.cold_redeem().unwrap(), cold_redeem_script(&xs));
        // keys rebuilt from the sheets are the safe keys of this block
        let rebuilt = Keys { cold: Some(colds[0]), cold2: Some(colds[1]), cold3: Some(colds[2]), public: keys.public.clone(), public_sha256: keys.public_sha256.clone(), ..Default::default() };
        rebuilt.check_self().unwrap();
        assert_eq!(rebuilt.cold_keys().len(), 3);
        // the file holds no secret and round-trips
        let dir = std::env::temp_dir().join(format!("kv-keys-phone-all-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        keys.save(&path, true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // No string VALUE in the file is a sheet word (field names such as "network" or "public" are words too).
        fn strings<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a str>) {
            match v {
                serde_json::Value::String(s) => out.push(s),
                serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
                serde_json::Value::Object(o) => o.values().for_each(|x| strings(x, out)),
                _ => {}
            }
        }
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut values = Vec::new();
        strings(&file, &mut values);
        for w in sheets.iter().flatten() {
            assert!(!values.iter().any(|v| v == w || v.split_whitespace().any(|t| t == w)), "a sheet word in keys.json");
        }
        let back = Keys::load_public(&path).unwrap();
        assert!(!back.has_secret() && back.public_sha256 == keys.public_sha256);
        std::fs::remove_dir_all(&dir).ok();
        // no heir: both heir fields empty; the slot falls back to stop key 1 and the safe behind the far-out clock
        let (keys, _secrets, _sheets) = Keys::fresh_phone_all(&net, "  ", false, false).unwrap();
        let p = keys.public();
        assert!(p.heir_x.is_empty() && p.heir_address.is_empty());
        assert!(p.check_complete().unwrap().iter().any(|r| r.starts_with("heir: none")), "{:?}", p.rules());
        assert_eq!(p.inherit_delay(&Dials::testnet()), INHERIT_DELAY_NONE);
        assert_eq!(p.heir_slot(vec![7], &Dials::testnet()), (unhex(&p.cancel_x), vec![7], INHERIT_DELAY_NONE));
        assert!(!keys.has_secret());
        // a bad heir key is refused
        assert!(Keys::fresh_phone_all(&net, "zz", false, false).unwrap_err().contains("heir"));
    }
}
