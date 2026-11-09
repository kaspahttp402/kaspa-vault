//! Lineage walk (docs/PAPER_STOP.md, ranking a'; docs/paper-stop-indexer.md): from the coin an old
//! watch code names to the account's live coin. The covenant address changes with every move, so
//! a record printed many payments ago names a coin that is long spent. The walk asks a history
//! source for the record's coin, then which transaction spent each coin, and follows that
//! transaction's continuation (the output carrying the same covenant id) until a coin is unspent
//! (live) or the account ended (stop, inheritance, a finish that emptied it).
//!
//! The source is untrusted (a public indexer). Every hop is checked here the way the follow-only
//! watcher checks a move (watch.rs `judge`): the input must spend the coin, through a known entry
//! of the account's own compiled rules, and the continuation script must equal the script
//! contract.rs `compile` gives for the next state, derived with contract.rs `after_*`. The script
//! commits to the whole state, so a match is a proof. A hop that does not check out stops the
//! walk. Read-only: nothing secret is sent, nothing is signed.
//!
//! What cannot be checked here: whether a transaction the source shows was ever accepted, and
//! its id (the indexer cannot give every field the id hashes). A made-up history can only end the
//! walk at a coin the node does not hold, so the caller must take the live coin's entry from the
//! node (its address is the walk's state compiled) before signing anything on it.

use crate::contract::{self, Coin, State, Vault, after_decline, after_finish, after_spend, spk_hash};
use crate::keys::Public;
use crate::network::Network;
use crate::watch::{Move, addr_of, follow_record};
use futures::future::BoxFuture;
use kaspa_addresses::Address;
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::tx::{ScriptPublicKey, Transaction, TransactionInput, TransactionOutpoint};
use kaspa_consensus_core::tx::{TransactionOutput, UtxoEntry};
use kaspa_txscript::pay_to_script_hash_script;
use crate::tx::lock_time_from_payload;
use silverscript_abi::SilAbiArtifact;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// What the user reads when a hop does not check out.
pub const CHECK_FAILED: &str = "The account's history could not be checked. No funds were moved.";
/// What the user reads when the history source cannot be read (its own text goes to the log).
pub const UNREADABLE: &str =
    "The account's history could not be read. Check the connection and try again. \
    No funds were moved.";
/// What the user reads when the source knows no coin of the record.
pub const NOT_FOUND: &str =
    "The account's coin could not be found in the history. No funds were moved.";
/// What the user reads for a record on rules other than the built-in ones.
pub const OLD_RULES: &str = "This account runs another version of the rules, which a paper stop \
     cannot follow. No funds were moved.";
/// The longest history a walk follows by default (payments, requests, finishes, top-ups...).
/// Top-ups keep the state, so padding collapses to one step wherever the source can show a newer
/// coin of the same state (`SpenderSource::coins_at`; the node shows the live one).
pub const MAX_HOPS: usize = 500;
/// The most time one window-opening spend may take whose lock time no hint gives: the search
/// tries the whole range the rules allow within it (`lock_search`).
pub const LOCK_BUDGET: Duration = Duration::from_secs(20);
/// The whole walk's time: every question to the source and every lock time search share it.
pub const WALK_TIME: Duration = Duration::from_secs(180); // a stop may wait; it must not give up
/// Questions one walk may ask its source (a hop asks one or two).
pub const WALK_CALLS: usize = 2 * MAX_HOPS + 4;
/// The walk ran out of its time or questions before it reached the live coin.
pub const TOO_LONG: &str =
    "The account's history is too long to follow in the time allowed. No funds were moved.";
const LONGER: &str = "The account's history is longer than";
/// Blocks our node is asked for its copy of a spend (for the real lock time).
const NODE_BLOCKS: usize = 2;
/// What the user reads while the source has a move it cannot vouch for yet.
pub const SETTLING: &str =
    "The account's latest move has not settled yet. Try again in a minute. No funds were moved.";
/// The most coins read from one `coins_at` answer, and the most threads one search uses.
const MAX_COINS: usize = 10_000;
const MAX_THREADS: usize = 8;
/// A search thread looks at the clock (and the other threads' hit) every this many tries.
const CLOCK_EVERY: usize = 4_096;
/// Why a hop failed (tests and logs only; the user reads `CHECK_FAILED`).
type Bad = &'static str;
/// Numbers a spend's window rule adds stay below this, so `after_spend` can never overflow.
const SANE: u64 = (i64::MAX / 3) as u64;
const RANGE: Bad = "numbers out of range";
const MISMATCH: Bad = "the continuation script is not the next state's";

/// A spending transaction as a history source reports it.
#[derive(Clone, Debug)]
pub struct Spend {
    /// Inputs with full signature scripts, outputs with their covenant bindings. Its `lock_time`
    /// field is not read: `lock_time` below says whether the source knows it.
    pub tx: Transaction,
    /// The transaction id as the source reports it (it names the continuation coin).
    pub txid: Hash,
    /// The lock time when the source has it (a node's block; the public indexer does not keep
    /// it). A hint like the payload's echo: used only when it reproduces the continuation.
    pub lock_time: Option<u64>,
    /// DAA score of the block that accepted the transaction: the continuation coin's DAA score.
    pub accepted_daa: u64,
    /// Blocks the source says hold the transaction: hints where our node keeps its copy.
    pub blocks: Vec<Hash>,
}

/// Where the walk learns the account's coins. Untrusted: every answer is checked before use.
pub trait SpenderSource: Send + Sync {
    /// A coin carrying `covenant_id` paid to `address` (any one: they all lie on the account's
    /// path), with its amount, script, DAA score and covenant id; None when the history has none.
    fn first_coin<'a>(
        &'a self,
        address: &'a Address,
        covenant_id: Hash,
    ) -> BoxFuture<'a, Result<Option<Coin>, String>>;
    /// The transaction that spent `outpoint` (matched on the outpoint, never the address), or
    /// None while the coin is unspent.
    fn spender<'a>(
        &'a self,
        outpoint: &'a TransactionOutpoint,
    ) -> BoxFuture<'a, Result<Option<Spend>, String>>;
    /// Our node's live coins paid to `address` (Ctx `try_utxos`, through `WithNode`); a
    /// history source answers none: its coins carry no hop proof. The walk keeps only a coin
    /// carrying the covenant id whose script is the current state's, and jumps to the newest.
    /// Optional: without it, every hop is walked.
    fn coins_at<'a>(&'a self, _address: &'a Address) -> BoxFuture<'a, Result<Vec<Coin>, String>> {
        Box::pin(futures::future::ready(Ok(Vec::new())))
    }
}

/// What the walk asks our own node: its live coins at an address (Ctx `try_utxos`), and the lock
/// time of a transaction in one of the named blocks (None when it does not hold it).
pub trait NodeView: Send + Sync {
    fn live_coins<'a>(&'a self, address: &'a Address) -> BoxFuture<'a, Result<Vec<Coin>, String>>;
    /// Err: the node could not answer (the reason, for the log); Ok(None): it does not hold it.
    fn lock_time<'a>(&'a self, txid: Hash, blocks: &'a [Hash])
        -> BoxFuture<'a, Result<Option<u64>, String>>;
}

impl NodeView for crate::tx::Ctx {
    fn live_coins<'a>(&'a self, address: &'a Address)
        -> BoxFuture<'a, Result<Vec<Coin>, String>> {
        Box::pin(self.try_utxos(address))
    }

    fn lock_time<'a>(&'a self, txid: Hash, blocks: &'a [Hash])
        -> BoxFuture<'a, Result<Option<u64>, String>> {
        use kaspa_rpc_core::api::rpc::RpcApi;
        Box::pin(async move {
            let mut failed = None;
            for block in blocks.iter().take(NODE_BLOCKS) {
                let answer = self.rpc().get_block(*block, true).await;
                let lock = answer.as_ref().ok().and_then(|b| lock_in(b, txid));
                if lock.is_some() {
                    return Ok(lock);
                }
                failed = failed.or(answer.err().map(|e| format!("get_block: {e}")));
            }
            failed.map_or(Ok(None), Err)
        })
    }
}

