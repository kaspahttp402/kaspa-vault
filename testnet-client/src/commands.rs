//! The vault commands, each a short function over a `Ctx`. Every one returns `Err(text)` instead
//! of panicking so the phone service (serve.rs) can show the reason.

use crate::contract::{self, Coin, DAA_DOMAIN_LIMIT, Dials, HARD_LIMIT, LOW_REMAINDER, OVER_CAP, State, Vault, after_decline, after_finish, kas, locked, sompi, spk_bytes, spk_hash};
use crate::network::Network;
use crate::keys::{Keys, OFFLINE_EXPORT_KIND, OfflineExport, PUBLIC_EXPORT_KIND, PublicExport, keypair, p2pk_address, p2pk_address_x, stop_slot, xonly};
use crate::tx::{CHANGE_FLOOR, Ctx, FeeError};

/// Closed account records live here, one file per closing (`Ctx::close_record`).
pub const CLOSED_DIR: &str = "closed";
pub const ACCOUNT_OPEN: &str = "This account is already open.";
use kaspa_addresses::Address;
use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
use kaspa_consensus_core::tx::{Transaction, TransactionInput, TransactionOutpoint, TransactionOutput, UtxoEntry};
use kaspa_txscript::pay_to_address_script;
use secp256k1::Keypair;
use silverscript_abi::ArtifactValue as V;
use std::path::{Path, PathBuf};

/// Fee reserve staged with a top-up; the contract allows one output, so what the fee does not use grows the vault.
const TOPUP_RESERVE: u64 = 2_000_000;

/// fund and topup: a vault below VAULT_FLOOR (minRemainder + maxFee) could never cancel, inherit or
/// request on its own (vault.sil L6), so money must not be born stuck. `have` is what the vault holds now.
fn floor_check(d: &Dials, have: u64, adding: u64) -> Result<(), String> {
    if have + adding < d.floor() {
        let held = if have > 0 { format!(", on top of the {:.5} KAS it holds", kas(have)) } else { String::new() };
        return Err(format!("a vault needs at least {} KAS so it can always be stopped or passed on ({:.5} KAS asked{held}).", kas(d.floor()), kas(adding)));
    }
    Ok(())
}

/// A fee-loop refusal in this command's bank words; the builder's own refusals pass through unchanged.
fn fee_words(e: FeeError, over: impl Fn(u64, u64) -> String) -> String {
    match e {
        FeeError::OverCeiling { need, ceiling } => over(need, ceiling),
        e => e.to_string(),
    }
}

/// The vault coin's entry, plus the lift coin's when there is one, in input order.
fn lift_entries(coin: &Coin, lift: Option<&(Coin, Keypair)>) -> Vec<UtxoEntry> {
    let mut entries = vec![coin.1.clone()];
    if let Some((c, _)) = lift {
        entries.push(c.1.clone());
    }
    entries
}

/// Flags of `keys`.
pub struct KeysOpts {
    pub auto_heir: bool,
    /// v6 hard-limit dial.
    pub hard_limit: bool,
    pub plain: bool,
    pub offline: bool,
    /// With `offline`, the drawer device of a phone wallet: stop key 1 is left to be born on the phone (D4).
    pub for_phone: bool,
    pub import_public: Option<PathBuf>,
    pub heir_x: Option<String>,
}
/// `keys --offline` writes this beside keys.json; carry it to the online device.
pub const OFFLINE_EXPORT_FILE: &str = "offline-public.json";
/// `keys --import-public offline-public.json` writes this beside keys.json; carry it back to the offline device.
pub const PUBLIC_EXPORT_FILE: &str = "vault-public.json";

/// `keys`: a new vault's keys.json in `dir`, locked with a passphrase unless --plain. Three flows:
/// everything on this machine (testnet proofs), `--offline` on the offline device, and
/// `--import-public <file>` on either side to join the two halves into one rule block (item 17).
pub fn do_keys(dir: &Path, o: KeysOpts, net: &Network) -> Result<(), String> {
    let keys_path = dir.join("keys.json");
    match (o.offline, &o.import_public) {
        (true, Some(_)) => Err(format!("--offline and --import-public do not go together: run `keys --offline` first, then `keys --import-public {PUBLIC_EXPORT_FILE}` once the online device has written it.")),
        (false, _) if o.for_phone => Err("--for-phone goes with --offline: it is the drawer device's flag.".into()),
        (true, None) => keys_offline(dir, &keys_path, o.plain, o.for_phone, net),
        (false, Some(file)) => keys_import(dir, &keys_path, &dir.join(file), &o, net),
        (false, None) => keys_all_here(&keys_path, &o, net),
    }
}

pub fn refuse_existing(keys_path: &Path) -> Result<(), String> {
    if keys_path.exists() {
        return Err(format!("{} already exists; refusing to overwrite. Move it away first if that vault is finished.", keys_path.display()));
    }
    Ok(())
}

fn print_block(keys: &Keys) {
    let p = keys.public();
    println!("network:         {}", p.network().name);
    println!("cold address:    {} (2-of-3 safe: cold sheets 1, 2, 3)", p.cold_address);
    println!("rule block code: {} (the code of the public block; fund and the offline device must show the same)", p.sha256());
    println!("heir address:    {} (may take over after the silence set by the heir delay dial at fund)", p.heir_address);
    for r in p.rules() {
        println!("  {r}");
    }
}

fn lock_note(plain: bool) -> &'static str {
    if plain { "plain text, tests only" } else { "locked with your passphrase" }
}

/// The old flow: every secret born on this machine (testnet proofs).
fn keys_all_here(keys_path: &Path, o: &KeysOpts, net: &Network) -> Result<(), String> {
    refuse_existing(keys_path)?;
    let mut keys = Keys::fresh(net);
    let mut public = keys.public();
    if let Some(x) = &o.heir_x {
        public.heir_address = p2pk_address_x(net.prefix, x)?.to_string();
        public.heir_x = x.to_ascii_lowercase();
        keys.heir = None;
    }
    public.auto_heir = o.auto_heir;
    public.hard_limit = o.hard_limit;
    keys.public_sha256 = Some(public.sha256());
    keys.public = Some(public);
    keys.save(keys_path, o.plain)?;
    println!("wrote {} ({})", keys_path.display(), lock_note(o.plain));
    println!("funding address: {}", p2pk_address(net.prefix, &keys.funding_key()));
    print_block(&keys);
    println!("note: every secret of this vault was made on this machine. For real money make the stop keys and cold sheets with `keys --offline` on the offline device, then `keys --import-public {OFFLINE_EXPORT_FILE}` here.");
    Ok(())
}

