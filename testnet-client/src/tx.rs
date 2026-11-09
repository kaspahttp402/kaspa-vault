//! Chain plumbing shared by every command: the `Ctx` (folder, keys, node), the pinned-source
//! compile of the vault for a state, utxo and DAA lookups, signing with a measured compute
//! budget, the relay fee, and submitting to the node.

use crate::contract::{self, Coin, Dials, KAS, State, Vault, kas};
#[cfg(test)]
use crate::contract::{MAX_FEE, MIN_REMAINDER};
use crate::network::Network;
use kaspa_consensus_core::tx::GenesisCovenantGroup;
use crate::keys::Keys;
use kaspa_addresses::Address;
use kaspa_consensus_core::hashing::sighash::{SigHashReusedValuesUnsync, calc_schnorr_signature_hash};
use kaspa_consensus_core::hashing::sighash_type::SIG_HASH_ALL;
use kaspa_consensus_core::mass::MassCalculator;
use kaspa_consensus_core::mass::units::{ComputeBudget, ScriptUnits};
use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
use kaspa_consensus_core::tx::{
    ComputeCommit, CovenantBinding, MutableTransaction, PopulatedTransaction, ScriptPublicKey, Transaction, TransactionInput, TransactionOutpoint,
    TransactionOutput, UtxoEntry, VerifiableTransaction,
};
use kaspa_txscript::caches::Cache;
use kaspa_txscript::covenants::CovenantsContext;
use kaspa_txscript::script_builder::ScriptBuilder;
use kaspa_txscript::{EngineCtx, EngineFlags, TxScriptEngine};
use kaspa_wrpc_client::prelude::*;
use secp256k1::{Keypair, Message};
use silverscript_abi::{ArtifactValue as V, SilAbiArtifact, encode_contract_entry_sig_script};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const FEE_GUESS: u64 = 300_000;
/// KIP-9 storage-mass parameter of the consensus mass rule (sompi * grams).
pub const STORAGE_MASS_PARAMETER: u64 = 1_000_000_000_000;
/// The testnet preset's floor (minRemainder + maxFee), kept for the tests; live code asks `Dials::floor`:
/// a vault below it can never cancel, inherit or request on its own, since the exit output must carry
/// minRemainder and the fee may take up to maxFee.
#[cfg(test)]
pub const VAULT_FLOOR: u64 = (MIN_REMAINDER + MAX_FEE) as u64;
/// Plain change below this is folded away rather than kept: a small coin costs more to move than it holds.
pub const CHANGE_FLOOR: u64 = KAS / 2;
/// The highest fee rate this program pays, as a multiple of the network's floor. A node that
/// names more is held to it, so a lying or broken node cannot price a move; the offline device
/// refuses a request above it (airgap::check_fee_rate).
pub const FEE_RATE_MULTIPLE: u64 = 10;
/// A plain send (purse, heir) has no dial bounding its fee: it may pay PLAIN_FEE_CEILING, or
/// PLAIN_FEE_MULTIPLE times what it costs at the network floor, whichever is larger.
pub const PLAIN_FEE_CEILING: u64 = KAS / 20;
pub const PLAIN_FEE_MULTIPLE: u64 = 3;
pub const FEE_TOO_HIGH: &str =
    "The network fee is unusually high right now. No funds were moved. Please try again later.";
/// Opens the payload of every covenant spend `vault_tx_lifted` builds. The payload is
/// `PAYLOAD_TAG` ("KV1", 3 bytes) then the transaction's lock_time as 8 bytes little-endian:
/// 11 bytes, public, nothing secret. Public indexers drop lock_time, but the next covenant state
/// can depend on it (a spend's window_start), so a recovery walk reads it back with
/// `lock_time_from_payload` instead of searching. The contract never reads it; the signature
/// hash covers it (a native transaction with a payload hashes it) and the mass counts it.
pub const PAYLOAD_TAG: &[u8; 3] = b"KV1";

/// The payload of a covenant spend with `lock_time` (format at `PAYLOAD_TAG`).
pub fn lock_time_payload(lock_time: u64) -> Vec<u8> {
    let mut payload = PAYLOAD_TAG.to_vec();
    payload.extend_from_slice(&lock_time.to_le_bytes());
    payload
}

/// The lock_time a covenant spend's payload echoes; None for any other payload (empty, an older
/// spend, another tag, a wrong length), so the walk falls back to its bounded search.
pub fn lock_time_from_payload(payload: &[u8]) -> Option<u64> {
    let rest = payload.strip_prefix(PAYLOAD_TAG)?;
    Some(u64::from_le_bytes(rest.try_into().ok()?))
}
/// The fee rate in use, sompi per gram: the one named place. `use_fee_floor` sets the network's floor
/// at start; `connect` raises it to the node's estimate (`get_fee_estimate`, normal bucket), never below
/// the floor nor above FEE_RATE_MULTIPLE times it; the offline device keeps the floor.
static FEE_RATE: AtomicU64 = AtomicU64::new(crate::network::TESTNET_FEE_FLOOR);

pub fn fee_rate() -> u64 {
    FEE_RATE.load(Ordering::Relaxed)
}
/// Starts the fee rate at this network's floor (main, before any connect).
pub fn use_fee_floor(net: &Network) {
    FEE_RATE.store(net.fee_floor, Ordering::Relaxed);
}
/// The offline device takes the fee floor the online device measured and wrote into the signing
/// request (item 5), never below its own network's floor nor above FEE_RATE_MULTIPLE times it.
pub fn set_fee_rate(rate: u64, net: &Network) {
    FEE_RATE.store(clamp_fee_rate(rate, net), Ordering::Relaxed);
}
/// A fee rate as this program uses it: never below the network's floor, never above
/// FEE_RATE_MULTIPLE times it.
pub fn clamp_fee_rate(rate: u64, net: &Network) -> u64 {
    rate.clamp(net.fee_floor, FEE_RATE_MULTIPLE.saturating_mul(net.fee_floor))
}
/// The node's estimate (sompi per gram, a float on the wire) as this program uses it, clamped;
/// a value that is not a number reads as zero, so as the floor.
pub fn node_fee_rate(feerate: f64, net: &Network) -> u64 {
    clamp_fee_rate(feerate.ceil() as u64, net)
}
/// The fee for `mass` grams at `rate` sompi per gram, plus ten percent; saturating, never wrapping.
pub fn fee_at(mass: u64, rate: u64) -> u64 {
    mass.saturating_mul(rate).saturating_mul(11) / 10
}
/// How far the measured network clock may stray from the preset before `connect` refuses (a fifth).
pub const RATE_TOLERANCE: f64 = 0.20;

/// The network's clock measured between two block headers, the pruning point and the sink (hours to
/// days apart): DAA units per second of their timestamps.
async fn measure_daa_rate(rpc: &KaspaRpcClient) -> Result<f64, String> {
    let dag = rpc.get_block_dag_info().await.map_err(|e| format!("{e}"))?;
    let sink = rpc.get_block(dag.sink, false).await.map_err(|e| format!("{e}"))?;
    let root = rpc.get_block(dag.pruning_point_hash, false).await.map_err(|e| format!("{e}"))?;
    let secs = (sink.header.timestamp as f64 - root.header.timestamp as f64) / 1000.0;
    if secs <= 0.0 {
        return Err("the node's block times did not move forward".into());
    }
    Ok((sink.header.daa_score as f64 - root.header.daa_score as f64) / secs)
}
/// The measured clock must sit within `tolerance` of the rate the dials were made with; otherwise every
/// day-long dial would be the wrong length.
pub fn check_daa_rate(measured: f64, expected: f64, tolerance: f64) -> Result<(), String> {
    if !measured.is_finite() || (measured - expected).abs() > expected * tolerance {
        return Err(format!(
            "The network clock is running at about {measured:.2} blocks a second, but this app \
             expects {expected:.1} (within {:.0} percent), so the daily limit, notice period and \
             inactivity period would run the wrong length. Please wait and try again, or use \
             another node.",
            tolerance * 100.0
        ));
    }
    Ok(())
}