impl crate::tx::Ctx {
    /// Whether our node's copy of transaction `txid` (looked for in `blocks`, the history's hints)
    /// spends `outpoint`. Err when the node holds no copy there or cannot answer: never a guess.
    pub async fn spent_by(&self, txid: Hash, blocks: &[Hash], outpoint: &TransactionOutpoint)
        -> Result<bool, String> {
        use kaspa_rpc_core::api::rpc::RpcApi;
        let mut failed = None;
        for block in blocks.iter().take(NODE_BLOCKS) {
            let b = match self.rpc().get_block(*block, true).await {
                Ok(b) => b,
                Err(e) => {
                    failed = failed.or(Some(format!("get_block: {e}")));
                    continue;
                }
            };
            if let Some(spends) = spends_in(&b, txid, outpoint) {
                return Ok(spends);
            }
        }
        Err(failed.unwrap_or_else(|| "spent_by: the node holds no copy of the ending spend".into()))
    }
}

/// Whether a block our node sent holds `txid` (matched on the node's own id) and, if so, whether
/// that transaction spends `outpoint`.
fn spends_in(block: &kaspa_rpc_core::RpcBlock, txid: Hash, outpoint: &TransactionOutpoint)
    -> Option<bool> {
    let ours = |t: &&kaspa_rpc_core::RpcTransaction| {
        t.verbose_data.as_ref().is_some_and(|d| d.transaction_id == txid)
    };
    let tx = block.transactions.iter().find(ours)?;
    let spent = |i: &kaspa_rpc_core::RpcTransactionInput| {
        i.previous_outpoint.transaction_id == outpoint.transaction_id
            && i.previous_outpoint.index == outpoint.index
    };
    Some(tx.inputs.iter().any(spent))
}

/// A transaction's lock time from a block our node sent, matched on the node's own id for it.
fn lock_in(block: &kaspa_rpc_core::RpcBlock, txid: Hash) -> Option<u64> {
    let ours = |t: &&kaspa_rpc_core::RpcTransaction| {
        t.verbose_data.as_ref().is_some_and(|d| d.transaction_id == txid)
    };
    block.transactions.iter().find(ours).map(|t| t.lock_time)
}

/// A history source plus our own node. Hops come from `history` (each one checked); `coins_at` is
/// the node's live coins only, so the walk never jumps to a coin only the history shows; a spend's
/// lock time comes from the node's copy when it holds one (the search is the fallback).
pub struct WithNode<'a> {
    history: &'a dyn SpenderSource,
    node: &'a dyn NodeView,
    /// Whether this walk already logged why the node could not give a lock time.
    warned: std::sync::atomic::AtomicBool,
}

impl<'a> WithNode<'a> {
    pub fn new(history: &'a dyn SpenderSource, node: &'a dyn NodeView) -> WithNode<'a> {
        WithNode { history, node, warned: false.into() }
    }

    /// The node's lock time for a spend; a node error falls back to the search, logged once.
    async fn node_lock(&self, spend: &Spend) -> Option<u64> {
        match self.node.lock_time(spend.txid, &spend.blocks).await {
            Ok(lock) => lock,
            Err(why) => {
                if !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("paper walk: node lock time unavailable, searching instead ({why})");
                }
                None
            }
        }
    }
}

impl SpenderSource for WithNode<'_> {
    fn first_coin<'a>(&'a self, address: &'a Address, covenant_id: Hash)
        -> BoxFuture<'a, Result<Option<Coin>, String>> {
        self.history.first_coin(address, covenant_id)
    }

    fn spender<'a>(&'a self, outpoint: &'a TransactionOutpoint)
        -> BoxFuture<'a, Result<Option<Spend>, String>> {
        Box::pin(async move {
            let Some(mut spend) = self.history.spender(outpoint).await? else { return Ok(None) };
            if let Some(lock) = self.node_lock(&spend).await {
                spend.lock_time = Some(lock);
            }
            Ok(Some(spend))
        })
    }

    fn coins_at<'a>(&'a self, address: &'a Address) -> BoxFuture<'a, Result<Vec<Coin>, String>> {
        self.node.live_coins(address)
    }
}

// INDEXER CLIENT: plugs in here. A public history service (api.kaspa.org, api-tn10.kaspa.org)
// implements `SpenderSource` as docs/paper-stop-indexer.md describes: `first_coin` from the
// address's full transactions, `spender` by paging the address's transactions after the coin's
// block time and matching an input's previous outpoint; `accepted_daa` from the accepting block.

/// Where the walk ended.
#[derive(Debug)]
pub struct Walk {
    /// The live coin and the record at it. The coin's entry came from the source: take it from
    /// the node at its address before building a transaction on it.
    pub live: Option<(Coin, Vault)>,
    /// The account ended: a stop or an inheritance paid it out, or a finish emptied it.
    pub ended: bool,
    /// Spending transactions followed (0: the record's coin is still live).
    pub hops: usize,
    /// The record after the last followed move; its history has one line per hop.
    pub last: Vault,
    /// The spend that ended the account (set only when `ended`): the one transaction whose cold
    /// coin may answer `stopped`.
    pub end_txid: Option<Hash>,
    /// The coin that ending spend spent (set only when `ended`): our node must show the ending
    /// transaction spending it (`Ctx::spent_by`) before its cold coin answers `stopped`.
    pub end_from: Option<TransactionOutpoint>,
    /// Blocks the history says hold the ending spend: where our node's copy is looked for.
    pub end_blocks: Vec<Hash>,
}

/// How far one walk may go: hops, total time (lookups and lock time searches together), and
/// questions to the source.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_hops: usize,
    pub time: Duration,
    pub calls: usize,
}

impl Limits {
    pub const WALK: Limits = Limits { max_hops: MAX_HOPS, time: WALK_TIME, calls: WALK_CALLS };
}

/// Whether a walk error means it ran out of its limits (paper.rs answers MOVED).
pub fn too_long(e: &str) -> bool {
    e == TOO_LONG || e.starts_with(LONGER)
}

/// What is left of one walk's limits.
struct Budget {
    deadline: Instant,
    calls: usize,
}

impl Budget {
    fn new(l: Limits) -> Budget {
        Budget { deadline: Instant::now() + l.time, calls: l.calls }
    }

    /// One question to the source within the walk's questions and deadline; its error as the
    /// user reads it.
    async fn ask<T>(&mut self, question: BoxFuture<'_, Result<T, String>>) -> Result<T, String> {
        if self.calls == 0 || Instant::now() >= self.deadline {
            return Err(TOO_LONG.into());
        }
        self.calls -= 1;
        let until = tokio::time::Instant::from_std(self.deadline);
        let answer = tokio::time::timeout_at(until, question).await;
        answer.map_err(|_| TOO_LONG.to_string())?.map_err(unreadable)
    }
}

/// paper.rs: from a watch code's public block and Vault record (the coin may be long spent), asks
/// `source` for a coin of the record's state, then walks to the live coin within `limits`.
pub async fn walk_to_live_from_record(
    public: &Public,
    record: &Vault,
    source: &dyn SpenderSource,
    limits: Limits,
) -> Result<Walk, String> {
    let rules = Arc::new(Rules::new(public, record)?);
    let mut budget = Budget::new(limits);
    let spk = rules.spk(record, &record.state);
    let address = rules.network.address_of(&spk).ok_or_else(|| CHECK_FAILED.to_string())?;
    let found = budget.ask(source.first_coin(&address, record.covenant_id)).await?;
    let printed = found.ok_or_else(|| NOT_FOUND.to_string())?;
    walk(&rules, record, printed, source, (limits.max_hops, &mut budget)).await
}

/// The same walk from a coin the caller already holds (`printed`, a coin of `record.state`).
pub async fn walk_to_live(
    public: &Public,
    record: &Vault,
    printed: &Coin,
    source: &dyn SpenderSource,
    limits: Limits,
) -> Result<Walk, String> {
    let rules = Arc::new(Rules::new(public, record)?);
    let mut budget = Budget::new(limits);
    walk(&rules, record, printed.clone(), source, (limits.max_hops, &mut budget)).await
}