/// Offline device: the stop keys and cold sheets are born here; only their public side leaves, in a file.
fn keys_offline(dir: &Path, keys_path: &Path, plain: bool, for_phone: bool, net: &Network) -> Result<(), String> {
    refuse_existing(keys_path)?;
    let keys = if for_phone { Keys::fresh_offline_for_phone() } else { Keys::fresh_offline() };
    let export = keys.offline_export(net)?;
    keys.save(keys_path, plain)?;
    let out = dir.join(OFFLINE_EXPORT_FILE);
    std::fs::write(&out, serde_json::to_string_pretty(&export).unwrap()).map_err(|e| format!("{}: {e}", out.display()))?;
    let stops = if for_phone { "stop keys 2, 3 (stop key 1 is born on the phone)" } else { "stop keys 1, 2, 3" };
    println!("wrote {} ({}): {stops} and cold sheets 1, 2, 3; no other secret is here", keys_path.display(), lock_note(plain));
    println!("wrote {} (public keys only, safe to carry)", out.display());
    println!("cold address:    {} (2-of-3 safe: cold sheets 1, 2, 3)", export.cold_address);
    println!("Next: carry {OFFLINE_EXPORT_FILE} to the online device and run `keys --import-public {OFFLINE_EXPORT_FILE}` there; then bring its {PUBLIC_EXPORT_FILE} back here and run `keys --import-public {PUBLIC_EXPORT_FILE}`.");
    println!("Write the safe sheets on paper from this device; they never leave it.");
    Ok(())
}

/// Writes `vault-public.json` (the rule block with its code) beside keys.json, for the drawer device to adopt.
pub fn write_public_export(dir: &Path, keys: &Keys) -> Result<PathBuf, String> {
    let public = keys.public();
    let carry = PublicExport { kind: PUBLIC_EXPORT_KIND.into(), public_sha256: public.sha256(), public };
    let out = dir.join(PUBLIC_EXPORT_FILE);
    std::fs::write(&out, serde_json::to_string_pretty(&carry).unwrap()).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(out)
}

/// Either side of the import: the offline export makes the online keys.json; the vault block completes the offline one.
pub fn keys_import(dir: &Path, keys_path: &Path, file: &Path, o: &KeysOpts, net: &Network) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{} is not what `keys` writes: {e}", file.display()))?;
    match v.get("kind").and_then(|k| k.as_str()) {
        Some(OFFLINE_EXPORT_KIND) => {
            refuse_existing(keys_path)?;
            let export: OfflineExport = serde_json::from_value(v).map_err(|e| format!("{}: {e}", file.display()))?;
            let keys = Keys::fresh_online(net, &export, o.heir_x.as_deref(), o.auto_heir, o.hard_limit)?;
            keys.save(keys_path, o.plain)?;
            let out = write_public_export(dir, &keys)?;
            let heir_note = if keys.heir.is_some() { " and a heir key (testnet only; give --heir-x for a real heir)" } else { "" };
            println!("wrote {} ({}): spend and funding keys{heir_note}", keys_path.display(), lock_note(o.plain));
            println!("wrote {} (the rule block with its code, safe to carry)", out.display());
            println!("funding address: {}", p2pk_address(net.prefix, &keys.funding_key()));
            print_block(&keys);
            println!("Next: carry {PUBLIC_EXPORT_FILE} back to the offline device and run `keys --import-public {PUBLIC_EXPORT_FILE}` there.");
            Ok(())
        }
        Some(PUBLIC_EXPORT_KIND) => {
            let carry: PublicExport = serde_json::from_value(v).map_err(|e| format!("{}: {e}", file.display()))?;
            net.check_recorded(&carry.public.network, &file.display().to_string())?;
            let plain = !Keys::is_locked(keys_path);
            let mut keys = Keys::load(keys_path)?;
            if keys.public.is_some() {
                return Err(format!("{} already has a rule block; nothing to import. Start a new vault in a new folder instead.", keys_path.display()));
            }
            keys.adopt_public(&carry)?;
            keys.save(keys_path, plain)?;
            println!("{} now holds the rule block ({})", keys_path.display(), lock_note(plain));
            print_block(&keys);
            Ok(())
        }
        _ => Err(format!("{} is neither {OFFLINE_EXPORT_FILE} nor {PUBLIC_EXPORT_FILE} (no `kind`); copy the file `keys` wrote.", file.display())),
    }
}

impl Ctx {
    /// Writes state.json with the rule block code of this device's keys.json beside the vault's
    /// fields (contract::PUBLIC_ANCHOR). A recorded code that no longer matches refuses; a vault
    /// file from before the code gets it now, said once.
    pub fn save_anchored(&self, v: &Vault) -> Result<(), String> {
        let now = self.keys.public().sha256();
        let had_file = self.state_path().exists();
        match contract::recorded_public_sha256(&self.state_path())? {
            Some(h) if h != now => return Err(contract::RULE_BLOCK_CHANGED.into()),
            Some(_) => {}
            None if had_file => println!("Recorded the rule block code of keys.json for this vault: {now} (the vault file had none)."),
            None => {}
        }
        contract::save_vault_anchored(&self.state_path(), v, &now)
    }

    /// The top-up address: from the funding secret, or from its public half (`funding_x`) on a device
    /// that keeps the secret elsewhere (the phone's signer store).
    pub fn funding_addr(&self) -> Result<Address, String> {
        match (&self.keys.funding, &self.keys.funding_x) {
            (Some(s), _) => Ok(self.network.p2pk(&xonly(&keypair(s)))),
            (None, Some(x)) => p2pk_address_x(self.network.prefix, x),
            (None, None) => Err("the funding key is not on this device".into()),
        }
    }
    fn spend_key(&self) -> Result<secp256k1::Keypair, String> {
        self.keys.spend.as_ref().map(keypair).ok_or("the spend key is not on this device".into())
    }
    fn open_vault(&self) -> Result<Vault, String> {
        self.try_load_vault()?.ok_or("no vault; run fund".into())
    }