/// Why a fee loop stopped without a transaction. `Build` carries the builder's own refusal, word for word.
#[derive(Debug, Clone, PartialEq)]
pub enum FeeError {
    /// The fee the network needs is above what this move may pay (both in sompi).
    OverCeiling { need: u64, ceiling: u64 },
    /// Fee and mass chased each other for eight rounds.
    NotSettled,
    /// A plain send would pay more than `plain_ceiling`: the network or the node asks too much.
    TooHigh,
    Build(String),
}

impl std::fmt::Display for FeeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeeError::OverCeiling { need, ceiling } => write!(f, "the network fee would be {:.5} KAS, more than the {} KAS this move allows", kas(*need), kas(*ceiling)),
            FeeError::NotSettled => write!(f, "The network charge could not be worked out. Try again in a moment."),
            FeeError::TooHigh => write!(f, "{FEE_TOO_HIGH}"),
            FeeError::Build(s) => write!(f, "{s}"),
        }
    }
}

impl From<FeeError> for String {
    fn from(e: FeeError) -> String {
        e.to_string()
    }
}

/// A priced move: the transaction as it would be sent and the fee it pays. Nothing was sent.
pub struct Quote {
    pub tx: Transaction,
    pub fee: u64,
}

#[derive(Clone)]
pub struct Ctx {
    pub dir: PathBuf,
    pub keys: Keys,
    /// None on the offline device.
    pub rpc: Option<KaspaRpcClient>,
    /// Keep going when the engine preflight fails (so the node's own rejection can be seen).
    pub force: bool,
    /// The network this folder lives on (from `--network`); every address and node check reads it here.
    pub network: Network,
    /// The dials a `fund` would open a vault with (the network's preset, turned by the fund flags). Once a
    /// vault exists its own dials in state.json win: see `dials()`.
    pub dials: Dials,
    /// Which account of this key set this context works on: 1 = `state.json`, N = `state-N.json` (several
    /// accounts share one key set, one purse and one safe; every money method takes `account`).
    pub account: u32,
}

/// Said when a node address would carry the vault's questions and answers in the clear.
pub const INSECURE_NODE: &str =
    "Only secure network connections are allowed. Use a node address that starts with wss://.";

/// The public resolver, asked for TLS (wss://) nodes only. kaspa-wrpc-client 2.1.0's
/// `Resolver::default()` asks for "any" node and can hand out a plain ws:// one that anyone on the
/// same Wi-Fi could read, or answer falsely (hide a withdrawal from the guard). The crate keeps the
/// first resolver's tls choice for the whole process, so every resolver here comes from this one.
pub fn secure_resolver() -> Resolver {
    Resolver::new(None, true)
}

/// Lets through only node addresses that encrypt: `wss://`. Plain `ws://` is allowed to this
/// machine itself (127.0.0.1, [::1]; never a name like localhost) on the laptop only, for the CLI guard's own local
/// node (kaspad's wRPC has no TLS); loopback never crosses a network. Never on the phone.
pub fn check_secure_url(url: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.strip_prefix("wss://").is_some_and(|rest| !rest.is_empty()) {
        return Ok(());
    }
    match lower.strip_prefix("ws://") {
        Some(rest) if !cfg!(target_os = "android") && is_loopback(rest) => Ok(()),
        _ => Err(INSECURE_NODE.to_string()),
    }
}

