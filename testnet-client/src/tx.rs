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
/// The fee rate in use, sompi per gram: the one named place. `use_fee_floor` sets the network's floor
/// at start; `connect` raises it to the node's estimate (`get_fee_estimate`, normal bucket), never below
/// the floor; the offline device keeps the floor.
static FEE_RATE: AtomicU64 = AtomicU64::new(crate::network::TESTNET_FEE_FLOOR);

pub fn fee_rate() -> u64 {
    FEE_RATE.load(Ordering::Relaxed)
}
/// Starts the fee rate at this network's floor (main, before any connect).
pub fn use_fee_floor(net: &Network) {
    FEE_RATE.store(net.fee_floor, Ordering::Relaxed);
}
/// The offline device takes the fee floor the online device measured and wrote into the signing
/// request (item 5), never below its own network's floor.
pub fn set_fee_rate(rate: u64, net: &Network) {
    FEE_RATE.store(rate.max(net.fee_floor), Ordering::Relaxed);
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
        return Err("the two block headers carry the same time".into());
    }
    Ok((sink.header.daa_score as f64 - root.header.daa_score as f64) / secs)
}
/// The measured clock must sit within `tolerance` of the rate the dials were made with; otherwise every
/// day-long dial would be the wrong length.
pub fn check_daa_rate(measured: f64, expected: f64, tolerance: f64) -> Result<(), String> {
    if !measured.is_finite() || (measured - expected).abs() > expected * tolerance {
        return Err(format!(
            "the network clock runs at about {measured:.2} DAA per second; this program expects {expected:.1} (within {:.0} percent), so the time dials would be the wrong length. Check the node, or wait and try again.",
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
    Build(String),
}

impl std::fmt::Display for FeeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeeError::OverCeiling { need, ceiling } => write!(f, "the network fee would be {:.5} KAS, more than the {} KAS this move allows", kas(*need), kas(*ceiling)),
            FeeError::NotSettled => write!(f, "The network charge could not be worked out. Try again in a moment."),
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

/// Connects to the folder's network: a pinned node by wRPC url, or a public one through the resolver.
/// A node that reports another network is refused: it is lying or set up wrong.
pub async fn connect(url: Option<&str>, net: &Network) -> Result<KaspaRpcClient, String> {
    let resolver = url.is_none().then(Resolver::default);
    let where_ = url.map(str::to_string).unwrap_or_else(|| format!("a public {} node", net.id));
    let rpc = KaspaRpcClient::new(WrpcEncoding::Borsh, url, resolver, Some(net.resolver_id()), None).map_err(|e| format!("Can't set up the connection to {where_}: {e}"))?;
    rpc.connect(Some(ConnectOptions { block_async_connect: true, strategy: ConnectStrategy::Fallback, ..Default::default() }))
        .await
        .map_err(|e| format!("Can't reach the network right now ({where_}): {e}"))?;
    let info = rpc.get_server_info().await.map_err(|e| format!("the node at {where_} did not say what it is: {e}"))?;
    println!("node: {} synced={} network={}", info.server_version, info.is_synced, info.network_id);
    net.check_node(&info.network_id, &where_)?;
    if !info.is_synced {
        return Err(format!("the node at {where_} is still catching up with the network. Wait for it, or use another node."));
    }
    match rpc.get_fee_estimate().await {
        Ok(est) => {
            let node = est.normal_buckets.first().map_or(est.priority_bucket.feerate, |b| b.feerate).ceil() as u64;
            FEE_RATE.store(node.max(net.fee_floor), Ordering::Relaxed);
            println!("fee rate: the node says {node} sompi/gram; using {} (floor {})", fee_rate(), net.fee_floor);
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
                return Err(format!("could not measure the network clock ({e}). Real money needs that check; use another node or try again."));
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
                n => format!("expected one vault coin, found {n}"),
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
                        ComputeBudget::checked_covering_script_units(units).ok_or_else(|| "budget does not fit u16".to_string())
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
        let calc = MassCalculator::new(1, 10, STORAGE_MASS_PARAMETER);
        let nc = calc.calc_non_contextual_masses(tx);
        let pop = PopulatedTransaction::new(tx, entries.to_vec());
        let storage = calc.calc_contextual_masses(&pop).map(|m| m.storage_mass).unwrap_or(0);
        let mass = nc.compute_mass.max(nc.transient_mass).max(storage);
        mass * fee_rate() * 11 / 10
    }

    /// Prices a move without sending it: builds with a guessed fee, measures the mass, rebuilds until
    /// the fee covers it, and refuses a fee above `ceiling` (sompi) before anything is signed for real.
    /// The builder may sign in memory; nothing reaches the node. This is the one fee loop: the commands
    /// send what it returns, the service quotes through it.
    pub fn quote(&self, entries: &[UtxoEntry], ceiling: Option<u64>, build: &dyn Fn(u64) -> Result<Transaction, String>) -> Result<Quote, FeeError> {
        // Every ceiling in use is above FEE_GUESS, so the settled fee never overshoots a ceiling.
        let mut fee = FEE_GUESS;
        for _ in 0..8 {
            let tx = build(fee).map_err(FeeError::Build)?;
            let need = self.required_fee(&tx, entries);
            if let Some(c) = ceiling
                && need > c {
                    return Err(FeeError::OverCeiling { need, ceiling: c });
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

    pub async fn try_submit(&self, tx: &Transaction, label: &str) -> Result<String, String> {
        let id = self.rpc().submit_transaction(tx.into(), false).await.map_err(|e| format!("{label} rejected by node: {e}"))?;
        println!("{label}: accepted into mempool, tx {id}");
        Ok(id.to_string())
    }

    /// Spends the vault coin at input 0 through `entry`, with the given outputs. `signer` is None for `finish`.
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
        let mut tx = Transaction::new(1, inputs, outputs, lock_time, SUBNETWORK_ID_NATIVE, 0, vec![]);
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

/// An RPC utxo entry as the (outpoint, entry) pair the rest of the client works with.
pub fn coin_of(e: RpcUtxosByAddressesEntry) -> Coin {
    let u = e.utxo_entry;
    (TransactionOutpoint::new(e.outpoint.transaction_id, e.outpoint.index), UtxoEntry::new(u.amount, u.script_public_key, u.block_daa_score, u.is_coinbase, u.covenant_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{keypair, xonly};
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
        assert!(err.contains("state.json was made for the mainnet network") && err.contains("--network mainnet"), "{err}");
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
        assert!(e.contains("7.90 DAA per second") && e.contains("within 20 percent"), "{e}");
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