    /// `status`: DAA, the funding coins, and the vault coin(s) with their lineage.
    pub async fn do_status(&self) -> Result<(), String> {
        println!("network {} ({})", self.network.name, self.network.id);
        println!("DAA score {}", self.try_daa().await?);
        let funding_addr = self.funding_addr()?;
        let coins = self.try_utxos(&funding_addr).await?;
        let total: u64 = coins.iter().map(|(_, u)| u.amount).sum();
        println!("funding {funding_addr}: {} coins, {:.4} KAS", coins.len(), kas(total));
        match self.try_load_vault()? {
            None => println!("no vault yet (run fund)"),
            Some(v) => {
                let addr = self.try_vault_address(&v.state)?;
                println!("vault {addr}\n  covenant id {}\n  state {:?}", v.covenant_id, v.state);
                if contract::recorded_public_sha256(&self.state_path())?.is_none() {
                    self.save_anchored(&v)?;
                }
                println!("  rule block code {}", self.keys.public().sha256());
                for r in self.keys.public().rules() {
                    println!("  {r}");
                }
                for d in v.dials.describe(self.network.daa_per_sec, !self.keys.public().heir_x.is_empty()) {
                    println!("  dial: {d}");
                }
                if let Some(h) = &v.contract_sha256 {
                    println!("  rules file sha256 {h} ({})", if h == contract::embedded_sha256() { "built into this program" } else { "an earlier reviewed version" });
                }
                for (op, u) in self.try_utxos(&addr).await? {
                    let lineage = if u.covenant_id == Some(v.covenant_id) { "ours" } else { "FOREIGN, not part of this vault" };
                    println!("  coin {}:{} {:.4} KAS daa {} [{lineage}]", op.transaction_id, op.index, kas(u.amount), u.block_daa_score);
                }
                for h in &v.history {
                    println!("  history: {h}");
                }
            }
        }
        Ok(())
    }

    /// `fund`: pin the contract source into this folder, then open a vault (genesis covenant) from the funding coins.
    pub async fn do_fund(&self, amount_kas: f64) -> Result<String, String> {
        // A rule change keeps the same account: the retired record's lines carry over, then "Rules updated".
        let carried = match self.try_load_vault()? {
            Some(v) => {
                self.close_record(&v).await?;
                v.history
            }
            None => Vec::new(),
        };
        let amount = sompi(amount_kas);
        let has_heir = !self.keys.public().heir_x.is_empty();
        self.dials.check(has_heir, self.network.is_mainnet())?;
        floor_check(&self.dials, 0, amount)?;
        // Item 18: an older or incomplete rule block never locks money; the rules are said out loud first.
        let rules = self.keys.public().check_complete()?;
        println!("Rules this vault will lock money under:");
        for r in &rules {
            println!("  {r}");
        }
        println!("Dials this vault will run with (fixed now; they are part of its address):");
        for d in self.dials.describe(self.network.daa_per_sec, has_heir) {
            println!("  {d}");
        }
        if self.network.is_mainnet() && (self.dials.delay as f64) < 3_600.0 * self.network.daa_per_sec {
            println!("Note: real money with a notice period under an hour (proof dials). For a vault you will keep, fund with --dials mainnet.");
        }
        let funding = keypair(self.keys.funding.as_ref().ok_or("the funding key is not on this device")?);
        let funding_addr = self.funding_addr()?;
        let coins = self.try_utxos(&funding_addr).await?;
        let total: u64 = coins.iter().map(|(_, u)| u.amount).sum();
        let entries: Vec<UtxoEntry> = coins.iter().map(|(_, u)| u.clone()).collect();
        let contract_sha256 = contract::pin_source(&self.dir)?;
        println!("rules file sha256 {contract_sha256} (built into this program)");
        let state = locked();
        let vault_spk = self.try_vault_spk(&state)?;
        let (tx, _) = self.try_with_fee(&entries, None, &|fee| {
            if total < amount + fee {
                return Err(format!("funding has {total} sompi, need {}", amount + fee));
            }
            // Change too small to keep as a coin goes into the deposit instead of exploding the mass.
            let change = total - amount - fee;
            let (deposit, change) = if change < CHANGE_FLOOR { (amount + change, 0) } else { (amount, change) };
            let mut outputs = vec![TransactionOutput::new(deposit, vault_spk.clone())];
            if change > 0 {
                outputs.push(TransactionOutput::new(change, pay_to_address_script(&funding_addr)));
            }
            self.plain_tx(&coins, outputs, &funding, true)
        })?;
        let covenant_id = tx.outputs[0].covenant.as_ref().ok_or("genesis covenant missing")?.covenant_id;
        let folded = tx.outputs[0].value - amount;
        if folded > 0 {
            println!("Change of {:.5} KAS too small to keep was added to the deposit.", kas(folded));
        }
        let id = self.try_submit(&tx, "fund").await?;
        let note = if folded > 0 { format!(" (+{:.5} KAS of small change)", kas(folded)) } else { String::new() };
        let v = Vault { covenant_id, state, pending_to: None, history: carry_history(carried, format!("fund {amount_kas} KAS{note} tx {id}")), contract_sha256: Some(contract_sha256), network: self.network.name.into(), dials: self.dials.clone() };
        self.save_anchored(&v)?;
        println!("vault opened at {} with covenant id {covenant_id}; rules pinned in {}", self.try_vault_address(&v.state)?, self.dir.join(contract::SOURCE_FILE).display());
        println!("rule block code {} recorded in state.json; the offline device's keys.json must show the same", self.keys.public().sha256());
        Ok(id)
    }

    /// `fund` on an account whose record says stopped: once the chain shows no coin of it left at its
    /// address, the record moves into `<dir>/closed/<unix seconds>-<file>` (its history stays on disk) and the
    /// account opens fresh under the new dials. An account still open refuses. A record this program cannot
    /// compile (rules from a version it does not carry) is closed without the chain check: this program could
    /// not move that coin anyway, and the record is kept for the version that can.
    pub async fn close_record(&self, v: &Vault) -> Result<(), String> {
        if !contract::is_stopped(v) {
            return Err(ACCOUNT_OPEN.into());
        }
        if let Ok(address) = self.try_vault_address(&v.state) {
            let held: u64 = self.try_utxos(&address).await?.into_iter().filter(|(_, u)| u.covenant_id == Some(v.covenant_id)).map(|(_, u)| u.amount).sum();
            if held > 0 {
                return Err(format!("The account still holds {:.8} KAS at its old address. Wait for the stop to land.", kas(held)));
            }
        }
        let closed = self.dir.join(CLOSED_DIR);
        std::fs::create_dir_all(&closed).map_err(|e| format!("{}: {e}", closed.display()))?;
        let from = self.state_path();
        let file = from.file_name().and_then(|s| s.to_str()).unwrap_or("state.json").to_string();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        std::fs::rename(&from, closed.join(format!("{secs}-{file}"))).map_err(|e| format!("{file}: {e}"))
    }

    /// `pay`: plain payment from the funding key to any address (used to fund a cold safe).
    pub async fn do_pay(&self, amount_kas: f64, to: &str) -> Result<String, String> {
        self.do_pay_sompi(Some(sompi(amount_kas)), to).await.map(|(id, _)| id)
    }