async fn walk(
    rules: &Arc<Rules>,
    record: &Vault,
    printed: Coin,
    source: &dyn SpenderSource,
    (max_hops, budget): (usize, &mut Budget),
) -> Result<Walk, String> {
    let (mut v, mut coin) = (record.clone(), printed);
    // `coins_at` is asked once per state reached, so padding costs one question however long
    let mut asked: Option<State> = None;
    for hops in 0..=max_hops {
        if asked.as_ref() != Some(&v.state) {
            coin = newest(rules, &v, coin, source, budget).await;
            asked = Some(v.state.clone());
        }
        let Some(spend) = budget.ask(source.spender(&coin.0)).await? else {
            let live = Some((coin, v.clone()));
            let (end_txid, end_from, end_blocks) = (None, None, Vec::new());
            return Ok(Walk { live, ended: false, hops, last: v, end_txid, end_from, end_blocks });
        };
        if hops == max_hops {
            break;
        }
        let (txid, end_blocks) = (spend.txid, spend.blocks.clone());
        let step = checked_hop(rules, &v, &coin, spend, budget.deadline).await?;
        v = step.record;
        match step.next {
            Some(c) => coin = c,
            None => {
                let (end_txid, end_from, hops) = (Some(txid), Some(coin.0), hops + 1);
                let last = v;
                let (live, ended) = (None, true);
                let walk = Walk { live, ended, hops, last, end_txid, end_from, end_blocks };
                return Ok(walk);
            }
        }
    }
    let end = "moves, so it was not followed to the end. No funds were moved.";
    Err(format!("{LONGER} {max_hops} {end}"))
}

/// `hop` on the blocking pool (a lock time search holds a thread). A refusal once the walk's
/// deadline has passed is the budget's (TOO_LONG), not the history's.
async fn checked_hop(rules: &Arc<Rules>, v: &Vault, coin: &Coin, spend: Spend, deadline: Instant)
    -> Result<Step, String> {
    let (r, v, c) = (Arc::clone(rules), v.clone(), coin.clone());
    let done = tokio::task::spawn_blocking(move || hop(&r, &v, &c, &spend, deadline)).await;
    match done {
        Ok(Ok(step)) => Ok(step),
        _ if Instant::now() >= deadline => Err(TOO_LONG.into()),
        _ => Err(CHECK_FAILED.into()),
    }
}

/// The newest coin of `v.state` the source shows at the state's address (the node: the live
/// coin), when it is not older than `coin`; else `coin`. Every coin carrying the covenant id lies
/// on the account's one path, and the script commits to the whole state, so a coin passing both
/// checks is a coin of this state: jumping to it skips only moves that came back to it (top-ups).
/// A failed lookup walks on: the shortcut is never needed for a correct walk. Through `WithNode`
/// the coins are our node's only, so the jump never lands on a coin only the history shows.
async fn newest(rules: &Rules, v: &Vault, coin: Coin, source: &dyn SpenderSource,
    budget: &mut Budget) -> Coin {
    let want = rules.spk(v, &v.state);
    let Some(address) = rules.network.address_of(&want) else { return coin };
    let Ok(coins) = budget.ask(source.coins_at(&address)).await else { return coin };
    let ours = |c: &&Coin| c.1.covenant_id == Some(v.covenant_id) && c.1.script_public_key == want;
    let best = coins.iter().take(MAX_COINS).filter(ours).max_by_key(|c| c.1.block_daa_score);
    match best {
        Some(b) if b.0 != coin.0 && b.1.block_daa_score >= coin.1.block_daa_score => b.clone(),
        _ => coin,
    }
}

/// A source error as the user reads it; an unsettled move (indexer.rs) is "try again shortly".
fn unreadable(e: String) -> String {
    if e == crate::indexer::UNSETTLED {
        return SETTLING.into();
    }
    if e == crate::indexer::OVER_BUDGET {
        return TOO_LONG.into();
    }
    let shown: String = e.chars().take(200).collect();
    eprintln!("paper walk: history source error: {shown}");
    UNREADABLE.into()
}

/// The account's rules as the engine compiles them, from a watch code's public block.
struct Rules {
    source: &'static str,
    public: Public,
    network: Network,
    cold: ScriptPublicKey,
    /// Where an inheritance pays: the heir, or cold when there is none (the slot repeats cold).
    heir: ScriptPublicKey,
    /// Compiles already made in this walk (`art`).
    arts: Mutex<Arts>,
}

/// A compile depends only on the source, the public block (both fixed in `Rules`), the dials and
/// the state, so it is kept keyed by the exact (dials, state) and reused only on an equal key.
/// Every check still compares against the compiled output as before; only the rebuild is skipped.
type Arts = Vec<(contract::Dials, State, Arc<SilAbiArtifact>)>;
/// Compiles kept (oldest dropped first): a hop needs the current and the next state.
const ART_CACHE: usize = 8;

impl Rules {
    /// The built-in rules (the paper stop is newer than every older version).
    fn new(public: &Public, record: &Vault) -> Result<Rules, String> {
        let built_in = contract::embedded_sha256();
        let sha = record.contract_sha256.as_deref().unwrap_or(built_in);
        if sha != built_in && sha != contract::FIRST_V6_SHA {
            return Err(OLD_RULES.into());
        }
        let network = public.network();
        let spk = |a: &str| network.try_to_spk(a).map_err(|_| CHECK_FAILED.to_string());
        let cold = spk(&public.cold_address)?;
        let heir = match public.heir_address.as_str() {
            "" => cold.clone(),
            a => spk(a)?,
        };
        let public = public.clone();
        let arts = Mutex::new(Vec::new());
        Ok(Rules { source: contract::EMBEDDED_SOURCE, public, network, cold, heir, arts })
    }

    fn art(&self, v: &Vault, s: &State) -> Arc<SilAbiArtifact> {
        let same = |e: &&(contract::Dials, State, Arc<SilAbiArtifact>)| e.0 == v.dials && e.1 == *s;
        let found = |a: &Arts| a.iter().find(same).map(|e| Arc::clone(&e.2));
        let hit = self.arts.lock().ok().and_then(|a| found(&a));
        if let Some(art) = hit {
            return art;
        }
        let art = Arc::new(contract::compile(self.source, &self.public, &v.dials, s));
        if let Ok(mut a) = self.arts.lock() {
            if a.len() >= ART_CACHE {
                a.remove(0);
            }
            a.push((v.dials.clone(), s.clone(), Arc::clone(&art)));
        }
        art
    }

    fn spk(&self, v: &Vault, s: &State) -> ScriptPublicKey {
        contract::spk_of(&self.art(v, s))
    }
}

/// One checked hop: the record after the move, and the continuation coin (None: the account ended).
struct Step {
    record: Vault,
    next: Option<Coin>,
}

/// A spending transaction as read for one hop.
struct Read<'a> {
    spend: &'a Spend,
    pushes: Vec<Vec<u8>>,
    entry: String,
    /// The output carrying the account's covenant id, with its index.
    cont: Cont<'a>,
    /// The walk's deadline, for a lock time search.
    deadline: Instant,
}

/// Checks one spending transaction of `coin` and applies it to `v` with watch.rs `follow_record`.
fn hop(rules: &Rules, v: &Vault, coin: &Coin, spend: &Spend, deadline: Instant)
    -> Result<Step, Bad> {
    if spend.accepted_daa < coin.1.block_daa_score {
        return Err("the spend was accepted before the coin existed");
    }
    let tx = &spend.tx;
    let spends = |i: &&TransactionInput| i.previous_outpoint == coin.0;
    let input = tx.inputs.first().filter(spends).ok_or("not this coin's spender")?;
    let art = rules.art(v, &v.state);
    let ours = coin.1.covenant_id == Some(v.covenant_id);
    if !ours || coin.1.script_public_key != contract::spk_of(&art) {
        return Err("the coin is not the account's in this state");
    }
    let pushes = pushes(&input.signature_script).ok_or("unreadable signature script")?;
    let [.., tag, code] = pushes.as_slice() else { return Err("too few pushes") };
    if *code != contract::bytecode(&art) {
        return Err("the input runs another script");
    }
    let entry = entry_name(&art, tag).ok_or("no known entry")?;
    let cont = continuation(tx, v)?;
    let r = Read { spend, pushes, entry, cont, deadline };
    let next = next_state(rules, v, coin, &r)?;
    let record = follow_record(v, &move_of(rules, v, coin, &r, next.clone()));
    let Some(((i, o), _)) = r.cont.zip(next) else { return Ok(Step { record, next: None }) };
    let op = TransactionOutpoint::new(spend.txid, u32::try_from(i).map_err(|_| RANGE)?);
    let (spk, cov) = (o.script_public_key.clone(), Some(v.covenant_id));
    let entry = UtxoEntry::new(o.value, spk, spend.accepted_daa, false, cov);
    Ok(Step { record, next: Some((op, entry)) })
}