/// True for `:` followed by digits only.
fn is_port(port: &str) -> bool {
    port.strip_prefix(':').is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// True when a url's part after `ws://` names this machine, with at most a numeric port.
fn is_loopback(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    ["127.0.0.1", "[::1]"].iter().any(|host| {
        authority.strip_prefix(host).is_some_and(|port| port.is_empty() || is_port(port))
    })
}

/// Opens the socket, TLS only: an explicit url must pass `check_secure_url`; with none, the
/// resolver is asked for wss nodes. Fails closed: when the node it reached is not secure (or no
/// secure node answers) it disconnects and says the network can't be reached; never falls back.
async fn open_secure(
    url: Option<&str>,
    net: &Network,
    where_: &str,
) -> Result<KaspaRpcClient, String> {
    let url = url.map(str::trim);
    if let Some(u) = url {
        check_secure_url(u)?;
    }
    let resolver = url.is_none().then(secure_resolver);
    let rpc = KaspaRpcClient::new(WrpcEncoding::Borsh, url, resolver, Some(net.resolver_id()), None)
        .map_err(|e| format!("Can't set up the connection to {where_}: {e}"))?;
    let strategy = ConnectStrategy::Fallback;
    let options = ConnectOptions { block_async_connect: true, strategy, ..Default::default() };
    rpc.connect(Some(options)).await.map_err(|e| {
        format!("Can't reach the network right now ({where_}): no secure node answered: {e}")
    })?;
    let reached = ensure_secure(&rpc, where_).await?;
    let scheme = reached.split("://").next().unwrap_or("?").to_ascii_lowercase();
    let kind = if scheme == "wss" { "encrypted, TLS" } else { "this machine only" };
    println!("node link: {scheme}:// ({kind})");
    Ok(rpc)
}

/// Checks the url the client actually reached (after a connect OR a reconnect, where a resolver
/// may hand back another node): not secure -> disconnect and fail closed. Returns that url.
pub(crate) async fn ensure_secure(rpc: &KaspaRpcClient, where_: &str) -> Result<String, String> {
    let reached = rpc.url().unwrap_or_default();
    if let Err(e) = check_secure_url(&reached) {
        rpc.disconnect().await.ok();
        return Err(format!("Can't reach the network right now ({where_}): {e}"));
    }
    Ok(reached)
}

/// Connects to the folder's network: a pinned node by wRPC url, or a public one through the
/// resolver, both TLS only (`open_secure`). A node that reports another network is refused: it is
/// lying or set up wrong.
pub async fn connect(url: Option<&str>, net: &Network) -> Result<KaspaRpcClient, String> {
    let where_ = url.map(str::to_string).unwrap_or_else(|| format!("a public {} node", net.id));
    let rpc = open_secure(url, net, &where_).await?;
    let info = rpc
        .get_server_info()
        .await
        .map_err(|e| format!("The node did not say which network it is on ({where_}): {e}"))?;
    println!("node: {} synced={} network={}", info.server_version, info.is_synced, info.network_id);
    net.check_node(&info.network_id, &where_)?;
    if !info.is_synced {
        return Err(format!(
            "The node is still catching up with the network ({where_}). \
             Please wait, or use another node."
        ));
    }
    match rpc.get_fee_estimate().await {
        Ok(est) => {
            let normal = est.normal_buckets.first();
            let node = normal.map_or(est.priority_bucket.feerate, |b| b.feerate);
            FEE_RATE.store(node_fee_rate(node, net), Ordering::Relaxed);
            let (using, floor) = (fee_rate(), net.fee_floor);
            let cap = FEE_RATE_MULTIPLE * floor;
            println!(
                "fee rate: the node says {node:.1} sompi/gram; using {using} (floor {floor}, cap {cap})"
            );
        }
        Err(e) => println!("fee rate: the node gave no estimate ({e}); using {} sompi/gram", fee_rate()),
    }
    // Item 5: the clock the day-long dials are made with, measured, and refused on mainnet when it is off.
    match measure_daa_rate(&rpc).await {
        Ok(rate) => {
            println!("clock: about {rate:.2} DAA per second (this program expects {:.1})", net.daa_per_sec);
            if let Err(e) = check_daa_rate(rate, net.daa_per_sec, RATE_TOLERANCE) {
                if net.is_mainnet() {
                    return Err(e);
                }
                println!("clock: {e}");
            }
        }
        Err(e) => {
            if net.is_mainnet() {
                return Err(format!(
                    "Could not check the network clock ({e}). Real funds need that check; \
                     please try again, or use another node."
                ));
            }
            println!("clock: could not measure the network clock ({e}); going on with {:.1} DAA per second", net.daa_per_sec);
        }
    }
    Ok(rpc)
}

/// The most accounts one key set may open on a phone (Home lists them; the ids are 1..=this).
pub const MAX_ACCOUNTS: u32 = 8;

/// The state file of an account: `state.json` for account 1 (every folder from before several accounts), `state-N.json` after.
pub fn state_file_name(account: u32) -> String {
    if account <= 1 { "state.json".to_string() } else { format!("state-{account}.json") }
}

/// The account ids that have a record in `dir`, in order (an account that was never funded has none).
pub fn accounts_in(dir: &Path) -> Vec<u32> {
    (1..=MAX_ACCOUNTS).filter(|n| dir.join(state_file_name(*n)).exists()).collect()
}

/// The lowest account id with no record yet, or none when every slot is taken.
pub fn next_account(dir: &Path) -> Option<u32> {
    (1..=MAX_ACCOUNTS).find(|n| !dir.join(state_file_name(*n)).exists())
}

impl Ctx {
    /// `<dir>/state.json` for account 1, `<dir>/state-N.json` for the others (`state_file_name`).
    pub fn state_path(&self) -> PathBuf {
        self.dir.join(state_file_name(self.account))
    }
    /// The same context on another account of the key set.
    pub fn with_account(&self, account: u32) -> Ctx {
        Ctx { account, ..self.clone() }
    }
    pub fn rpc(&self) -> &KaspaRpcClient {
        self.rpc.as_ref().expect("this command needs the network")
    }
    pub fn cold_spk(&self) -> ScriptPublicKey {
        self.network.to_spk(&self.keys.public().cold_address)
    }

    /// The contract source this folder is bound to (see contract.rs). An open vault must carry
    /// `contract_sha256`; older state files are told how to pin.
    fn source(&self) -> Result<String, String> {
        let expected = match self.try_load_vault()? {
            Some(Vault { contract_sha256: Some(h), .. }) => Some(h),
            Some(_) => {
                return Err(format!(
                    "state.json has no contract_sha256: copy contracts/vault.sil to {} and add its SHA-256 as \"contract_sha256\"",
                    self.dir.join(contract::SOURCE_FILE).display()
                ));
            }
            None => None,
        };
        contract::load_source(&self.dir, &self.state_path(), expected.as_deref())
    }
    pub fn compile(&self, s: &State) -> SilAbiArtifact {
        self.try_compile(s).unwrap_or_else(|e| panic!("{e}"))
    }
    pub fn try_compile(&self, s: &State) -> Result<SilAbiArtifact, String> {
        self.try_compile_with(&self.dials()?, s)
    }
    /// The vault for a state under a given set of dials (the offline device takes them from the signing request).
    pub fn try_compile_with(&self, dials: &Dials, s: &State) -> Result<SilAbiArtifact, String> {
        Ok(contract::compile(&self.source()?, &self.keys.public(), dials, s))
    }
    pub fn compile_with(&self, dials: &Dials, s: &State) -> SilAbiArtifact {
        self.try_compile_with(dials, s).unwrap_or_else(|e| panic!("{e}"))
    }
    /// The dials in force: the open vault's own (state.json), else the ones a `fund` would use.
    pub fn dials(&self) -> Result<Dials, String> {
        Ok(self.try_load_vault()?.map_or_else(|| self.dials.clone(), |v| v.dials))
    }
    pub fn try_vault_spk(&self, s: &State) -> Result<ScriptPublicKey, String> {
        Ok(contract::spk_of(&self.try_compile(s)?))
    }
    pub fn try_vault_address(&self, s: &State) -> Result<Address, String> {
        Ok(self.network.address_of(&self.try_vault_spk(s)?).expect("p2sh address"))
    }
    pub fn vault_spk(&self, s: &State) -> ScriptPublicKey {
        contract::spk_of(&self.compile(s))
    }
    pub fn vault_address(&self, s: &State) -> Address {
        self.network.address_of(&self.vault_spk(s)).expect("p2sh address")
    }

    pub fn try_load_vault(&self) -> Result<Option<Vault>, String> {
        match std::fs::read_to_string(self.state_path()) {
            Ok(s) => {
                let v: Vault = serde_json::from_str(&s).map_err(|e| format!("state.json: {e}"))?;
                self.network.check_recorded(&v.network, "state.json")?;
                Ok(Some(v))
            }
            Err(_) => Ok(None),
        }
    }
    pub fn load_vault(&self) -> Option<Vault> {
        self.try_load_vault().unwrap_or_else(|e| panic!("{e}"))
    }
    pub fn try_save_vault(&self, v: &Vault) -> Result<(), String> {
        let text = serde_json::to_string_pretty(v).map_err(|e| format!("state.json: {e}"))?;
        std::fs::write(self.state_path(), text).map_err(|e| format!("state.json: {e}"))
    }

    pub async fn try_utxos(&self, address: &Address) -> Result<Vec<Coin>, String> {
        let entries = self.rpc().get_utxos_by_addresses(vec![address.clone()]).await.map_err(|e| format!("get_utxos_by_addresses: {e}"))?;
        Ok(entries.into_iter().map(coin_of).collect())
    }

    /// The safe coin a stop from another phone (a guardian's key) made from this record's coin, by its tx id: the
    /// newest safe coin whose paying input sat at this account's address (node RPC `get_utxo_return_address`).
    /// The record may predate a helper's decline, so a Pending record also tries its Locked-again address.
    /// None when no recent safe coin came from this account (an heir claim, or the stop not in a
    /// block yet). `want`: only the safe coin made by that very tx (a sibling account with the same
    /// rules shares the address, so its old stop's coin must not pass for this one); None takes the
    /// newest match.
    pub async fn stop_from_elsewhere(&self, v: &Vault, want: Option<&str>)
        -> Result<Option<String>, String> {
        let mut homes = vec![self.try_vault_address(&v.state)?];
        if v.state.stage == 1 {
            homes.push(self.try_vault_address(&crate::contract::after_decline(&v.state))?);
        }
        let cold = self.network.address(&self.keys.public().cold_address)?;
        let coins = self.try_utxos(&cold).await?;
        for (o, u) in stop_candidates(coins, want) {
            // A coin the node can no longer answer for: skip it, never guess.
            if let Ok(from) = self.rpc().get_utxo_return_address(o.transaction_id, u.block_daa_score).await
                && homes.contains(&from)
            {
                return Ok(Some(o.transaction_id.to_string()));
            }
        }
        Ok(None)
    }

    pub async fn try_daa(&self) -> Result<u64, String> {
        Ok(self.rpc().get_block_dag_info().await.map_err(|e| format!("get_block_dag_info: {e}"))?.virtual_daa_score)
    }

    /// Finds the single live vault coin and checks it matches the stored state and lineage.
    pub async fn try_vault_coin(&self, v: &Vault) -> Result<Coin, String> {
        let addr = self.try_vault_address(&v.state)?;
        let coins = self.try_utxos(&addr).await?;
        let mut live: Vec<_> = coins.into_iter().filter(|(_, u)| u.covenant_id == Some(v.covenant_id)).collect();
        if live.len() != 1 {
            return Err(match live.len() {
                0 => format!("no vault coin with covenant id {} at {addr}; is the last transaction accepted yet?", v.covenant_id),
                n => format!(
                    "This account should hold one covenant coin, but the network shows {n}. \
                     No funds were moved."
                ),
            });
        }
        Ok(live.pop().unwrap())
    }

    pub fn sign_input(&self, tx: &Transaction, entries: &[UtxoEntry], idx: usize, k: &Keypair) -> Vec<u8> {
        let reused = SigHashReusedValuesUnsync::new();
        let mtx = MutableTransaction::with_entries(tx.clone(), entries.to_vec());
        let hash = calc_schnorr_signature_hash(&mtx.as_verifiable(), idx, SIG_HASH_ALL, &reused);
        let sig = k.sign_schnorr(Message::from_digest_slice(hash.as_bytes().as_slice()).unwrap());
        let mut b = sig.as_ref().to_vec();
        b.push(SIG_HASH_ALL.to_u8());
        b
    }

    /// A plain pay-to-pubkey signature script for input `idx`.
    pub fn plain_sigscript(&self, tx: &Transaction, entries: &[UtxoEntry], idx: usize, k: &Keypair) -> Vec<u8> {
        ScriptBuilder::new().add_data(&self.sign_input(tx, entries, idx, k)).unwrap().drain()
    }

    pub fn vault_sigscript(&self, art: &SilAbiArtifact, entry: &str, args: &[V]) -> Vec<u8> {
        let name = art.contracts.keys().next().unwrap().clone();
        let mut script = encode_contract_entry_sig_script(art, &name, entry, args).expect("encode entry");
        script.extend(ScriptBuilder::with_flags(EngineFlags::default()).add_data(&contract::bytecode(art)).unwrap().drain());
        script
    }

    /// Version-1 inputs commit to a compute budget, and the signature covers it.
    /// So: sign with budget 0, measure the real script cost in the engine, set the
    /// covering budget, sign again, then prove the final transaction passes the
    /// engine under its own budget before it goes anywhere near the node.
    pub fn try_sign_and_budget(&self, tx: &mut Transaction, entries: &[UtxoEntry], sigscript: &dyn Fn(usize, &Transaction) -> Vec<u8>) -> Result<(), String> {
        for pass in 0..2 {
            for input in tx.inputs.iter_mut() {
                input.signature_script.clear();
            }
            let scripts: Vec<Vec<u8>> = (0..tx.inputs.len()).map(|i| sigscript(i, tx)).collect();
            for (input, script) in tx.inputs.iter_mut().zip(scripts) {
                input.signature_script = script;
            }
            tx.finalize();
            let budgets = {
                let cache = Cache::new(1_000);
                let reused = SigHashReusedValuesUnsync::new();
                let pop = PopulatedTransaction::new(tx, entries.to_vec());
                let cov = CovenantsContext::from_tx(&pop).map_err(|e| format!("covenant context: {e:?}"))?;
                (0..tx.inputs.len())
                    .map(|i| {
                        let input = &tx.inputs[i];
                        let limit = if pass == 0 { ScriptUnits(u64::MAX) } else { input.compute_commit.allowed_script_units() };
                        let mut vm = TxScriptEngine::from_transaction_input_with_script_units_limit(
                            &pop,
                            input,
                            i,
                            pop.utxo(i).unwrap(),
                            EngineCtx::new(&cache).with_reused(&reused).with_covenants_ctx(&cov),
                            EngineFlags::default(),
                            limit,
                        );
                        let units = match vm.execute() {
                            Ok(()) => vm.used_script_units(),
                            Err(e) if self.force => {
                                println!("preflight: the engine REJECTS input {i} (pass {pass}): {e:?}; --force sends it anyway with a doubled budget");
                                ScriptUnits(vm.used_script_units().0 * 2)
                            }
                            Err(e) => return Err(format!("preflight: input {i} fails the script engine (pass {pass}): {e:?}")),
                        };
                        ComputeBudget::checked_covering_script_units(units).ok_or_else(|| {
                            "This transaction is too large for the network to check. \
                             No funds were moved."
                                .to_string()
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?
            };
            if pass == 0 {
                for (input, budget) in tx.inputs.iter_mut().zip(budgets) {
                    input.compute_commit = ComputeCommit::from(budget);
                }
            } else {
                println!("preflight ok, compute budgets {:?}", budgets.iter().map(|b| b.0).collect::<Vec<_>>());
            }
        }
        Ok(())
    }

    /// Relay fee the node will demand: mass (compute, transient or storage, whichever is
    /// largest) times the fee rate, plus ten percent. The one mass formula in the client.
    pub fn required_fee(&self, tx: &Transaction, entries: &[UtxoEntry]) -> u64 {
        fee_at(self.mass(tx, entries), fee_rate())
    }

    /// The mass the node charges for: compute, transient or storage, whichever is largest.
    fn mass(&self, tx: &Transaction, entries: &[UtxoEntry]) -> u64 {
        let calc = MassCalculator::new(1, 10, STORAGE_MASS_PARAMETER);
        let nc = calc.calc_non_contextual_masses(tx);
        let pop = PopulatedTransaction::new(tx, entries.to_vec());
        let storage = calc.calc_contextual_masses(&pop).map(|m| m.storage_mass).unwrap_or(0);
        nc.compute_mass.max(nc.transient_mass).max(storage)
    }

    /// What a plain send (no ceiling of its own) may pay for `mass`: PLAIN_FEE_CEILING, or
    /// PLAIN_FEE_MULTIPLE times its cost at the network floor, whichever is larger.
    fn plain_ceiling(&self, mass: u64) -> u64 {
        let at_floor = fee_at(mass, self.network.fee_floor);
        PLAIN_FEE_CEILING.max(at_floor.saturating_mul(PLAIN_FEE_MULTIPLE))
    }

    /// Prices a move without sending it: builds with a guessed fee, measures the mass, rebuilds until
    /// the fee covers it, and refuses a fee above `ceiling` (sompi) before anything is signed for real.
    /// The builder may sign in memory; nothing reaches the node. This is the one fee loop: the commands
    /// send what it returns, the service quotes through it. No ceiling (a plain send) means
    /// `plain_ceiling`: every move has one.
    pub fn quote(&self, entries: &[UtxoEntry], ceiling: Option<u64>, build: &dyn Fn(u64) -> Result<Transaction, String>) -> Result<Quote, FeeError> {
        self.quote_at(fee_rate(), entries, ceiling, build)
    }

    /// `quote` at `rate` sompi per gram.
    fn quote_at(
        &self,
        rate: u64,
        entries: &[UtxoEntry],
        ceiling: Option<u64>,
        build: &dyn Fn(u64) -> Result<Transaction, String>,
    ) -> Result<Quote, FeeError> {
        // Every ceiling in use is above FEE_GUESS, so the settled fee never overshoots a ceiling.
        let mut fee = FEE_GUESS;
        for _ in 0..8 {
            let tx = build(fee).map_err(FeeError::Build)?;
            let mass = self.mass(&tx, entries);
            let need = fee_at(mass, rate);
            if let Some(c) = ceiling
                && need > c {
                    return Err(FeeError::OverCeiling { need, ceiling: c });
                }
            if ceiling.is_none() && need > self.plain_ceiling(mass) {
                return Err(FeeError::TooHigh);
            }
            if fee >= need {
                return Ok(Quote { tx, fee });
            }
            fee = need;
        }
        Err(FeeError::NotSettled)
    }

    /// `quote` for the commands that go on to send: the transaction and its fee.
    pub fn try_with_fee(&self, entries: &[UtxoEntry], ceiling: Option<u64>, build: &dyn Fn(u64) -> Result<Transaction, String>) -> Result<(Transaction, u64), FeeError> {
        self.quote(entries, ceiling, build).map(|q| (q.tx, q.fee))
    }

    /// Sends `tx`. A failed answer is judged by its class, not by what some node holds now (the phone's Resolver
    /// may reconnect to a lagging node): only the node's own rule rejection proves nothing moved. A duplicate
    /// rejection or a lost answer (timeout, closed socket, anything unknown) is a success when the tx sits in our
    /// node's mempool and ANSWER_LOST_MARK (the money may have moved) otherwise. A request that never left an
    /// offline phone reads as the network being down.
    pub async fn try_submit(&self, tx: &Transaction, label: &str) -> Result<String, String> {
        let first = match tokio::time::timeout(SUBMIT_TIMEOUT, self.rpc().submit_transaction(tx.into(), false)).await {
            Ok(Ok(id)) => {
                println!("{label}: accepted into mempool, tx {id}");
                return Ok(id.to_string());
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => format!("no answer to the submit in {} s", SUBMIT_TIMEOUT.as_secs()),
        };
        let id = tx.id();
        let class = classify_submit_error(&first);
        let looks = mempool_looks(class);
        let in_mempool = looks > 0 && self.in_our_mempool(id, looks).await;
        match submit_outcome(class, in_mempool) {
            SubmitOutcome::Sent => {
                println!("{label}: accepted into mempool, tx {id} (the node's first answer was lost: {first})");
                Ok(id.to_string())
            }
            SubmitOutcome::Refused => Err(format!("{label} rejected by node: {first}")),
            SubmitOutcome::NotSent => Err(format!("{label} not sent. Can't reach the network right now: {first}")),
            SubmitOutcome::Lost(why) => {
                println!("{label}: {ANSWER_LOST_MARK}, tx {id} may have moved: {first}; {why}");
                Err(format!("{label} {ANSWER_LOST_MARK}: tx {id}: {first}; then {why}"))
            }
        }
    }

    /// Up to `looks` looks (at most SUBMIT_CHECK_TRIES), SUBMIT_CHECK_GAP apart, for `id` in our node's mempool,
    /// orphans left out (an orphan is not a sent payment). Each look gives up after SUBMIT_CHECK_TIMEOUT, so a
    /// sick node cannot hold the money lock for long.
    async fn in_our_mempool(&self, id: kaspa_consensus_core::tx::TransactionId, looks: u32) -> bool {
        for attempt in 0..looks.min(SUBMIT_CHECK_TRIES) {
            if attempt > 0 {
                tokio::time::sleep(SUBMIT_CHECK_GAP).await;
            }
            let look = tokio::time::timeout(SUBMIT_CHECK_TIMEOUT, self.rpc().get_mempool_entry(id, false, false));
            if matches!(look.await, Ok(Ok(_))) {
                return true;
            }
        }
        false
    }

    /// Spends the vault coin at input 0 through `entry`, with the given outputs. `signer` is None for `finish`.
    /// The payload echoes `lock_time` (see `PAYLOAD_TAG`).
    #[allow(clippy::too_many_arguments)]
    pub fn vault_tx(
        &self,
        v: &Vault,
        coin: &Coin,
        entry: &str,
        signer: Option<&Keypair>,
        extra_args: &[V],
        outputs: Vec<TransactionOutput>,
        sequence: u64,
        lock_time: u64,
    ) -> Result<Transaction, String> {
        self.vault_tx_lifted(v, coin, None, entry, signer, extra_args, outputs, sequence, lock_time)
    }

    /// `vault_tx` with one plain coin of the funding key as input 1: the contract's L6 lift, for a
    /// vault too small to pay its own way out (cancel, inherit). `lift` is the coin and its key.
    #[allow(clippy::too_many_arguments)]
    pub fn vault_tx_lifted(
        &self,
        v: &Vault,
        coin: &Coin,
        lift: Option<(&Coin, &Keypair)>,
        entry: &str,
        signer: Option<&Keypair>,
        extra_args: &[V],
        outputs: Vec<TransactionOutput>,
        sequence: u64,
        lock_time: u64,
    ) -> Result<Transaction, String> {
        let art = self.try_compile(&v.state)?;
        let mut inputs = vec![TransactionInput::new_with_compute_budget(coin.0, vec![], sequence, 0)];
        let mut entries = vec![coin.1.clone()];
        if let Some((c, _)) = lift {
            inputs.push(TransactionInput::new_with_compute_budget(c.0, vec![], 0, 0));
            entries.push(c.1.clone());
        }
        // The payload is set before signing: the signature commits to it, and the quote that
        // measures this transaction's mass is measuring the bytes that are sent.
        let payload = lock_time_payload(lock_time);
        let native = SUBNETWORK_ID_NATIVE;
        let mut tx = Transaction::new(1, inputs, outputs, lock_time, native, 0, payload);
        self.try_sign_and_budget(&mut tx, &entries, &|i, t| {
            if i > 0 {
                return self.plain_sigscript(t, &entries, i, lift.expect("a lifted input has a key").1);
            }
            let mut args = vec![];
            if let Some(k) = signer {
                args.push(V::Bytes(self.sign_input(t, &entries, 0, k)));
            }
            args.extend(extra_args.iter().cloned());
            self.vault_sigscript(&art, entry, &args)
        })?;
        Ok(tx)
    }

    pub fn continuation(&self, value: u64, next: &State, v: &Vault) -> TransactionOutput {
        TransactionOutput::with_covenant(value, self.vault_spk(next), Some(CovenantBinding::new(0, v.covenant_id)))
    }

    /// A plain one-signer transaction (funding key): `coins` in, `outputs` out. With `genesis`,
    /// output 0 opens a new covenant lineage (its id is the new vault's covenant id).
    pub fn plain_tx(&self, coins: &[Coin], outputs: Vec<TransactionOutput>, k: &Keypair, genesis: bool) -> Result<Transaction, String> {
        let entries: Vec<UtxoEntry> = coins.iter().map(|(_, u)| u.clone()).collect();
        let inputs = coins.iter().map(|(op, _)| TransactionInput::new_with_compute_budget(*op, vec![], 0, 0)).collect();
        let mut tx = Transaction::new(1, inputs, outputs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
        if genesis {
            tx.populate_genesis_covenants(&[GenesisCovenantGroup::new(0, vec![0])]).map_err(|e| format!("genesis covenant: {e:?}"))?;
        }
        self.try_sign_and_budget(&mut tx, &entries, &|i, t| self.plain_sigscript(t, &entries, i, k))?;
        Ok(tx)
    }
}

/// The safe coins `stop_from_elsewhere` asks the node about: newest first, only the `want` tx's
/// coins when given (before the cap, so newer coins cannot push the real stop out), at most 8.
fn stop_candidates(mut coins: Vec<Coin>, want: Option<&str>) -> Vec<Coin> {
    coins.retain(|(o, _)| want.is_none_or(|w| o.transaction_id.to_string() == w));
    coins.sort_by_key(|(_, u)| std::cmp::Reverse(u.block_daa_score));
    coins.truncate(8);
    coins
}

/// Marks a submit whose answer was lost and whose money may have moved (core `plain` shows ANSWER_LOST).
pub(crate) const ANSWER_LOST_MARK: &str = "submit answer lost";
const SUBMIT_CHECK_TRIES: u32 = 3;
const SUBMIT_CHECK_GAP: std::time::Duration = std::time::Duration::from_secs(2);
const SUBMIT_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// The submit itself: the library's own 60 s would hold the money lock too long; a timeout reads as Unknown.
const SUBMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The class of a failed submit, read from its error text as kaspa-wrpc-client 2.1.0 (Borsh) words it: every
/// client error comes back as "RPC Server (remote error) -> " + the workflow-rpc client error, and a reply the
/// node sent carries "RPC response error". Anything not proven otherwise is Unknown (the safe side).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SubmitClass {
    /// The node ran its rules on the tx and refused it (RpcError::RejectedTransaction): it entered no mempool.
    Rule,
    /// The node refused it as one it already holds or has accepted: it may well have been sent.
    Duplicate,
    /// The socket was down before the request left (WebSocketError::NotConnected): nothing was sent.
    NotSent,
    /// A timeout, a closed socket or anything else: the node may have taken it.
    Unknown,
}

#[derive(Debug, PartialEq)]
pub(crate) enum SubmitOutcome {
    /// The node holds the transaction: it was sent.
    Sent,
    /// The node's own rules refused it: nothing moved.
    Refused,
    /// The request never left the phone: nothing moved, the network is down.
    NotSent,
    /// The money may have moved (why, for the log).
    Lost(String),
}

pub(crate) fn classify_submit_error(e: &str) -> SubmitClass {
    // "already spent by transaction .. in the mempool": a tx spending our coin (often our own first try) is there.
    const DUPLICATE: [&str; 4] =
        ["is already in the mempool", "was already accepted by the consensus", "is already in the orphan pool", "in the mempool"];
    let node_said = e.contains("RPC response error");
    if node_said && e.contains("Rejected transaction ") {
        if DUPLICATE.iter().any(|d| e.contains(d)) {
            SubmitClass::Duplicate
        } else {
            SubmitClass::Rule
        }
    } else if !node_said && e.contains("WebSocket -> WebSocket is not connected") {
        SubmitClass::NotSent
    } else {
        SubmitClass::Unknown
    }
}

/// How many mempool looks a failed submit of `class` earns: none where the class alone decides.
pub(crate) fn mempool_looks(class: SubmitClass) -> u32 {
    match class {
        SubmitClass::Rule | SubmitClass::NotSent => 0,
        SubmitClass::Duplicate => 1,
        SubmitClass::Unknown => SUBMIT_CHECK_TRIES,
    }
}

/// What a failed submit means, from its class and whether our node's mempool holds the tx.
pub(crate) fn submit_outcome(class: SubmitClass, in_mempool: bool) -> SubmitOutcome {
    match (class, in_mempool) {
        (SubmitClass::Rule, _) => SubmitOutcome::Refused,
        (SubmitClass::NotSent, _) => SubmitOutcome::NotSent,
        (_, true) => SubmitOutcome::Sent,
        (SubmitClass::Duplicate, false) => SubmitOutcome::Lost("the node says it has the tx but its mempool does not show it".into()),
        (SubmitClass::Unknown, false) => SubmitOutcome::Lost("the tx is not in our node's mempool".into()),
    }
}

/// An RPC utxo entry as the (outpoint, entry) pair the rest of the client works with.
pub fn coin_of(e: RpcUtxosByAddressesEntry) -> Coin {
    let u = e.utxo_entry;
    (TransactionOutpoint::new(e.outpoint.transaction_id, e.outpoint.index), UtxoEntry::new(u.amount, u.script_public_key, u.block_daa_score, u.is_coinbase, u.covenant_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Error texts as kaspa-wrpc-client 2.1.0 (Borsh) words them.
    const RULE: &str = "RPC Server (remote error) -> RPC response error Text(\"Rejected transaction ab12: one of the transaction sequence locks conditions was not met\")";
    const DUP: &str = "RPC Server (remote error) -> RPC response error Text(\"Rejected transaction ab12: transaction ab12 is already in the mempool\")";
    const MINED: &str = "RPC Server (remote error) -> RPC response error Text(\"Rejected transaction ab12: transaction ab12 was already accepted by the consensus\")";
    const OFFLINE: &str = "RPC Server (remote error) -> WebSocket -> WebSocket is not connected";
    const SPENT: &str = "RPC Server (remote error) -> RPC response error Text(\"Rejected transaction cd34: output ab12:0 already spent by transaction ab12 in the mempool\")";
    const LOST: [&str; 5] = [
        "no answer to the submit in 20 s",
        "RPC Server (remote error) -> RPC request timeout",
        "RPC Server (remote error) -> WebSocket disconnected",
        "RPC Server (remote error) -> RPC response error Close",
        "Rejected transaction ab12: not from the node",
    ];

    #[test]
    fn a_failed_submit_is_judged_by_its_error_class() {
        use SubmitClass::*;
        assert_eq!([RULE, DUP, MINED, OFFLINE, SPENT].map(classify_submit_error), [Rule, Duplicate, Duplicate, NotSent, Duplicate]);
        for e in LOST {
            assert_eq!(classify_submit_error(e), Unknown, "{e}");
        }
        assert_eq!([Rule, NotSent, Duplicate, Unknown].map(mempool_looks), [0, 0, 1, SUBMIT_CHECK_TRIES]);
        for hit in [false, true] {
            assert_eq!(submit_outcome(Rule, hit), SubmitOutcome::Refused, "a rule rejection never waits");
            assert_eq!(submit_outcome(NotSent, hit), SubmitOutcome::NotSent);
        }
        assert_eq!(submit_outcome(Duplicate, true), SubmitOutcome::Sent);
        assert_eq!(submit_outcome(Unknown, true), SubmitOutcome::Sent);
        assert!(matches!(submit_outcome(Duplicate, false), SubmitOutcome::Lost(_)));
        assert!(matches!(submit_outcome(Unknown, false), SubmitOutcome::Lost(_)), "a lost answer is never 'nothing moved'");
    }
    use crate::keys::{keypair, xonly};

    /// A sibling account's old stop coin (same safe, newer or not) never stands in for the wanted
    /// stop.
    #[test]
    fn stop_candidates_keep_only_the_wanted_stop() {
        let coin = |b: u8, daa: u64| {
            let op = TransactionOutpoint::new(kaspa_consensus_core::Hash::from_bytes([b; 32]), 0);
            (op, UtxoEntry::new(5, ScriptPublicKey::from_vec(0, vec![]), daa, false, None))
        };
        let ours = coin(1, 10).0.transaction_id.to_string();
        let mut coins: Vec<Coin> = (2..12).map(|b| coin(b, 100 + u64::from(b))).collect();
        coins.push(coin(1, 10));
        let picked = stop_candidates(coins.clone(), Some(&ours));
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].0.transaction_id.to_string(), ours);
        assert!(stop_candidates(coins.clone(), Some(&"ee".repeat(32))).is_empty());
        let any = stop_candidates(coins, None);
        assert_eq!(any.len(), 8);
        assert_eq!(any[0].1.block_daa_score, 111);
    }

    #[test]
    fn only_secure_node_urls_pass() {
        let good = ["wss://node.example:443", " WSS://n.example/path ", "ws://127.0.0.1:17210",
            "ws://[::1]:17110/"];
        for ok in good {
            assert!(check_secure_url(ok).is_ok(), "{ok}");
        }
        let bad = ["ws://node.example:17210", "node.example", "wss://", "http://a",
            "ws://127.0.0.1.evil.example:1", "ws://localhost@evil.example", "ws://localhost", "ws://localhost:17110", "ws://127.0.0.1:1x",
            "ws://[::1]evil", ""];
        for b in bad {
            assert_eq!(check_secure_url(b).unwrap_err(), INSECURE_NODE, "{b}");
        }
    }

    #[test]
    fn resolver_asks_for_tls_nodes_on_the_public_list() {
        let r = secure_resolver();
        assert!(r.tls(), "the resolver must ask for wss nodes only");
        assert!(r.urls().is_none(), "the public resolver list");
    }

    /// A plain ws:// node is refused before any socket opens, in plain words.
    #[tokio::test]
    async fn connect_refuses_plain_ws_node() {
        let err = connect(Some("ws://192.0.2.1:17210"), &Network::testnet()).await.unwrap_err();
        assert_eq!(err, INSECURE_NODE);
        let err = connect(Some("192.0.2.1:17210"), &Network::testnet()).await.unwrap_err();
        assert_eq!(err, INSECURE_NODE);
    }
    use kaspa_consensus_core::Hash;
    use kaspa_consensus_core::tx::ScriptPublicKey;
    use kaspa_txscript::pay_to_address_script;

    #[test]
    fn state_json_for_another_network_is_refused() {
        let dir = std::env::temp_dir().join("kv-state-net");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = ctx();
        c.dir = dir.clone();
        let v = Vault { covenant_id: Default::default(), state: contract::locked(), pending_to: None, history: vec![], contract_sha256: None, network: "mainnet".into(), dials: Dials::testnet() };
        std::fs::write(dir.join("state.json"), serde_json::to_string(&v).unwrap()).unwrap();
        let err = c.try_load_vault().unwrap_err();
        assert!(err.contains("state.json was made for the mainnet network") && err.contains("Open it on mainnet"), "{err}");
        c.network = Network::mainnet();
        assert_eq!(c.try_load_vault().unwrap().unwrap().network, "mainnet");
        // a vault file from before the word loads on testnet and is refused on mainnet
        std::fs::write(dir.join("state.json"), serde_json::to_string(&Vault { network: String::new(), ..v }).unwrap()).unwrap();
        assert!(c.try_load_vault().unwrap_err().contains("does not say which network"));
        c.network = Network::testnet();
        assert!(c.try_load_vault().unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn ctx() -> Ctx {
        Ctx { dir: std::env::temp_dir(), keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 }
    }
    /// One 100 KAS coin at the funding address, its key and its script.
    fn coin(ctx: &Ctx) -> (Coin, Keypair, ScriptPublicKey) {
        let k = keypair(ctx.keys.funding.as_ref().unwrap());
        let spk = pay_to_address_script(&Network::testnet().p2pk(&xonly(&k)));
        ((TransactionOutpoint::new(Hash::from_bytes([1u8; 32]), 0), UtxoEntry::new(100 * KAS, spk.clone(), 100, false, None)), k, spk)
    }
    /// A plain payment of `amount` with change back, as fund/pay build it.
    fn pay(ctx: &Ctx, c: &Coin, k: &Keypair, change_spk: &ScriptPublicKey, amount: u64, fee: u64) -> Result<Transaction, String> {
        let to = pay_to_address_script(&Network::testnet().p2pk(&xonly(&keypair(&[7u8; 32]))));
        let change = c.1.amount.checked_sub(amount + fee).ok_or("the coin cannot pay the fee")?;
        ctx.plain_tx(&[c.clone()], vec![TransactionOutput::new(amount, to), TransactionOutput::new(change, change_spk.clone())], k, false)
    }

    #[test]
    fn quote_is_the_fee_the_signed_transaction_pays() {
        let ctx = ctx();
        let (c, k, spk) = coin(&ctx);
        let q = ctx.quote(&[c.1.clone()], Some(MAX_FEE as u64), &|fee| pay(&ctx, &c, &k, &spk, KAS, fee)).unwrap();
        let paid = c.1.amount - q.tx.outputs.iter().map(|o| o.value).sum::<u64>();
        assert_eq!(paid, q.fee);
        assert!(q.fee >= ctx.required_fee(&q.tx, &[c.1.clone()]) && q.fee <= MAX_FEE as u64, "fee {}", q.fee);
    }

    /// A Locked vault and its 50 KAS coin, accepted at DAA 1,000.
    fn vault_and_coin(ctx: &Ctx) -> (Vault, Coin) {
        let v = Vault {
            covenant_id: Hash::from_bytes([1u8; 32]),
            state: contract::locked(),
            pending_to: None,
            history: vec![],
            contract_sha256: None,
            network: String::new(),
            dials: Dials::testnet(),
        };
        let spk = ctx.vault_spk(&v.state);
        let entry = UtxoEntry::new(50 * KAS, spk, 1_000, false, Some(v.covenant_id));
        (v, (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), entry))
    }

    /// A lock time late enough that a spend opens a fresh window (window_start = lock time).
    fn fresh_window(v: &Vault) -> u64 {
        1_000 + v.dials.window as u64 + 7
    }

    /// A half-KAS spend (the testnet cap is 1 KAS) at `lock_time` that opens a fresh window,
    /// as `do_spend` builds it.
    fn vault_spend(ctx: &Ctx, v: &Vault, c: &Coin, lock_time: u64, fee: u64)
    -> Result<Transaction, String> {
        let to = pay_to_address_script(&Network::testnet().p2pk(&xonly(&keypair(&[7u8; 32]))));
        let amount = KAS / 2;
        let spent = (amount + fee) as i64;
        let next = State { window_start: lock_time as i64, spent, ..v.state.clone() };
        let left = c.1.amount.checked_sub(amount + fee).ok_or("the vault cannot pay the fee")?;
        let outputs = vec![TransactionOutput::new(amount, to), ctx.continuation(left, &next, v)];
        let k = keypair(ctx.keys.spend.as_ref().unwrap());
        ctx.vault_tx(v, c, "spend", Some(&k), &[], outputs, 0, lock_time)
    }

    #[test]
    fn a_vault_spend_payload_echoes_its_lock_time() {
        let ctx = ctx();
        let (v, c) = vault_and_coin(&ctx);
        let lock_time = fresh_window(&v);
        let tx = vault_spend(&ctx, &v, &c, lock_time, 20_000).unwrap();
        assert_eq!(tx.lock_time, lock_time);
        assert_eq!(tx.payload.len(), PAYLOAD_TAG.len() + 8, "tag and lock time, nothing else");
        assert_eq!(lock_time_from_payload(&tx.payload), Some(lock_time));
        // a stop (lock time 0) carries the same format
        let key = ctx.keys.try_stop_key(0).unwrap();
        let out = vec![TransactionOutput::new(c.1.amount - 20_000, ctx.cold_spk())];
        let stop = ctx.vault_tx(&v, &c, "cancel", Some(&key), &[V::Int(0)], out, 0, 0).unwrap();
        assert_eq!(lock_time_from_payload(&stop.payload), Some(0));
    }

    #[test]
    fn an_outside_safe_takes_the_stop_and_the_old_safe_is_refused() {
        let mut ctx = ctx();
        let old_safe = ctx.cold_spk();
        let outside = Network::testnet().p2pk(&xonly(&keypair(&[42u8; 32]))).to_string();
        ctx.keys.set_cold_external(&outside).unwrap();
        let want = pay_to_address_script(&Network::testnet().address(&outside).unwrap());
        assert_eq!(ctx.cold_spk(), want);
        let (v, c) = vault_and_coin(&ctx);
        let key = ctx.keys.try_stop_key(0).unwrap();
        // the emergency stop (and so the challenge's cap-cancel and final sweep) pays the outside address, checked in the engine
        let out = vec![TransactionOutput::new(c.1.amount - 20_000, ctx.cold_spk())];
        let stop = ctx.vault_tx(&v, &c, "cancel", Some(&key), &[V::Int(0)], out, 0, 0).unwrap();
        assert_eq!(stop.outputs[0].script_public_key, want);
        // the cold sheets' old safe, or any other address, fails the covenant
        for to in [old_safe, pay_to_address_script(&Network::testnet().p2pk(&xonly(&keypair(&[43u8; 32]))))] {
            let out = vec![TransactionOutput::new(c.1.amount - 20_000, to)];
            let err = ctx.vault_tx(&v, &c, "cancel", Some(&key), &[V::Int(0)], out, 0, 0).unwrap_err();
            assert!(err.contains("fails the script engine"), "{err}");
        }
    }

    #[test]
    fn a_payload_that_is_not_ours_reads_as_none() {
        assert_eq!(lock_time_from_payload(&lock_time_payload(u64::MAX)), Some(u64::MAX));
        let ours = lock_time_payload(42);
        let mut long = ours.clone();
        long.push(0);
        let mut other = ours.clone();
        other[2] = b'2';
        for bad in [vec![], ours[..10].to_vec(), long, other] {
            assert_eq!(lock_time_from_payload(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_vault_spend_quote_has_the_mass_of_the_signed_transaction() {
        let ctx = ctx();
        let (v, c) = vault_and_coin(&ctx);
        let entries = [c.1.clone()];
        let lock_time = fresh_window(&v);
        let build = |fee| vault_spend(&ctx, &v, &c, lock_time, fee);
        let q = ctx.quote(&entries, Some(MAX_FEE as u64), &build).unwrap();
        let signed = build(q.fee).unwrap();
        assert_eq!(signed.payload, q.tx.payload);
        assert_eq!(ctx.mass(&q.tx, &entries), ctx.mass(&signed, &entries));
        assert!(q.fee >= ctx.required_fee(&signed, &entries), "quote pays the signed bytes");
        // the payload is priced: its bytes are in the compute mass the quote measured
        let calc = MassCalculator::new(1, 10, STORAGE_MASS_PARAMETER);
        let mut bare = signed.clone();
        bare.payload.clear();
        let compute = |t: &Transaction| calc.calc_non_contextual_masses(t).compute_mass;
        assert_eq!(compute(&signed), compute(&bare) + signed.payload.len() as u64);
    }

    #[test]
    fn fee_ceiling_refuses_a_dust_payment() {
        // a 0.001 KAS payee output has a storage mass near 1e7 grams: about 11 KAS of fee
        let ctx = ctx();
        let (c, k, spk) = coin(&ctx);
        let dust = KAS / 1000;
        let ceiling = (MAX_FEE as u64).max(dust / 10);
        match ctx.quote(&[c.1.clone()], Some(ceiling), &|fee| pay(&ctx, &c, &k, &spk, dust, fee)) {
            Err(FeeError::OverCeiling { need, ceiling: c }) => {
                assert_eq!(c, MAX_FEE as u64);
                assert!(need > KAS, "need {need}");
            }
            Err(e) => panic!("expected OverCeiling, got {e}"),
            Ok(q) => panic!("expected OverCeiling, got a quote of {} sompi", q.fee),
        }
        // with no ceiling the same move settles, at a fee above the payment itself
        let q = ctx.quote(&[c.1.clone()], None, &|fee| pay(&ctx, &c, &k, &spk, dust, fee)).unwrap();
        assert!(q.fee > KAS, "fee {}", q.fee);
        assert_eq!(c.1.amount - q.tx.outputs.iter().map(|o| o.value).sum::<u64>(), q.fee);
    }

    #[test]
    fn node_fee_rate_is_clamped_and_fees_never_wrap() {
        let net = Network::testnet();
        let (floor, cap) = (net.fee_floor, FEE_RATE_MULTIPLE * net.fee_floor);
        assert_eq!(node_fee_rate(1e12, &net), cap, "a node naming a huge rate is held to the cap");
        assert_eq!(node_fee_rate(f64::INFINITY, &net), cap);
        assert_eq!(node_fee_rate(f64::NAN, &net), floor, "not a number reads as the floor");
        assert_eq!(node_fee_rate(-5.0, &net), floor);
        assert_eq!(node_fee_rate(floor as f64 * 2.5, &net), floor * 5 / 2);
        assert_eq!(clamp_fee_rate(u64::MAX, &net), cap);
        assert_eq!(fee_at(u64::MAX, 2), u64::MAX / 10, "saturates instead of wrapping");
        assert_eq!(fee_at(1_000, floor), 1_100 * floor);
    }

    #[test]
    fn plain_send_refuses_a_fee_far_above_the_floor() {
        // a 1 KAS payment from a 100 KAS coin: about 0.011 KAS at the floor
        let ctx = ctx();
        let (c, k, spk) = coin(&ctx);
        let floor = ctx.network.fee_floor;
        let one = |fee: u64| pay(&ctx, &c, &k, &spk, KAS, fee);
        let q = ctx.quote_at(floor, &[c.1.clone()], None, &one).unwrap();
        assert!(q.fee < PLAIN_FEE_CEILING, "fee {}", q.fee);
        let top = FEE_RATE_MULTIPLE * floor;
        match ctx.quote_at(top, &[c.1.clone()], None, &one) {
            Err(e) => assert_eq!(e.to_string(), FEE_TOO_HIGH),
            Ok(q) => panic!("expected a refusal, got a quote of {} sompi", q.fee),
        }
        // a vault move keeps its own dial ceiling and words
        let ceiling = Some(MAX_FEE as u64);
        let r = ctx.quote_at(top, &[c.1.clone()], ceiling, &one);
        assert!(matches!(r, Err(FeeError::OverCeiling { .. })));
    }

    #[test]
    fn fee_rate_is_the_floor_without_a_node() {
        assert_eq!(fee_rate(), Network::testnet().fee_floor);
        assert_eq!(VAULT_FLOOR, 105_000_000);
        assert_eq!(VAULT_FLOOR, Dials::testnet().floor());
    }

    #[test]
    fn the_clock_check_allows_a_fifth_and_refuses_more() {
        assert!(check_daa_rate(10.0, 10.0, RATE_TOLERANCE).is_ok());
        assert!(check_daa_rate(8.0, 10.0, RATE_TOLERANCE).is_ok() && check_daa_rate(12.0, 10.0, RATE_TOLERANCE).is_ok());
        let e = check_daa_rate(7.9, 10.0, RATE_TOLERANCE).unwrap_err();
        assert!(e.contains("7.90 blocks a second") && e.contains("within 20 percent"), "{e}");
        assert!(check_daa_rate(1.0, 10.0, RATE_TOLERANCE).is_err());
        assert!(check_daa_rate(f64::NAN, 10.0, RATE_TOLERANCE).is_err());
    }

    #[test]
    fn the_vault_dials_win_over_the_preset_once_a_vault_exists() {
        let dir = std::env::temp_dir().join(format!("kv-dials-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = Ctx { dir: dir.clone(), keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::mainnet(10.0), account: 1 };
        assert_eq!(ctx.dials().unwrap(), Dials::mainnet(10.0), "no vault yet: the fund dials");
        let v = Vault { covenant_id: Default::default(), state: contract::locked(), pending_to: None, history: vec![], contract_sha256: None, network: String::new(), dials: Dials::testnet() };
        std::fs::write(dir.join("state.json"), serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(ctx.dials().unwrap(), Dials::testnet(), "an open vault keeps the dials it was funded with");
        std::fs::remove_dir_all(&dir).ok();
    }
}