    /// The payment behind `pay` and the phone's `purse.pay`: `Some(amount)` with the change back to the purse,
    /// `None` everything the purse holds less the fee in one output. Returns (tx id, what reached `to` in sompi).
    pub async fn do_pay_sompi(&self, amount: Option<u64>, to: &str) -> Result<(String, u64), String> {
        let funding = keypair(self.keys.funding.as_ref().ok_or("the funding key is not on this device")?);
        let funding_addr = self.funding_addr()?;
        let pay_spk = self.network.try_to_spk(to)?;
        if amount == Some(0) {
            return Err("pay needs an amount above zero".into());
        }
        let coins = self.try_utxos(&funding_addr).await?;
        if coins.is_empty() {
            return Err("the purse holds nothing".into());
        }
        let total: u64 = coins.iter().map(|(_, u)| u.amount).sum();
        let entries: Vec<UtxoEntry> = coins.iter().map(|(_, u)| u.clone()).collect();
        let (tx, _) = self.try_with_fee(&entries, None, &|fee| {
            let paying = match amount {
                Some(a) => a,
                None => total.checked_sub(fee).filter(|p| *p > 0).ok_or_else(|| format!("the purse holds {total} sompi, less than the {fee} sompi fee"))?,
            };
            if total < paying + fee {
                return Err(format!("funding has {total} sompi, need {}", paying + fee));
            }
            let mut outputs = vec![TransactionOutput::new(paying, pay_spk.clone())];
            let change = total - paying - fee;
            if change > 0 {
                outputs.push(TransactionOutput::new(change, pay_to_address_script(&funding_addr)));
            }
            self.plain_tx(&coins, outputs, &funding, false)
        })?;
        let paid = tx.outputs[0].value;
        let id = self.try_submit(&tx, "pay").await?;
        println!("paid {:.8} KAS from {funding_addr} to {to}, tx {id}", kas(paid));
        Ok((id, paid))
    }

    /// Pays from the vault under the per-window cap; returns the tx id. `self.force` sends
    /// an over-cap spend anyway so the node's own rejection can be seen.
    pub async fn do_spend(&self, amount_kas: f64, to: &str) -> Result<String, String> {
        let force = self.force;
        let mut v = self.open_vault()?;
        if v.state.stage != 0 {
            return Err("vault is Pending; finish or cancel first".into());
        }
        let amount = sompi(amount_kas);
        let to_script = self.network.try_to_spk(to)?;
        let coin = self.try_vault_coin(&v).await?;
        let lock_time = self.try_daa().await?.saturating_sub(20);
        if lock_time >= DAA_DOMAIN_LIMIT {
            return Err(format!("DAA score {lock_time} is past the DAA domain"));
        }
        let in_daa = coin.1.block_daa_score as i64;
        let next_for = |fee: u64| {
            let outflow = (amount + fee) as i64;
            let (ws, spent) = if lock_time as i64 >= v.state.window_start + v.dials.window && lock_time as i64 >= in_daa + v.dials.window {
                (lock_time as i64, outflow)
            } else {
                (v.state.window_start, v.state.spent + outflow)
            };
            State { window_start: ws, spent, ..v.state.clone() }
        };
        let spend_key = self.spend_key()?;
        // No contract bound on a spend's fee, so the client draws one: the vault's allowance or a tenth
        // of the payment, whichever is larger (a tiny payee output has a huge storage mass).
        let ceiling = (v.dials.max_fee as u64).max(amount / 10);
        let (tx, fee) = self.try_with_fee(std::slice::from_ref(&coin.1), Some(ceiling), &|fee| {
            let next = next_for(fee);
            if next.spent > v.dials.cap {
                if !force {
                    return Err(format!("{OVER_CAP}: {} > {} sompi in the current window (use --force to let the node say so)", next.spent, v.dials.cap));
                }
                println!("over the cap ({} > {} sompi); sending anyway because of --force", next.spent, v.dials.cap);
            }
            let left = coin.1.amount.checked_sub(amount + fee).ok_or("the vault does not hold that much")?;
            if left < v.dials.min_remainder as u64 && !force {
                return Err(format!("{LOW_REMAINDER}: {left} < {} sompi", v.dials.min_remainder));
            }
            let outputs = vec![TransactionOutput::new(amount, to_script.clone()), self.continuation(left, &next, &v)];
            self.vault_tx(&v, &coin, "spend", Some(&spend_key), &[], outputs, 0, lock_time)
        })
        .map_err(|e| {
            fee_words(e, |need, _| {
                format!("the network fee for this payment would be {:.5} KAS, more than a tenth of the payment. Pay a larger amount or add to the vault first.", kas(need))
            })
        })?;
        let id = self.try_submit(&tx, "spend").await?;
        v.state = next_for(fee);
        v.history.push(format!("spend {amount_kas} KAS to {to}, fee {:.5} KAS, tx {id}", kas(fee)));
        self.save_anchored(&v)?;
        Ok(id)
    }