/// The output carrying the account's covenant id: at most one, created by the vault input (0).
type Cont<'a> = Option<(usize, &'a TransactionOutput)>;

fn continuation<'a>(tx: &'a Transaction, v: &Vault) -> Result<Cont<'a>, Bad> {
    let ours = |o: &TransactionOutput| o.covenant.is_some_and(|c| c.covenant_id == v.covenant_id);
    let mut found = tx.outputs.iter().enumerate().filter(|(_, o)| ours(o));
    let first = found.next();
    if found.next().is_some() {
        return Err("two outputs carry the covenant");
    }
    if first.is_some_and(|(_, o)| o.covenant.is_some_and(|c| c.authorizing_input != 0)) {
        return Err("the continuation is not bound to the vault input");
    }
    Ok(first)
}

/// The state the move leads to (None: the account ended here), derived as watch.rs `judge` does,
/// and checked: the continuation script must be exactly the compiled script of that state.
fn next_state(rules: &Rules, v: &Vault, coin: &Coin, r: &Read) -> Result<Option<State>, Bad> {
    let s = &v.state;
    let next = match (r.entry.as_str(), r.cont) {
        ("spend", Some((_, o))) => return spend_next(rules, v, coin, r, o).map(Some),
        ("request", Some(_)) => request_next(s, &r.pushes).ok_or("unreadable request")?,
        ("finish", Some(_)) => {
            payout_ok(rules, v, "finish", &r.spend.tx)?;
            after_finish(s)
        }
        ("decline", Some(_)) => after_decline(s),
        ("topup", Some(_)) => s.clone(),
        ("cancel" | "inherit" | "inheritOpen" | "finish", _) => {
            payout_ok(rules, v, &r.entry, &r.spend.tx)?;
            return Ok(None);
        }
        _ => return Err("a move the rules do not make"),
    };
    let want = r.cont.map(|(_, o)| &o.script_public_key).ok_or("no continuation")?;
    if rules.spk(v, &next) != *want {
        return Err(MISMATCH);
    }
    Ok(Some(next))
}

/// `spend` (contract.rs `after_spend`): the outflow is the coin minus the continuation. Either the
/// open window takes it, or a new window opens at the transaction's lock time. Lock time hints
/// (the source's, the payload's echo) are tried first; the card key signs both, so one that does
/// not reproduce the continuation is dropped and the lock time is searched (`lock_search`).
fn spend_next(rules: &Rules, v: &Vault, coin: &Coin, r: &Read, o: &TransactionOutput)
    -> Result<State, Bad> {
    let spend = r.spend;
    let outflow = i128::from(coin.1.amount) - i128::from(o.value);
    let outflow = i64::try_from(outflow).map_err(|_| RANGE)?;
    let (s, w, want) = (&v.state, v.dials.window, &o.script_public_key);
    if [outflow, s.spent, s.window_start, w].iter().any(|x| x.unsigned_abs() > SANE) {
        return Err(RANGE);
    }
    // a coin DAA past every lock time: `after_spend` keeps the open window
    let same = after_spend(s, w, i64::MAX / 4, 0, outflow);
    if rules.spk(v, &same) == *want {
        return Ok(same);
    }
    // a coin DAA before every lock time: a new window opens at `lock` once the open one has passed
    let opened = |lock: i64| after_spend(s, w, i64::MIN / 4, lock, outflow);
    let hints = [spend.lock_time, lock_time_from_payload(&spend.tx.payload)];
    for lock in hints.into_iter().flatten().filter_map(|l| sane(l).ok()) {
        let n = opened(lock);
        if rules.spk(v, &n) == *want {
            return Ok(n);
        }
    }
    // the rules open a window only past both the open window's start and the coin's own DAA score
    let low = (s.window_start + w).max(sane(coin.1.block_daa_score)? + w);
    lock_search(rules, v, (sane(spend.accepted_daa)?, low), &opened, (want, r.deadline))
}

fn sane(x: u64) -> Result<i64, Bad> {
    i64::try_from(x).ok().filter(|x| x.unsigned_abs() <= SANE).ok_or(RANGE)
}

/// A window-opening spend's lock time no hint gave: every value the rules allow, from the lowest
/// (`low`) to the accepting block's DAA score (`high`), within `LOCK_BUDGET` and the walk's
/// deadline. Each try patches the value into a template of the compiled script and hashes it;
/// a hit is confirmed by a full
/// compile, so the template only speeds the search and the proof stays.
fn lock_search(rules: &Rules, v: &Vault, (high, low): (i64, i64), opened: &dyn Fn(i64) -> State,
    (want, deadline): (&ScriptPublicKey, Instant)) -> Result<State, Bad> {
    let none = "no lock time the rules allow matched the continuation within the budget";
    if high < low {
        return Err(none);
    }
    let (t, target) = (Template::new(rules, v, &opened(low))?, script_hash(want).ok_or(none)?);
    let until = deadline.min(Instant::now() + LOCK_BUDGET);
    let lock = sweep(&t, (high, low), target, until).ok_or(none)?;
    let n = opened(lock);
    if rules.spk(v, &n) == *want { Ok(n) } else { Err("the template is not the script") }
}

/// Splits [low, high] over the CPU's threads (at most `MAX_THREADS`), each trying its share from
/// both ends inward: the top holds the engine's own DAA - 20, the bottom the lowest value the
/// rules allow (a thief's likely pick). The first hit stops every thread; so does `deadline`.
fn sweep(t: &Template, (high, low): (i64, i64), target: &[u8], deadline: Instant)
    -> Option<i64> {
    let span = u64::try_from(high - low).ok()?;
    let n = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, MAX_THREADS);
    let stop = AtomicBool::new(false);
    let ends = (high, low, span);
    std::thread::scope(|scope| {
        let runs: Vec<_> = (0..n as u64)
            .map(|i| {
                let (mut t, stop) = (t.clone(), &stop);
                scope.spawn(move || stripe(&mut t, ends, (i, n), target, stop, deadline))
            })
            .collect();
        runs.into_iter().filter_map(|r| r.join().ok().flatten()).next()
    })
}

/// One thread's share: k = i, i + n, i + 2n... up to the span. An even k tries `high - k/2`, an
/// odd one `low + k/2`, so the two ends meet in the middle and every value is tried once.
fn stripe(t: &mut Template, (high, low, span): (i64, i64, u64), (i, n): (u64, usize),
    target: &[u8], stop: &AtomicBool, deadline: Instant) -> Option<i64> {
    for (j, k) in (i..=span).step_by(n).enumerate() {
        if j % CLOCK_EVERY == 0 && (stop.load(Relaxed) || Instant::now() > deadline) {
            return None;
        }
        let half = i64::try_from(k / 2).ok()?;
        let lock = if k % 2 == 0 { high - half } else { low + half };
        if t.hash(lock)?.as_bytes() == target {
            stop.store(true, Relaxed);
            return Some(lock);
        }
    }
    None
}

/// A window-opening state's compiled script with its window start as a patchable 8-byte field.
#[derive(Clone)]
struct Template {
    code: Vec<u8>,
    at: usize,
}

impl Template {
    /// Read off the compiler: two compiles whose window starts differ in every byte must differ in
    /// exactly those 8 bytes, little-endian, and nowhere else; any other shape is refused.
    fn new(rules: &Rules, v: &Vault, base: &State) -> Result<Template, Bad> {
        let (a, b) = (0x0102_0304_0506_0708_i64, 0x1112_1314_1516_1718_i64);
        let with = |ws: i64| State { window_start: ws, ..base.clone() };
        let code = |ws: i64| contract::bytecode(&rules.art(v, &with(ws)));
        let (ca, cb) = (code(a), code(b));
        let all = 0..ca.len().max(cb.len());
        let diff: Vec<usize> = all.filter(|&i| ca.get(i) != cb.get(i)).collect();
        let at = *diff.first().ok_or("no template")?;
        let shape = ca.len() == cb.len() && diff == (at..at + 8).collect::<Vec<_>>();
        let field = |c: &[u8], x: i64| c.get(at..at + 8) == Some(&x.to_le_bytes()[..]);
        if !(shape && field(&ca, a) && field(&cb, b)) {
            return Err("no template");
        }
        Ok(Template { code: ca, at })
    }

    /// The script hash with `window_start` in the field: the digest `pay_to_script_hash_script`
    /// takes (blake2b-256), without building the script public key.
    fn hash(&mut self, window_start: i64) -> Option<blake2b_simd::Hash> {
        let field = self.code.get_mut(self.at..self.at + 8)?;
        field.copy_from_slice(&window_start.to_le_bytes());
        Some(blake2b_simd::Params::new().hash_length(32).hash(&self.code))
    }
}

/// The script hash a continuation pays to, read off the shape `pay_to_script_hash_script` builds
/// (None for any other script: no vault state compiles to it).
fn script_hash(want: &ScriptPublicKey) -> Option<&[u8]> {
    let probe = pay_to_script_hash_script(&[]);
    let (p, w) = (probe.script(), want.script());
    let shape = probe.version() == want.version() && p.len() == 35 && w.len() == 35;
    (shape && p[..2] == w[..2] && p[34] == w[34]).then(|| &w[2..34])
}

/// `request(sig, newDestSpk, newAmount)`: Pending, with the destination's hash and the amount.
fn request_next(s: &State, p: &[Vec<u8>]) -> Option<State> {
    if p.len() < 4 {
        return None;
    }
    let dest_hash = spk_hash(&decode_spk(&p[1])?);
    let amount = script_int(&p[2])?;
    Some(State { stage: 1, dest_hash, amount, ..s.clone() })
}

/// An ending (or a finish) must pay where the rules pin it, as watch.rs `judge` checks: a finish to
/// the recorded destination and amount, a cancel to cold, an inheritance to the heir (no heir:
/// cold), from Pending after paying the recorded destination first.
fn payout_ok(rules: &Rules, v: &Vault, entry: &str, tx: &Transaction) -> Result<(), Bad> {
    let s = &v.state;
    let out = |i: usize| tx.outputs.get(i);
    let pays = |o: Option<&TransactionOutput>, want: &ScriptPublicKey| {
        o.is_some_and(|o| o.script_public_key == *want)
    };
    let to_dest = |o: Option<&TransactionOutput>| {
        let hash = |o: &TransactionOutput| spk_hash(&o.script_public_key);
        o.is_some_and(|o| hash(o) == s.dest_hash && o.value == s.amount as u64)
    };
    let ok = match entry {
        "cancel" => pays(out(0), &rules.cold),
        "finish" => to_dest(out(0)),
        _ if s.stage == 1 => to_dest(out(0)) && pays(out(1), &rules.heir),
        _ => pays(out(0), &rules.heir),
    };
    if ok { Ok(()) } else { Err("the move paid somewhere the rules do not allow") }
}

/// The watcher's `Move` for this hop (watch.rs `judge` fills the same fields), for `follow_record`.
fn move_of(rules: &Rules, v: &Vault, coin: &Coin, r: &Read, next: Option<State>) -> Move {
    let (tx, p, net) = (&r.spend.tx, &r.pushes, &rules.network);
    let in_value = coin.1.amount;
    let out0 = tx.outputs.first();
    let v0 = out0.map_or(0, |o| o.value);
    let to0 = out0.map(|o| addr_of(net, &o.script_public_key));
    let request = if r.entry == "request" { p.get(1).and_then(|d| decode_spk(d)) } else { None };
    let to = request.map(|spk| addr_of(net, &spk)).or(to0).unwrap_or("nowhere".into());
    let moved = match (r.entry.as_str(), r.cont) {
        ("request", _) => next.as_ref().map_or(0, |n| n.amount.max(0) as u64),
        ("topup", Some((_, o))) => o.value.saturating_sub(in_value),
        _ => v0,
    };
    let paid = tx.outputs.iter().fold(0u64, |a, o| a.saturating_add(o.value));
    let fee = (tx.inputs.len() == 1).then(|| in_value.saturating_sub(paid));
    let slot = if p.len() >= 4 { p.iter().rev().nth(2).and_then(|b| script_int(b)) } else { None };
    Move {
        entry: r.entry.clone(),
        closed: next.is_none(),
        next,
        to,
        moved,
        fee,
        txid: r.spend.txid.to_string(),
        slot,
        delay: v.dials.delay,
        heir: rules.public.heir_address.clone(),
    }
}

/// The entry a dispatch tag selects in the compiled rules (as watch.rs `entry_name`).
fn entry_name(art: &SilAbiArtifact, tag: &[u8]) -> Option<String> {
    let c = art.contracts.values().next()?;
    let hit = c.entries.iter().find(|(_, e)| e.dispatch_tag.as_bytes().as_slice() == tag);
    hit.map(|(n, _)| n.clone())
}

/// A signature script's data pushes, read as watch.rs `pushes` reads them (private there): a
/// truncated push or any non-push opcode makes the whole script unreadable (None).
fn pushes(script: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = vec![];
    let mut i = 0;
    // every turn consumes at least one byte, so the script's length bounds the loop
    for _ in 0..script.len() {
        let Some(&op) = script.get(i) else { break };
        i += 1;
        if let Some(small) = small_int(op) {
            out.push(small);
            continue;
        }
        let (n, head) = push_len(op, script.get(i..)?)?;
        i += head;
        out.push(script.get(i..i.checked_add(n)?)?.to_vec());
        i += n;
    }
    Some(out)
}

/// OP_1NEGATE and OP_1..OP_16 as the numbers they push.
fn small_int(op: u8) -> Option<Vec<u8>> {
    match op {
        0x4f => Some(vec![0x81]),
        0x51..=0x60 => Some(vec![op - 0x50]),
        _ => None,
    }
}

/// The data length a push opcode announces and how many length bytes follow it (None: no push).
fn push_len(op: u8, rest: &[u8]) -> Option<(usize, usize)> {
    let le = |n: usize| -> Option<usize> {
        Some(rest.get(..n)?.iter().rev().fold(0usize, |a, &x| (a << 8) | x as usize))
    };
    match op {
        0x00..=0x4b => Some((op as usize, 0)),
        0x4c => Some((le(1)?, 1)),
        0x4d => Some((le(2)?, 2)),
        0x4e => Some((le(4)?, 4)),
        _ => None,
    }
}

/// Script-number decoding: little-endian, sign bit in the last byte; over 8 bytes is unreadable.
fn script_int(b: &[u8]) -> Option<i64> {
    if b.len() > 8 {
        return None;
    }
    let Some((&last, _)) = b.split_last() else { return Some(0) };
    let mut v: i64 = 0;
    for (i, &x) in b.iter().enumerate() {
        let x = if i == b.len() - 1 { x & 0x7f } else { x };
        v |= (x as i64) << (8 * i);
    }
    Some(if last & 0x80 != 0 { -v } else { v })
}