    /// First half of a withdrawal with notice; returns the tx id.
    pub async fn do_request(&self, amount_kas: f64, to: &str) -> Result<String, String> {
        let mut v = self.open_vault()?;
        // v6 hard-limit dial: the contract's request entry fails outright; say so first.
        if self.keys.public().hard_limit {
            return Err(HARD_LIMIT.into());
        }
        if v.state.stage != 0 {
            return Err("vault is already Pending".into());
        }
        let amount = sompi(amount_kas);
        // Contract v3 (L3): a request below minRemainder could never finish into a minable
        // output, so the contract refuses it; say so here instead of a bare engine VerifyError.
        if (amount as i64) < v.dials.min_remainder {
            return Err(format!("a request must be at least {} KAS on this vault ({amount_kas} KAS asked)", kas(v.dials.min_remainder as u64)));
        }
        let to_script = self.network.try_to_spk(to)?;
        let coin = self.try_vault_coin(&v).await?;
        let dest_hash = spk_hash(&to_script);
        let next = State { stage: 1, dest_hash: dest_hash.clone(), amount: amount as i64, ..v.state.clone() };
        let args = vec![V::Bytes(spk_bytes(&to_script)), V::Int(amount as i64)];
        let spend_key = self.spend_key()?;
        let (tx, fee) = self.try_with_fee(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
            let left = coin.1.amount.checked_sub(fee).ok_or("the vault cannot pay the fee")?;
            // What finish will meet: after the amount leaves, the rest stays as a coin of at least
            // minRemainder (finish's own fee is at most maxFee) or burns as fee when it is at most maxFee.
            let after = left.checked_sub(amount).ok_or("the vault does not hold that much")?;
            if after > v.dials.max_fee as u64 && after < v.dials.floor() {
                return Err(format!(
                    "after this withdrawal the vault would keep {:.5} KAS, too little to finish or to keep (it must keep at least {} KAS, or nothing). Withdraw a different amount.",
                    kas(after),
                    kas(v.dials.floor())
                ));
            }
            let outputs = vec![self.continuation(left, &next, &v)];
            self.vault_tx(&v, &coin, "request", Some(&spend_key), &args, outputs, 0, 0)
        })
        .map_err(|e| {
            fee_words(e, |need, ceiling| {
                format!("starting this withdrawal would cost {:.5} KAS in network fees, more than this vault allows ({} KAS). Wait for a quieter network.", kas(need), kas(ceiling))
            })
        })?;
        let id = self.try_submit(&tx, "request").await?;
        v.state = next;
        v.pending_to = Some(to.to_string());
        v.history.push(format!("request {amount_kas} KAS to {to}, fee {:.5} KAS, tx {id}; finish allowed after {} DAA", kas(fee), v.dials.delay));
        self.save_anchored(&v)?;
        Ok(id)
    }

    /// Completes a pending withdrawal; returns the tx id and whether that emptied the vault.
    pub async fn do_finish(&self) -> Result<(String, bool), String> {
        let mut v = self.open_vault()?;
        if v.state.stage != 1 {
            return Err("nothing pending".into());
        }
        let coin = self.try_vault_coin(&v).await?;
        println!("coin age {} DAA, delay {}", self.try_daa().await?.saturating_sub(coin.1.block_daa_score), v.dials.delay);
        let dest = v.pending_to.clone().ok_or("pending destination")?;
        let dest_script = self.network.try_to_spk(&dest)?;
        if spk_hash(&dest_script) != v.state.dest_hash {
            return Err("stored destination does not match recorded hash".into());
        }
        let amount = v.state.amount as u64;
        let remainder = coin.1.amount.checked_sub(amount).ok_or("the vault coin is smaller than the pending amount")?;
        let next = after_finish(&v.state);
        // The contract's rule: the rest stays as a coin of at least minRemainder, or burns as the whole
        // fee when it is at most maxFee; nothing in between, and never a burn above maxFee.
        let burn = remainder <= v.dials.max_fee as u64;
        let (tx, fee) = self.try_with_fee(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
            let mut outputs = vec![TransactionOutput::new(amount, dest_script.clone())];
            if burn {
                if remainder < fee {
                    return Err(format!(
                        "finishing now would leave {:.5} KAS for a network fee of {:.5} KAS. Cancel to move everything to your safe.",
                        kas(remainder),
                        kas(fee)
                    ));
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
                outputs.push(self.continuation(left, &next, &v));
            }
            self.vault_tx(&v, &coin, "finish", None, &[], outputs, v.dials.delay as u64, 0)
        })
        .map_err(|e| {
            fee_words(e, |need, ceiling| {
                format!("finishing now would cost {:.5} KAS in network fees, more than this vault allows ({} KAS). Wait for a quieter network or cancel.", kas(need), kas(ceiling))
            })
        })?;
        let paid = if burn { remainder } else { fee };
        let id = self.try_submit(&tx, "finish").await?;
        v.state = next;
        v.pending_to = None;
        v.history.push(format!("finish {} KAS to {dest}, fee {:.5} KAS, tx {id}", kas(amount), kas(paid)));
        self.save_anchored(&v)?;
        if burn {
            println!("vault emptied; remove state.json before funding again");
        }
        Ok((id, burn))
    }

    /// The emergency stop: everything to the cold address, signed here with stop key `which` (1, 2 or 3).
    pub async fn do_cancel(&self, which: u8) -> Result<String, String> {
        let slot = stop_slot(which)?;
        let stop_key = self.keys.try_stop_key(slot)?;
        let mut v = self.open_vault()?;
        let coin = self.try_vault_coin(&v).await?;
        let cold = self.cold_spk();
        let lift = self.lift_for(coin.1.amount).await?;
        let entries = lift_entries(&coin, lift.as_ref());
        let (tx, fee) = self.try_with_fee(&entries, Some(v.dials.max_fee as u64), &|fee| {
            let total = coin.1.amount + lift.as_ref().map_or(0, |(c, _)| c.1.amount);
            let left = total.checked_sub(fee).ok_or("the vault cannot pay the fee")?;
            if left < v.dials.min_remainder as u64 {
                return Err(format!("stopping now would move {:.5} KAS, less than the {} KAS an exit must carry", kas(left), kas(v.dials.min_remainder as u64)));
            }
            let outputs = vec![TransactionOutput::new(left, cold.clone())];
            // The contract counts stop slots from 0; the command line from 1.
            self.vault_tx_lifted(&v, &coin, lift.as_ref().map(|(c, k)| (c, k)), "cancel", Some(&stop_key), &[V::Int(slot as i64)], outputs, 0, 0)
        })
        .map_err(|e| {
            fee_words(e, |need, ceiling| {
                format!("stopping now would cost {:.5} KAS in network fees, more than this vault allows ({} KAS). Wait for a quieter network.", kas(need), kas(ceiling))
            })
        })?;
        let id = self.try_submit(&tx, "cancel").await?;
        v.history.push(match &lift {
            Some((c, _)) => format!(
                "cancel (stop key {which}): everything to cold, carried by {:.5} KAS from the funding key (the vault alone could not pay its way out), fee {:.5} KAS, tx {id}",
                kas(c.1.amount),
                kas(fee)
            ),
            None => format!("cancel (stop key {which}): everything to cold, fee {:.5} KAS, tx {id}", kas(fee)),
        });
        self.save_anchored(&v)?;
        println!("vault emptied to cold; remove state.json before funding again");
        Ok(id)
    }

    /// v5: a stop key sends a waiting withdrawal back to Locked. Nothing leaves the vault but the
    /// network fee (paid from the vault coin, as `request` pays); the window counters stay as they
    /// are. A stop key, not the spend key, because the spend key may be the thief's.
    pub async fn do_decline(&self, which: u8) -> Result<String, String> {
        let slot = stop_slot(which)?;
        let stop_key = self.keys.try_stop_key(slot)?;
        let mut v = self.open_vault()?;
        if v.state.stage != 1 {
            return Err("Nothing is waiting: the vault is Locked.".into());
        }
        let coin = self.try_vault_coin(&v).await?;
        let amount = v.state.amount as u64;
        let dest = v.pending_to.clone().unwrap_or_else(|| "an unrecorded address".to_string());
        let next = after_decline(&v.state);
        let (tx, fee) = self.try_with_fee(std::slice::from_ref(&coin.1), Some(v.dials.max_fee as u64), &|fee| {
            let left = coin.1.amount.checked_sub(fee).ok_or("the vault cannot pay the fee")?;
            if left < v.dials.min_remainder as u64 {
                return Err(format!(
                    "declining now would leave {:.5} KAS in the vault, less than the {} KAS it must keep. Cancel to move everything to your safe.",
                    kas(left),
                    kas(v.dials.min_remainder as u64)
                ));
            }
            let outputs = vec![self.continuation(left, &next, &v)];
            // The contract counts stop slots from 0; the command line from 1.
            self.vault_tx(&v, &coin, "decline", Some(&stop_key), &[V::Int(slot as i64)], outputs, 0, 0)
        })
        .map_err(|e| {
            fee_words(e, |need, ceiling| {
                format!("declining now would cost {:.5} KAS in network fees, more than this vault allows ({} KAS). Wait for a quieter network or cancel.", kas(need), kas(ceiling))
            })
        })?;
        let id = self.try_submit(&tx, "decline").await?;
        v.state = next;
        v.pending_to = None;
        v.history.push(format!("decline (stop key {which}): the request for {} KAS to {dest} is withdrawn, the vault is Locked again, fee {:.5} KAS, tx {id}", kas(amount), kas(fee)));
        self.save_anchored(&v)?;
        println!("withdrawal declined; the vault is Locked again");
        Ok(id)
    }

    /// Grows the vault. The contract allows exactly one output (the continuation), so there is no
    /// room for change: first a plain payment stages one coin of topup + TOPUP_RESERVE at the funding
    /// address, then the vault coin (input 0) and that staged coin go into one continuation of
    /// old amount + staged coin - fee, state unchanged (the unused part of the reserve stays in the
    /// vault). Locked (stage=0) only, signed with the spend key.
    pub async fn do_topup(&self, amount_kas: f64) -> Result<String, String> {
        let mut v = self.open_vault()?;
        if v.state.stage != 0 {
            return Err("vault is Pending; finish or cancel first (topup is stage=0 only)".into());
        }
        let amount = sompi(amount_kas);
        if amount == 0 {
            return Err("topup needs an amount above zero".into());
        }
        let spend_key = self.spend_key()?;
        let funding = keypair(self.keys.funding.as_ref().ok_or("the funding key is not on this device")?);
        let funding_addr = self.funding_addr()?;
        let coin = self.try_vault_coin(&v).await?;
        floor_check(&self.dials()?, coin.1.amount, amount)?;
        let plain = self.try_utxos(&funding_addr).await?;
        let total: u64 = plain.iter().map(|(_, u)| u.amount).sum();
        let staged = amount + TOPUP_RESERVE;
        // Step 1: stage one plain coin of exactly topup + reserve at the funding address.
        let plain_entries: Vec<UtxoEntry> = plain.iter().map(|(_, u)| u.clone()).collect();
        let (stage_tx, _) = self.try_with_fee(&plain_entries, None, &|fee| {
            if total < staged + fee {
                return Err(format!("funding has {total} sompi, need {}", staged + fee));
            }
            let mut outputs = vec![TransactionOutput::new(staged, pay_to_address_script(&funding_addr))];
            let change = total - staged - fee;
            if change > 0 {
                outputs.push(TransactionOutput::new(change, pay_to_address_script(&funding_addr)));
            }
            self.plain_tx(&plain, outputs, &funding, false)
        })?;
        let stage_id = self.try_submit(&stage_tx, "topup-stage").await?;
        let stage_op = TransactionOutpoint::new(stage_tx.id(), 0);
        let mut staged_coin = None;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if let Some(c) = self.try_utxos(&funding_addr).await?.into_iter().find(|(op, _)| *op == stage_op) {
                staged_coin = Some(c);
                break;
            }
        }
        let staged_coin = staged_coin.ok_or(format!("staged coin {stage_id}:0 did not reach the utxo set in 30 s"))?;
        // Step 2: vault coin + staged coin -> one continuation, nothing else.
        let entries = vec![coin.1.clone(), staged_coin.1.clone()];
        let art = self.try_compile(&v.state)?;
        let (tx, fee) = self.try_with_fee(&entries, Some(TOPUP_RESERVE), &|fee| {
            let inputs = vec![
                TransactionInput::new_with_compute_budget(coin.0, vec![], 0, 0),
                TransactionInput::new_with_compute_budget(staged_coin.0, vec![], 0, 0),
            ];
            let outputs = vec![self.continuation(coin.1.amount + staged - fee, &v.state, &v)];
            let mut tx = Transaction::new(1, inputs, outputs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
            self.try_sign_and_budget(&mut tx, &entries, &|i, t| {
                if i == 0 {
                    self.vault_sigscript(&art, "topup", &[V::Bytes(self.sign_input(t, &entries, 0, &spend_key))])
                } else {
                    self.plain_sigscript(t, &entries, i, &funding)
                }
            })?;
            Ok(tx)
        })
        .map_err(|e| {
            fee_words(e, |need, ceiling| {
                format!("adding to the vault now would cost {:.5} KAS in network fees, more than the {} KAS reserve staged for it (the staged coin stays at the funding address). Wait for a quieter network.", kas(need), kas(ceiling))
            })
        })?;
        let grown = staged - fee;
        let id = self.try_submit(&tx, "topup").await?;
        v.history.push(format!("topup {amount_kas} (+{:.5} unused reserve) stage tx {stage_id} tx {id}", kas(grown - amount)));
        self.save_anchored(&v)?;
        println!("vault now holds {:.4} KAS; state unchanged", kas(coin.1.amount + grown));
        Ok(id)
    }

    /// The heir takes everything minus fee to the heir address once the coin has sat untouched
    /// for the inherit delay; the input sequence carries the age, as `finish` does. Works at any
    /// stage: from Pending (contract v3, M1) the pending request is still honoured, output 0
    /// pays `amount` to its destination and the heir gets the rest at output 1 (fee out of the
    /// heir's share); from Locked the heir gets the whole coin in one output. Either way the
    /// heir's output must carry at least minRemainder (L6). Closes the vault in state.json like cancel.
    pub async fn do_inherit(&self) -> Result<String, String> {
        let public = self.keys.public();
        if public.heir_x.is_empty() {
            return Err("this vault has no heir (no heir_x in the public part of keys.json)".into());
        }
        let heir_key = self.keys.heir.as_ref().map(keypair).ok_or("the heir key is not on this device")?;
        if faster_hex::hex_string(&xonly(&heir_key)) != public.heir_x {
            return Err("the heir secret here is not this vault's heir key".into());
        }
        self.heir_exit("inherit", Some(&heir_key)).await
    }

    /// v5 `inheritOpen`: the same inheritance with no key at all, so a watcher (or anyone) can send
    /// it the moment the silence has passed. Only a vault made with `keys --auto-heir` accepts it;
    /// the outputs are fixed by the contract, so whoever sends it can only pay the fee.
    pub async fn do_inherit_open(&self) -> Result<String, String> {
        let public = self.keys.public();
        if !public.auto_heir {
            return Err("This vault does not send to the heir by itself: it was set up without the auto-heir dial.".into());
        }
        if public.heir_x.is_empty() {
            return Err("this vault has no heir (no heir_x in the public part of keys.json)".into());
        }
        self.heir_exit("inheritOpen", None).await
    }

    /// The heir exit shared by `inherit` (heir key) and `inheritOpen` (no key): from Pending the
    /// request's destination is paid first, then the heir gets the rest minus the fee.
    async fn heir_exit(&self, entry: &str, signer: Option<&Keypair>) -> Result<String, String> {
        let public = self.keys.public();
        let mut v = self.open_vault()?;
        let coin = self.try_vault_coin(&v).await?;
        let delay = public.inherit_delay(&v.dials);
        println!("coin age {} DAA, inherit delay {delay}", self.try_daa().await?.saturating_sub(coin.1.block_daa_score));
        let heir_spk = self.network.try_to_spk(&public.heir_address)?;
        // From Pending the request's destination is the one `request` stored in state.json
        // (checked against the recorded hash exactly as `finish` does).
        let pending = if v.state.stage == 1 {
            let dest = v.pending_to.clone().ok_or("the vault is Pending but state.json holds no pending destination")?;
            let dest_script = self.network.try_to_spk(&dest)?;
            if spk_hash(&dest_script) != v.state.dest_hash {
                return Err("stored destination does not match recorded hash".into());
            }
            Some((v.state.amount as u64, dest_script, dest))
        } else {
            None
        };
        let paid = pending.as_ref().map_or(0, |p| p.0);
        let lift = self.lift_for(coin.1.amount.saturating_sub(paid)).await?;
        let entries = lift_entries(&coin, lift.as_ref());
        let (tx, fee) = self.try_with_fee(&entries, Some(v.dials.max_fee as u64), &|fee| {
            let total = coin.1.amount + lift.as_ref().map_or(0, |(c, _)| c.1.amount);
            let left = total.checked_sub(paid + fee).ok_or("the vault cannot pay the pending amount and the fee")?;
            if left < v.dials.min_remainder as u64 {
                return Err(format!("the heir's share would be {} KAS, under the {} KAS minimum a closing output must carry", kas(left), kas(v.dials.min_remainder as u64)));
            }
            let mut outputs = Vec::new();
            if let Some((amount, dest_script, _)) = &pending {
                outputs.push(TransactionOutput::new(*amount, dest_script.clone()));
            }
            outputs.push(TransactionOutput::new(left, heir_spk.clone()));
            self.vault_tx_lifted(&v, &coin, lift.as_ref().map(|(c, k)| (c, k)), entry, signer, &[], outputs, delay as u64, 0)
        })
        .map_err(|e| {
            fee_words(e, |need, ceiling| {
                format!("passing the vault on now would cost {:.5} KAS in network fees, more than this vault allows ({} KAS). Wait for a quieter network.", kas(need), kas(ceiling))
            })
        })?;
        let id = self.try_submit(&tx, entry).await?;
        let carried = match &lift {
            Some((c, _)) => format!(", carried by {:.5} KAS from the funding key (the vault alone could not pay its way out)", kas(c.1.amount)),
            None => String::new(),
        };
        match &pending {
            Some((amount, _, dest)) => v.history.push(format!(
                "{entry} from Pending: {} KAS to {dest} (the pending request), the rest to heir {}{carried}, fee {:.5} KAS, tx {id}",
                kas(*amount),
                public.heir_address,
                kas(fee)
            )),
            None => v.history.push(format!("{entry}: everything to heir {}{carried}, fee {:.5} KAS, tx {id}", public.heir_address, kas(fee))),
        }
        self.save_anchored(&v)?;
        println!("vault emptied to the heir; remove state.json before funding again");
        Ok(id)
    }

    /// The contract's L6 lift. `have` is what the vault can move at the exit (its coin, minus a pending
    /// payment). Below VAULT_FLOOR it cannot pay the fee and still leave minRemainder, so the smallest
    /// funding coin that makes up the difference rides along as a second input (None when the vault pays
    /// its own way). Refuses in words when nothing here can carry it.
    async fn lift_for(&self, have: u64) -> Result<Option<(Coin, Keypair)>, String> {
        let d = self.dials()?;
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
        let Some(secret) = self.keys.funding.as_ref() else {
            return Err(format!("{too_small}, and the funding key is not on this device to carry it. Run this from the device that holds it, or add to the vault first."));
        };
        let funding = keypair(secret);
        let mut coins = self.try_utxos(&self.funding_addr()?).await?;
        coins.retain(|(_, u)| !u.is_coinbase);
        coins.sort_by_key(|(_, u)| u.amount);
        match coins.into_iter().find(|(_, u)| u.amount >= short) {
            Some(c) => {
                println!("the vault alone cannot pay its way out; {:.5} KAS from the funding key rides along to carry it", kas(c.1.amount));
                Ok(Some((c, funding)))
            }
            None => Err(format!("{too_small}, and the funding key has no coin of {:.5} KAS to carry it. Pay at least that much to the funding address first.", kas(short))),
        }
    }
}