/// A script public key as the contract pushes it: two version bytes (big-endian) then the script.
fn decode_spk(b: &[u8]) -> Option<ScriptPublicKey> {
    let version = <[u8; 2]>::try_from(b.get(..2)?).ok()?;
    Some(ScriptPublicKey::from_vec(u16::from_be_bytes(version), b.get(2..)?.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Dials, KAS, locked, spk_bytes};
    use crate::keys::Keys;
    use crate::tx::{Ctx, lock_time_payload};
    use futures::FutureExt;
    use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
    use kaspa_txscript::script_builder::ScriptBuilder;

    const FEE: u64 = KAS / 1000;
    /// DAA between moves: past the testnet window (1 200), so every spend opens a new window.
    const LATER: u64 = 2_000;

    /// A fake history service answering from transactions the tests built.
    #[derive(Default)]
    struct Fake {
        first: Option<(Address, Coin)>,
        txs: Vec<(TransactionOutpoint, Spend)>,
        fail: bool,
        /// Withhold lock times, as the public indexer does.
        hide_locks: bool,
        /// What `coins_at` shows (the node's live coin, or a history source's newest).
        coins: Vec<(Address, Coin)>,
    }

    impl SpenderSource for Fake {
        fn first_coin<'a>(&'a self, address: &'a Address, cov: Hash)
            -> BoxFuture<'a, Result<Option<Coin>, String>> {
            let ours = |(a, c): &&(Address, Coin)| a == address && c.1.covenant_id == Some(cov);
            let hit = self.first.as_ref().filter(ours).map(|h| h.1.clone());
            let answer = if self.fail { Err("offline".into()) } else { Ok(hit) };
            futures::future::ready(answer).boxed()
        }

        fn spender<'a>(&'a self, op: &'a TransactionOutpoint)
            -> BoxFuture<'a, Result<Option<Spend>, String>> {
            let hit = self.txs.iter().find(|(o, _)| o == op).map(|(_, t)| t.clone());
            let hide = |s: Spend| if self.hide_locks { Spend { lock_time: None, ..s } } else { s };
            let answer = if self.fail { Err("offline".into()) } else { Ok(hit.map(hide)) };
            futures::future::ready(answer).boxed()
        }

        fn coins_at<'a>(&'a self, address: &'a Address)
            -> BoxFuture<'a, Result<Vec<Coin>, String>> {
            let hits = self.coins.iter().filter(|(a, _)| a == address).map(|(_, c)| c.clone());
            futures::future::ready(Ok(hits.collect())).boxed()
        }
    }

    /// An account's history as the engine writes it: each move spends the current coin.
    struct Chain {
        ctx: Ctx,
        v: Vault,
        printed: Coin,
        coin: Coin,
        daa: u64,
        txs: Vec<(TransactionOutpoint, Spend)>,
        hide_locks: bool,
        /// Coins `coins_at` shows.
        node: Vec<Coin>,
    }

    fn chain(tag: &str) -> Chain {
        let dir = std::env::temp_dir().join(format!("kv-lineage-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        contract::pin_source(&dir).unwrap();
        let (network, dials) = (Network::testnet(), Dials::testnet());
        let keys = Keys::fresh(&network);
        let ctx = Ctx { dir, keys, rpc: None, force: false, network, dials, account: 1 };
        let v = Vault {
            covenant_id: Hash::from_bytes([7u8; 32]),
            state: locked(),
            pending_to: None,
            history: vec![],
            contract_sha256: None,
            network: String::new(),
            dials: ctx.dials.clone(),
        };
        let op = TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0);
        let (spk, daa) = (ctx.vault_spk(&v.state), 1_000_000);
        let printed = (op, UtxoEntry::new(10 * KAS, spk, daa, false, Some(v.covenant_id)));
        let (txs, node) = (vec![], vec![]);
        Chain { ctx, v, coin: printed.clone(), printed, daa, txs, hide_locks: false, node }
    }

    fn payee() -> ScriptPublicKey {
        ScriptPublicKey::from_vec(0, [vec![0x20], vec![9u8; 32], vec![0xac]].concat())
    }

    impl Chain {
        /// Spends the current coin through `entry` (the sigscript pushes the args, the entry's
        /// dispatch tag and the bytecode, as watch.rs tests build it), accepted at `self.daa`;
        /// `next` moves the coin on.
        fn spend_by(&mut self, entry: &str, args: Vec<Vec<u8>>, outs: Vec<TransactionOutput>,
            lock: u64, next: Option<State>) -> Transaction {
            let art = self.ctx.compile(&self.v.state);
            let c = art.contracts.values().next().unwrap();
            let tag = c.entries[entry].dispatch_tag.as_bytes().to_vec();
            let mut b = ScriptBuilder::new();
            for push in args.into_iter().chain([tag, contract::bytecode(&art)]) {
                b.add_data(&push).unwrap();
            }
            let inputs = vec![TransactionInput::new(self.coin.0, b.drain(), 0, 0)];
            let tx = Transaction::new(1, inputs, outs, lock, SUBNETWORK_ID_NATIVE, 0, vec![]);
            let (txid, lock_time, accepted_daa) = (tx.id(), Some(lock), self.daa);
            let spend = Spend { tx: tx.clone(), txid, lock_time, accepted_daa, blocks: vec![] };
            self.txs.push((self.coin.0, spend));
            let Some(n) = next else { return tx };
            let i = tx.outputs.iter().position(|o| o.covenant.is_some()).unwrap();
            let o = &tx.outputs[i];
            let cov = Some(self.v.covenant_id);
            let entry = UtxoEntry::new(o.value, o.script_public_key.clone(), self.daa, false, cov);
            self.coin = (TransactionOutpoint::new(tx.id(), i as u32), entry);
            self.v.state = n;
            tx
        }

        fn cont(&self, value: u64, next: &State) -> TransactionOutput {
            self.ctx.continuation(value, next, &self.v)
        }

        /// A card payment of `pay`, `gap` DAA after the last move, signed at DAA - 20 like the
        /// engine (commands.rs); the rest stays.
        fn spend(&mut self, pay: u64, gap: u64) -> Transaction {
            self.spend_locked(pay, gap, self.daa + gap - 20)
        }

        /// The same payment signed with lock time `lock` (a thief may pick any allowed value).
        fn spend_locked(&mut self, pay: u64, gap: u64, lock: u64) -> Transaction {
            self.daa += gap;
            let w = self.v.dials.window;
            let coin_daa = self.coin.1.block_daa_score;
            let n = after_spend(&self.v.state, w, coin_daa as i64, lock as i64, (pay + FEE) as i64);
            let kept = self.coin.1.amount - pay - FEE;
            let outs = vec![TransactionOutput::new(pay, payee()), self.cont(kept, &n)];
            self.spend_by("spend", vec![vec![0u8; 65]], outs, lock, Some(n))
        }

        /// The lowest lock time the rules let the next spend open a window at.
        fn low(&self) -> u64 {
            let (s, w) = (&self.v.state, self.v.dials.window);
            (s.window_start + w).max(self.coin.1.block_daa_score as i64 + w) as u64
        }

        /// `n` top-ups of 1 KAS, `LATER` DAA apart (the state never changes), built from one
        /// signature script and one continuation so a long padding stays quick to build.
        fn topups(&mut self, n: usize) {
            let (art, cov) = (self.ctx.compile(&self.v.state), Some(self.v.covenant_id));
            let c = art.contracts.values().next().unwrap();
            let tag = c.entries["topup"].dispatch_tag.as_bytes().to_vec();
            let mut b = ScriptBuilder::new();
            for push in [vec![0u8; 65], tag, contract::bytecode(&art)] {
                b.add_data(&push).unwrap();
            }
            let (sig, cont) = (b.drain(), self.cont(0, &self.v.state));
            for _ in 0..n {
                self.daa += LATER;
                let mut out = cont.clone();
                out.value = self.coin.1.amount + KAS;
                let inputs = vec![TransactionInput::new(self.coin.0, sig.clone(), 0, 0)];
                let outs = vec![out.clone()];
                let tx = Transaction::new(1, inputs, outs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
                let (txid, accepted_daa) = (tx.id(), self.daa);
                let spend = Spend { tx, txid, lock_time: Some(0), accepted_daa, blocks: vec![] };
                self.txs.push((self.coin.0, spend));
                let entry = UtxoEntry::new(out.value, out.script_public_key, self.daa, false, cov);
                self.coin = (TransactionOutpoint::new(txid, 0), entry);
            }
        }

        fn request(&mut self, amount: u64) -> Transaction {
            self.daa += LATER;
            let (dest, s) = (payee(), self.v.state.clone());
            let n = State { stage: 1, dest_hash: spk_hash(&dest), amount: amount as i64, ..s };
            let outs = vec![self.cont(self.coin.1.amount - FEE, &n)];
            let push = (amount as i64).to_le_bytes()[..4].to_vec();
            self.spend_by("request", vec![vec![0u8; 65], spk_bytes(&dest), push], outs, 0, Some(n))
        }

        fn finish(&mut self) -> Transaction {
            self.daa += LATER;
            let (amount, n) = (self.v.state.amount as u64, after_finish(&self.v.state));
            let kept = self.coin.1.amount - amount - FEE;
            let outs = vec![TransactionOutput::new(amount, payee()), self.cont(kept, &n)];
            self.spend_by("finish", vec![], outs, 0, Some(n))
        }

        fn cancel(&mut self) -> Transaction {
            self.daa += LATER;
            let cold = self.ctx.cold_spk();
            let outs = vec![TransactionOutput::new(self.coin.1.amount - FEE, cold)];
            self.spend_by("cancel", vec![vec![0u8; 65], vec![]], outs, 0, None)
        }

        /// The walk paper.rs runs: from the record as printed (Locked, the first coin).
        async fn walk(&self, max: usize) -> Result<Walk, String> {
            self.walk_with(Limits { max_hops: max, ..Limits::WALK }, None).await
        }

        /// The same walk within `limits`; with `node`, through `WithNode` (the history's
        /// `coins_at` is then ignored: only `node`'s coins count).
        async fn walk_with(&self, limits: Limits, node: Option<&FakeNode>)
            -> Result<Walk, String> {
            let at = |c: &Coin| self.ctx.network.address_of(&c.1.script_public_key).unwrap();
            let first = Some((at(&self.printed), self.printed.clone()));
            let coins = self.node.iter().map(|c| (at(c), c.clone())).collect();
            let (txs, hide_locks) = (self.txs.clone(), self.hide_locks);
            let fake = Fake { first, txs, hide_locks, coins, ..Fake::default() };
            let record = Vault { state: locked(), ..self.v.clone() };
            let public = self.ctx.keys.public();
            let Some(node) = node else {
                return walk_to_live_from_record(&public, &record, &fake, limits).await;
            };
            let with = WithNode::new(&fake, node);
            walk_to_live_from_record(&public, &record, &with, limits).await
        }
    }

    #[tokio::test]
    async fn a_record_three_moves_back_walks_to_the_live_coin() {
        let mut c = chain("three");
        c.spend(KAS, LATER);
        c.request(2 * KAS);
        let last = c.finish();
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert_eq!((w.hops, w.ended), (3, false));
        let (coin, v) = w.live.expect("live coin");
        assert_eq!(coin.0, TransactionOutpoint::new(last.id(), 1));
        assert_eq!((coin.1.amount, coin.1.block_daa_score), (last.outputs[1].value, c.daa));
        assert_eq!(coin.1.script_public_key, c.ctx.vault_spk(&c.v.state));
        assert_eq!(v.state, c.v.state, "the walk derives the state the engine wrote");
        assert_eq!(v.history.len(), 3, "{:?}", v.history);
        assert!(v.history[1].starts_with("request 2 KAS"), "{:?}", v.history);
        assert!(v.history[2].starts_with("finish 2 KAS"), "{:?}", v.history);
    }

    #[tokio::test]
    async fn a_stop_at_hop_two_ends_the_walk() {
        let mut c = chain("stop");
        c.spend(KAS, LATER);
        c.cancel();
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert!(w.live.is_none());
        assert_eq!((w.hops, w.ended), (2, true));
        assert!(contract::is_stopped(&w.last), "{:?}", w.last.history);
    }

    #[tokio::test]
    async fn a_forged_continuation_is_refused() {
        // a continuation script for another state
        let mut c = chain("forged-script");
        c.spend(KAS, LATER);
        let other = State { spent: 1, ..locked() };
        c.txs[0].1.tx.outputs[1].script_public_key = c.ctx.vault_spk(&other);
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
        // the right script, more kept than the window counter allows for
        let mut c = chain("forged-amount");
        c.spend(KAS, LATER);
        c.txs[0].1.tx.outputs[1].value += KAS;
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
        // a forged second hop stops the walk too
        let mut c = chain("forged-hop2");
        c.spend(KAS, LATER);
        c.spend(KAS, LATER);
        c.txs[1].1.tx.outputs[1].value -= 1;
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
        // a stop that pays somewhere other than cold
        let mut c = chain("forged-cancel");
        c.cancel();
        c.txs[0].1.tx.outputs[0].script_public_key = payee();
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
    }

    #[tokio::test]
    async fn an_unrelated_transaction_or_a_missing_coin_is_refused() {
        let mut c = chain("unrelated");
        let mut d = chain("unrelated-other");
        d.coin.0 = TransactionOutpoint::new(Hash::from_bytes([9u8; 32]), 0);
        d.spend(KAS, LATER);
        c.txs.push((c.printed.0, d.txs[0].1.clone()));
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
        let public = c.ctx.keys.public();
        let empty = Fake::default();
        let e = walk_to_live_from_record(&public, &c.v, &empty, Limits::WALK).await.unwrap_err();
        assert_eq!(e, NOT_FOUND);
        let down = Fake { fail: true, ..Fake::default() };
        let e = walk_to_live(&public, &c.v, &c.printed, &down, Limits::WALK).await.unwrap_err();
        assert_eq!(e, UNREADABLE);
    }

    #[tokio::test]
    async fn max_hops_is_enforced() {
        let mut c = chain("max");
        c.spend(KAS, LATER);
        c.spend(KAS, LATER);
        c.spend(KAS, LATER);
        let e = c.walk(2).await.unwrap_err();
        assert!(e.contains("longer than 2 moves") && e.contains("No funds were moved"), "{e}");
        assert_eq!(c.walk(3).await.unwrap().hops, 3);
    }

    #[tokio::test]
    async fn a_printed_coin_still_live_is_zero_hops() {
        let c = chain("live");
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert_eq!((w.hops, w.ended), (0, false));
        let (coin, v) = w.live.unwrap();
        assert_eq!((coin, v.state), (c.printed.clone(), locked()));
    }

    #[tokio::test]
    async fn withheld_lock_times_are_recovered_by_the_search() {
        let mut c = chain("hidden");
        c.hide_locks = true;
        c.spend(KAS / 4, LATER); // opens a window at an unknown lock time
        c.spend(KAS / 4, 100); // the same window: no unknown value
        c.spend(KAS / 4, LATER); // a new window again
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert_eq!(w.hops, 3);
        let (coin, v) = w.live.unwrap();
        assert_eq!((coin.0, v.state.clone()), (c.coin.0, c.v.state.clone()));
        assert_eq!(v.state.window_start, (c.daa - 20) as i64, "the lock time found is real");
    }

    #[tokio::test]
    async fn a_low_lock_time_far_below_the_accepting_daa_is_found() {
        // signed at (or above) the lowest lock time the rules allow, sent 250 000 DAA later: far
        // past the old 100 000-try cap (a debug build tries ~2 000 a second, so stay near the end)
        for above in [0, 10_000] {
            let mut c = chain(&format!("low-{above}"));
            c.hide_locks = true;
            let lock = c.low() + above;
            c.spend_locked(KAS, 250_000, lock);
            let w = c.walk(MAX_HOPS).await.unwrap();
            let (coin, v) = w.live.unwrap();
            assert_eq!((coin.0, v.state.clone()), (c.coin.0, c.v.state.clone()));
            assert_eq!(v.state.window_start, lock as i64, "the lock time found is real");
        }
    }

    #[tokio::test]
    async fn a_lying_lock_time_hint_falls_back_to_the_search() {
        // the card key signs the payload, so it can echo any value; a source can lie too
        for (hint, echo) in [(None, 12_345), (Some(12_345), u64::MAX), (Some(u64::MAX), 0)] {
            let mut c = chain(&format!("hint-{echo}"));
            c.spend_locked(KAS, 250_000, c.low() + 7);
            c.txs[0].1.lock_time = hint;
            c.txs[0].1.tx.payload = lock_time_payload(echo);
            let w = c.walk(MAX_HOPS).await.unwrap();
            assert_eq!(w.live.unwrap().1.state, c.v.state, "{hint:?} {echo}");
        }
        // a true echo is used as is
        let mut c = chain("hint-true");
        c.hide_locks = true;
        c.spend(KAS, LATER);
        c.txs[0].1.tx.payload = lock_time_payload(c.daa - 20);
        assert_eq!(c.walk(MAX_HOPS).await.unwrap().live.unwrap().1.state, c.v.state);
    }

    #[tokio::test]
    async fn a_lying_hint_and_no_matching_lock_time_is_refused() {
        let mut c = chain("no-match");
        c.spend(KAS, LATER);
        c.txs[0].1.lock_time = Some(12_345);
        c.txs[0].1.tx.payload = lock_time_payload(u64::MAX);
        // the accepting block reported before the real lock time: no allowed value reproduces it
        c.txs[0].1.accepted_daa -= 100;
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
        // the accepting block reported below the lowest allowed value: an empty range
        c.txs[0].1.accepted_daa = c.printed.1.block_daa_score;
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
    }

    #[tokio::test]
    async fn top_up_padding_collapses_through_the_node() {
        let mut c = chain("padding");
        c.spend(KAS, LATER);
        c.topups(600);
        // 50 hops, or the walk-wide time under a loaded test run: either way out of budget
        let e = c.walk(50).await.unwrap_err();
        assert!(too_long(&e) && e.contains("No funds were moved"), "{e}");
        // the node's live coin, beside a newer coin at the address with another covenant id
        let mut stray = c.coin.clone();
        stray.0 = TransactionOutpoint::new(Hash::from_bytes([8u8; 32]), 0);
        stray.1.covenant_id = Some(Hash::from_bytes([8u8; 32]));
        stray.1.block_daa_score += 1;
        c.node = vec![stray, c.coin.clone()];
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert_eq!(w.hops, 1, "one spend, then the jump to the live coin");
        let (coin, v) = w.live.unwrap();
        assert_eq!((coin.0, v.state), (c.coin.0, c.v.state.clone()));
    }

    #[tokio::test]
    async fn padding_before_a_move_collapses_when_the_source_shows_the_newest_coin() {
        let mut c = chain("padding-before");
        c.topups(600);
        let newest_locked = c.coin.clone();
        c.spend(KAS, LATER);
        c.node = vec![newest_locked];
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert_eq!(w.hops, 1, "a jump past the padding, then the spend");
        assert_eq!(w.live.unwrap().0.0, c.coin.0);
    }

    /// Our node as the walk sees it: its live coins, and a lock time it holds.
    #[derive(Default)]
    struct FakeNode {
        coins: Vec<(Address, Coin)>,
        lock: Option<u64>,
        down: bool,
    }

    impl NodeView for FakeNode {
        fn live_coins<'a>(&'a self, address: &'a Address)
            -> BoxFuture<'a, Result<Vec<Coin>, String>> {
            let hits = self.coins.iter().filter(|(a, _)| a == address).map(|(_, c)| c.clone());
            futures::future::ready(Ok(hits.collect())).boxed()
        }

        fn lock_time<'a>(&'a self, _txid: Hash, _blocks: &'a [Hash])
            -> BoxFuture<'a, Result<Option<u64>, String>> {
            let answer = if self.down { Err("node down".into()) } else { Ok(self.lock) };
            futures::future::ready(answer).boxed()
        }
    }

    #[test]
    fn lock_in_reads_only_the_matching_transaction() {
        use kaspa_consensus_core::header::Header;
        use kaspa_rpc_core::{RpcBlock, RpcHeader, RpcTransaction, RpcTransactionVerboseData};
        let rpc_tx = |id: u8, lock: u64| {
            let tx = Transaction::new(1, vec![], vec![], lock, SUBNETWORK_ID_NATIVE, 0, vec![]);
            let mut t = RpcTransaction::from(&tx);
            let h = Hash::from_bytes([id; 32]);
            t.verbose_data = Some(RpcTransactionVerboseData {
                transaction_id: h, hash: h, compute_mass: 0, block_hash: h, block_time: 0,
            });
            t
        };
        let bare = Header::from_precomputed_hash(Hash::from_bytes([9; 32]), vec![]);
        let header = RpcHeader::from(&bare);
        let transactions = vec![rpc_tx(1, 111), rpc_tx(2, 222)];
        let block = RpcBlock { header, transactions, verbose_data: None };
        assert_eq!(lock_in(&block, Hash::from_bytes([2; 32])), Some(222));
        assert_eq!(lock_in(&block, Hash::from_bytes([3; 32])), None);
    }

    #[tokio::test]
    async fn a_node_error_falls_back_to_the_history_lock_time() {
        let mut c = chain("nodedown");
        c.spend(KAS, LATER);
        let (op, s) = c.txs[0].clone();
        let fake = Fake { txs: c.txs.clone(), ..Fake::default() };
        let node = FakeNode { down: true, lock: Some(1), ..FakeNode::default() };
        let with = WithNode::new(&fake, &node);
        assert_eq!(with.spender(&op).await.unwrap().unwrap().lock_time, s.lock_time);
        assert!(with.warned.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn endless_valid_looking_history_stops_at_the_walk_budget() {
        let mut c = chain("endless");
        c.topups(40);
        let few = Limits { calls: 12, ..Limits::WALK };
        assert_eq!(c.walk_with(few, None).await.unwrap_err(), TOO_LONG);
        let no_time = Limits { time: Duration::ZERO, ..Limits::WALK };
        assert_eq!(c.walk_with(no_time, None).await.unwrap_err(), TOO_LONG);
        assert!(too_long(TOO_LONG) && too_long(&c.walk(5).await.unwrap_err()));
        // the production budget walks 40 hops (cached compile: ~1.3 s release, a few s debug)
        let t0 = std::time::Instant::now();
        assert_eq!(c.walk_with(Limits::WALK, None).await.unwrap().hops, 40);
        eprintln!("walk40 took {:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn a_coin_only_the_history_shows_is_never_taken_as_live() {
        let mut c = chain("hint");
        let mut forged = c.printed.clone();
        forged.0 = TransactionOutpoint::new(Hash::from_bytes([7; 32]), 0);
        forged.1.block_daa_score += 1;
        c.node = vec![forged.clone()];
        // a bare history source would be believed ...
        let w = c.walk(MAX_HOPS).await.unwrap();
        assert_eq!(w.live.unwrap().0.0, forged.0);
        // ... through WithNode only our node's coins count
        let w = c.walk_with(Limits::WALK, Some(&FakeNode::default())).await.unwrap();
        assert_eq!((w.live.unwrap().0.0, w.hops), (c.printed.0, 0));
    }

    #[tokio::test]
    async fn our_nodes_lock_time_is_preferred() {
        let mut c = chain("nodelock");
        c.spend(KAS, LATER);
        let (op, s) = c.txs[0].clone();
        let fake = Fake { txs: c.txs.clone(), hide_locks: true, ..Fake::default() };
        let node = FakeNode { lock: s.lock_time, ..FakeNode::default() };
        let with = WithNode::new(&fake, &node);
        assert!(s.lock_time.is_some());
        assert_eq!(with.spender(&op).await.unwrap().unwrap().lock_time, s.lock_time);
        assert_eq!(fake.spender(&op).await.unwrap().unwrap().lock_time, None);
    }

    #[tokio::test]
    async fn a_spend_accepted_before_its_coin_is_refused() {
        let mut c = chain("early");
        c.spend(KAS, LATER);
        assert!(c.printed.1.block_daa_score > 0);
        c.txs[0].1.accepted_daa = c.printed.1.block_daa_score - 1;
        assert_eq!(c.walk(MAX_HOPS).await.unwrap_err(), CHECK_FAILED);
    }

    #[test]
    fn the_lock_search_rate_is_measured() {
        let c = chain("rate");
        let public = c.ctx.keys.public();
        let rules = Rules::new(&public, &c.v).unwrap();
        let mut t = Template::new(&rules, &c.v, &c.v.state).unwrap();
        let want = rules.spk(&c.v, &c.v.state);
        let target = script_hash(&want).unwrap();
        assert_eq!(t.hash(c.v.state.window_start).unwrap().as_bytes(), target, "the same digest");
        let (tries, start) = (20_000u32, Instant::now());
        for k in 0..tries {
            assert_ne!(t.hash(-1 - i64::from(k)).unwrap().as_bytes(), target);
        }
        let per = start.elapsed() / tries;
        let n = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS);
        let cover = LOCK_BUDGET.as_nanos() / per.as_nanos().max(1) * n as u128;
        eprintln!("lock search: {per:?} a try, {n} threads, ~{cover} values in {LOCK_BUDGET:?}");
        assert!(cover > 100_000, "even a debug build beats the old 100 000-try cap: {cover}");
    }

    #[test]
    fn hostile_pushes_read_as_none_not_panic() {
        assert_eq!(pushes(&[0x4e, 0xff, 0xff, 0xff, 0xff]), None);
        assert_eq!(pushes(&[0x02, 0xaa]), None);
        let small = Some(vec![vec![7], vec![], vec![1], vec![0x81]]);
        assert_eq!(pushes(&[0x01, 0x07, 0x00, 0x51, 0x4f]), small);
        assert_eq!(pushes(&[0x01, 0x07, 0xac]), None);
        assert_eq!(script_int(&[0xe8, 0x03]), Some(1000));
        assert_eq!(script_int(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), None);
        assert!(decode_spk(&[0]).is_none());
    }
}