/// The new record's history after a re-open: the closed record's lines (oldest first), a marker, then the fund line.
/// The fund line stays last because readers take the newest line from the end.
pub fn carry_history(old: Vec<String>, fund_line: String) -> Vec<String> {
    if old.is_empty() {
        return vec![fund_line];
    }
    let mut h = old;
    h.push("Rules updated".into());
    h.push(fund_line);
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_reopened_account_keeps_its_past() {
        let old = vec!["fund 10 KAS tx a".to_string(), "cancel (stop key 1): everything to the safe, tx b".to_string()];
        let h = super::carry_history(old.clone(), "fund 9 KAS tx c".into());
        assert_eq!(h.len(), 4);
        assert_eq!(&h[..2], &old[..]);
        assert_eq!(h[2], "Rules updated", "plain words: never reads like a new account");
        assert_eq!(h.last().unwrap(), "fund 9 KAS tx c", "newest line last: is_stopped and fee_of read the last one");
        assert_eq!(super::carry_history(vec![], "fund 1 KAS tx d".into()), vec!["fund 1 KAS tx d".to_string()]);
    }

    use super::*;

    fn do_keys_t(dir: &Path, o: KeysOpts) -> Result<(), String> {
        do_keys(dir, o, &Network::testnet())
    }

    #[test]
    fn keys_import_refuses_a_file_made_for_another_network() {
        let root = std::env::temp_dir().join("kv-net-mismatch");
        std::fs::remove_dir_all(&root).ok();
        let (off, on) = (root.join("off"), root.join("on"));
        std::fs::create_dir_all(&off).unwrap();
        std::fs::create_dir_all(&on).unwrap();
        let opts = |import: Option<PathBuf>| KeysOpts { auto_heir: false, hard_limit: false, plain: true, for_phone: false, offline: false, import_public: import, heir_x: None };
        do_keys(&off, KeysOpts { offline: true, ..opts(None) }, &Network::testnet()).unwrap();
        let export = off.join(OFFLINE_EXPORT_FILE);
        assert!(std::fs::read_to_string(&export).unwrap().contains("\"network\": \"testnet\""));
        // the online device started for mainnet refuses the testnet export in words that say what to do
        let err = do_keys(&on, opts(Some(export.clone())), &Network::mainnet()).unwrap_err();
        assert!(err.contains("made for the testnet network") && err.contains("--network testnet"), "{err}");
        assert!(!on.join("keys.json").exists());
        // the right network takes it and records itself in the rule block
        do_keys(&on, opts(Some(export)), &Network::testnet()).unwrap();
        let online = Keys::load(&on.join("keys.json")).unwrap();
        assert_eq!(online.public().network, "testnet");
        assert!(Network::mainnet().check_recorded(&online.public().network, "keys.json").unwrap_err().contains("--network testnet"));
        // carrying the block back to an offline folder started for mainnet is refused the same way
        let carry = on.join(PUBLIC_EXPORT_FILE);
        let err = do_keys(&off, opts(Some(carry)), &Network::mainnet()).unwrap_err();
        assert!(err.contains("made for the testnet network"), "{err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fund_and_topup_refuse_a_vault_born_stuck() {
        let err = floor_check(&Dials::testnet(), 0, sompi(0.9)).unwrap_err();
        assert!(err.starts_with("a vault needs at least 1.05 KAS so it can always be stopped or passed on"), "{err}");
        assert_eq!(floor_check(&Dials::testnet(), 0, sompi(1.05)), Ok(()));
        assert_eq!(floor_check(&Dials::testnet(), 0, sompi(2.5)), Ok(()));
        // topup: what the vault holds counts
        let err = floor_check(&Dials::testnet(), sompi(0.5), sompi(0.3)).unwrap_err();
        assert!(err.contains("on top of the 0.50000 KAS it holds"), "{err}");
        assert_eq!(floor_check(&Dials::testnet(), sompi(0.9), sompi(0.15)), Ok(()));
    }

    #[test]
    fn save_anchored_records_the_code_once_and_refuses_a_changed_block() {
        use kaspa_consensus_core::Hash;
        let dir = std::env::temp_dir().join(format!("kv-save-anchored-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx { dir: dir.clone(), keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        std::fs::write(dir.join("keys.json"), serde_json::to_string_pretty(&ctx.keys).unwrap()).unwrap();
        let v = Vault { covenant_id: Hash::from_bytes([6u8; 32]), state: locked(), pending_to: None, history: vec![], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        // a vault file from before the code: the next save records it
        ctx.try_save_vault(&v).unwrap();
        assert_eq!(contract::recorded_public_sha256(&dir.join("state.json")), Ok(None));
        ctx.save_anchored(&v).unwrap();
        assert_eq!(contract::recorded_public_sha256(&dir.join("state.json")), Ok(Some(ctx.keys.public().sha256())));
        assert!(ctx.try_load_vault().unwrap().is_some(), "the vault still loads with the code beside it");
        // another device's keys.json against this vault: refused in bank words, state.json untouched
        let other = Ctx { dir: dir.clone(), keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 };
        let before = std::fs::read_to_string(dir.join("state.json")).unwrap();
        assert_eq!(other.save_anchored(&v), Err(contract::RULE_BLOCK_CHANGED.to_string()));
        assert_eq!(std::fs::read_to_string(dir.join("state.json")).unwrap(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fee_words_keep_the_builders_own_refusal() {
        let over = fee_words(FeeError::OverCeiling { need: 1, ceiling: 2 }, |n, c| format!("over {n} {c}"));
        assert_eq!(over, "over 1 2");
        assert_eq!(fee_words(FeeError::Build(format!("{OVER_CAP}: x")), |_, _| String::new()), format!("{OVER_CAP}: x"));
        assert!(fee_words(FeeError::NotSettled, |_, _| String::new()).starts_with("The network charge could not be worked out"));
    }

    #[test]
    fn keys_offline_then_import_on_both_sides() {
        let root = std::env::temp_dir().join(format!("kv-keys-flow-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let (off, on) = (root.join("offline"), root.join("online"));
        std::fs::create_dir_all(&off).unwrap();
        std::fs::create_dir_all(&on).unwrap();
        let opts = |import: Option<PathBuf>, heir_x: Option<String>| KeysOpts { auto_heir: false, hard_limit: false, plain: true, for_phone: false, offline: false, import_public: import, heir_x };
        do_keys_t(&off, KeysOpts { offline: true, ..opts(None, None) }).unwrap();
        let export = off.join(OFFLINE_EXPORT_FILE);
        assert!(export.exists() && !std::fs::read_to_string(&export).unwrap().contains("\"cancel\""));
        let err = do_keys_t(&off, KeysOpts { offline: true, ..opts(None, None) }).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert!(do_keys_t(&off, KeysOpts { offline: true, ..opts(Some(export.clone()), None) }).unwrap_err().contains("do not go together"));
        // online: the heir's own key, so no heir secret anywhere
        let heir_x = faster_hex::hex_string(&xonly(&keypair(&[5u8; 32])));
        do_keys_t(&on, opts(Some(export.clone()), Some(heir_x.clone()))).unwrap();
        let online = Keys::load(&on.join("keys.json")).unwrap();
        assert!(online.spend.is_some() && online.funding.is_some() && online.heir.is_none() && online.cancel.is_none() && online.cold.is_none());
        assert_eq!(online.public().heir_x, heir_x);
        online.public().check_complete().unwrap();
        // back on the offline device: the block lands and matches the secrets there
        let carry = on.join(PUBLIC_EXPORT_FILE);
        do_keys_t(&off, opts(Some(carry.clone()), None)).unwrap();
        let offline = Keys::load(&off.join("keys.json")).unwrap();
        assert_eq!(offline.public_sha256, online.public_sha256);
        assert_eq!(offline.check_self(), Ok(()));
        assert!(offline.spend.is_none() && offline.cancel.is_some() && offline.cold3.is_some());
        assert!(do_keys_t(&off, opts(Some(carry), None)).unwrap_err().contains("already has a rule block"));
        // the old flow still works and writes the code; a file that is neither export is refused in words
        let all = root.join("all");
        std::fs::create_dir_all(&all).unwrap();
        do_keys_t(&all, opts(None, Some(heir_x.clone()))).unwrap();
        let k = Keys::load(&all.join("keys.json")).unwrap();
        assert!(k.heir.is_none() && k.public_sha256.is_some() && k.public().heir_x == heir_x);
        std::fs::write(root.join("x.json"), "{}").unwrap();
        assert!(do_keys_t(&on, opts(Some(root.join("x.json")), None)).unwrap_err().contains("neither"));
        std::fs::remove_dir_all(&root).ok();
    }
}
