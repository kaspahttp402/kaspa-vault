//! History client for the recovery lineage walk (lineage.rs `SpenderSource`): reads the public
//! Kaspa REST indexer the way docs/paper-stop-indexer.md lays out (call B to find a spender,
//! call C for the accepting block's DAA score, call A to place a coin this client did not see).
//! Untrusted and read-only: the walk checks every answer. Only addresses, transaction ids and
//! block hashes are sent, and only to a fixed allow-list of hosts (https only, no proxy, no
//! redirects, no cookies, a generic User-Agent).
//!
//! The indexer has no "spent by" call: a coin's spender is found by paging the transactions of
//! the coin's address from shortly before the coin's block time and matching an input's previous
//! outpoint (never the address: accounts share addresses). Only accepted transactions count. The
//! indexer keeps no lock time, so every `Spend` carries `lock_time: None`.

use crate::contract::Coin;
use crate::lineage::{Spend, SpenderSource};
use crate::network::Network;
use futures::FutureExt;
use futures::future::BoxFuture;
use kaspa_addresses::Address;
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::subnets::{SUBNETWORK_ID_NATIVE, SubnetworkId};
use kaspa_consensus_core::tx::{CovenantBinding, ScriptPublicKey, Transaction, TransactionInput};
use kaspa_consensus_core::tx::{TransactionOutpoint, TransactionOutput, UtxoEntry};
use kaspa_txscript::pay_to_address_script;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

/// Mainnet history service (full history today, nothing promised).
pub const MAINNET_BASE: &str = "https://api.kaspa.org";
/// Testnet-10 history service (about 7 days, rolling).
pub const TESTNET_BASE: &str = "https://api-tn10.kaspa.org";
/// Per request, connecting included.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Most pages one lookup reads.
pub const MAX_PAGES: usize = 40;
/// Largest answer body read.
pub const MAX_BODY: usize = 2 * 1024 * 1024;
/// Requests and answer bytes one Indexer may spend (paper.rs makes one per walk).
pub const MAX_REQUESTS: usize = 600;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// The request or byte budget ran out (lineage.rs reads it as too long to follow).
pub const OVER_BUDGET: &str = "the history service budget for one look ran out";
/// Blocks kept per transaction (where our node is asked for its copy).
const MAX_BLOCKS: usize = 4;
/// Retries after a failed request (network error, 429, 5xx), with backoff.
pub const RETRIES: u32 = 2;
/// Transactions per page.
const PAGE_LIMIT: u32 = 50;
/// DAG block times arrive out of order: look from this long before the coin's block time.
const MARGIN_MS: u64 = 600_000;
const USER_AGENT: &str = "kaspa-rest-client/1.0";
/// Coins remembered (address and block time); cleared when full.
const MAX_SEEN: usize = 4096;

pub const TOO_BIG: &str = "the history service sent too large an answer";
pub const TOO_LONG: &str = "the coin's history is longer than the pages read";
pub const UNSETTLED: &str = "a move of the account has not settled yet; try again in a minute";
pub const TWO_SPENDERS: &str = "the history service shows two spenders of one coin";
pub const NOT_INDEXED: &str =
    "the history service does not know the coin (older than its history?)";
const STUCK: &str = "the history service's pages did not move on";

/// The service for the engine's network.
pub fn base_for(network: &Network) -> &'static str {
    if network.is_mainnet() { MAINNET_BASE } else { TESTNET_BASE }
}

/// The allow-listed base URL equal to `base`, or a refusal (these two https origins only).
pub fn allowed(base: &str) -> Result<&'static str, String> {
    let hit = [MAINNET_BASE, TESTNET_BASE].into_iter().find(|b| *b == base);
    hit.ok_or_else(|| format!("{base} is not an allowed history service"))
}

/// The public indexer as a `SpenderSource`.
pub struct Indexer {
    fetch: Box<dyn Fetch>,
    /// Coin -> its address and block time, from earlier answers: where its spender is listed.
    seen: Mutex<HashMap<TransactionOutpoint, Seen>>,
    /// Requests made and answer bytes read so far.
    used: Mutex<(usize, usize)>,
}

#[derive(Clone, Debug)]
struct Seen {
    address: String,
    time: u64,
}

/// Where a page starts: after a block time (upward), or before one (downward; None: newest).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dir {
    After(u64),
    Before(Option<u64>),
}

struct Page {
    value: Value,
    after: Option<u64>,
    before: Option<u64>,
}

/// What a scan found: the matching accepted transactions of one page, and whether a matching
/// transaction that is not (yet) accepted was seen.
struct Found {
    accepted: Vec<Listed>,
    pending: bool,
}

impl Indexer {
    /// The client for `network`'s allow-listed service.
    pub fn new(network: &Network) -> Result<Indexer, String> {
        Indexer::with_base(base_for(network))
    }

    /// The client for `base`, which must be one of the allow-listed services.
    pub fn with_base(base: &str) -> Result<Indexer, String> {
        let base = allowed(base)?;
        Ok(Indexer::over(Box::new(Http::new(base)?)))
    }

    fn over(fetch: Box<dyn Fetch>) -> Indexer {
        Indexer { fetch, seen: Mutex::new(HashMap::new()), used: Mutex::new((0, 0)) }
    }

    fn remember(&self, op: TransactionOutpoint, seen: Seen) {
        let mut map = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() >= MAX_SEEN {
            map.clear();
        }
        map.insert(op, seen);
    }

    /// Counts requests and bytes against the budget: past it, OVER_BUDGET.
    fn charge(&self, requests: usize, bytes: usize) -> Result<(), String> {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        *used = (used.0.saturating_add(requests), used.1.saturating_add(bytes));
        let over = used.0 > MAX_REQUESTS || used.1 > MAX_BYTES;
        if over { Err(OVER_BUDGET.into()) } else { Ok(()) }
    }

    fn recall(&self, op: &TransactionOutpoint) -> Option<Seen> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).get(op).cloned()
    }

    /// GET and parse; None on 404 (unknown id).
    async fn json(&self, path: &str) -> Result<Option<Page>, String> {
        safe_path(path)?;
        self.charge(1, 0)?;
        let r = self.fetch.get(path).await?;
        self.charge(0, r.body.len())?;
        match r.status {
            200 => {}
            404 => return Ok(None),
            s => return Err(format!("the history service answered {s}")),
        }
        let value = serde_json::from_slice(&r.body).map_err(|_| "unreadable answer".to_string())?;
        Ok(Some(Page { value, after: r.after, before: r.before }))
    }

    /// Call C: the accepting block's DAA score (the new coin's UTXO DAA score).
    async fn daa(&self, block: Option<Hash>) -> Result<u64, String> {
        let block = block.ok_or("an accepted transaction names no accepting block")?;
        let path = format!("/blocks/{block}?includeTransactions=false");
        let page = self.json(&path).await?.ok_or("the accepting block is unknown")?;
        num(page.value.pointer("/header/daaScore")).ok_or_else(|| "no DAA score".to_string())
    }

    /// Where `op`'s spender is listed: remembered, or call A on its transaction.
    async fn locate(&self, op: &TransactionOutpoint) -> Result<Seen, String> {
        if let Some(s) = self.recall(op) {
            return Ok(s);
        }
        let q = "inputs=false&outputs=true&resolve_previous_outpoints=no";
        let id = op.transaction_id;
        let page = self.json(&format!("/transactions/{id}?{q}")).await?.ok_or(NOT_INDEXED)?;
        let l = parse_tx(&page.value)?;
        let i = usize::try_from(op.index).map_err(|_| NOT_INDEXED)?;
        let address = l.addresses.get(i).cloned().flatten();
        match address {
            Some(address) if l.txid == id => Ok(Seen { address, time: l.block_time }),
            _ => Err(NOT_INDEXED.into()),
        }
    }

    /// Call B, paged: the matching transactions of the first page holding an accepted match.
    async fn scan<F>(&self, address: &str, mut dir: Dir, want: F) -> Result<Found, String>
    where
        F: Fn(&Listed) -> bool + Send + Sync,
    {
        Address::try_from(address).map_err(|_| "the history service named a bad address")?;
        let mut pending = false;
        for _ in 0..MAX_PAGES {
            let Some(page) = self.json(&page_path(address, dir)).await? else {
                return Ok(Found { accepted: Vec::new(), pending });
            };
            let hits = parse_page(&page.value)?.into_iter().filter(|l| want(l));
            let (accepted, waiting): (Vec<Listed>, Vec<Listed>) = hits.partition(|l| l.accepted);
            pending |= !waiting.is_empty();
            match next_dir(dir, &page)? {
                Some(d) if accepted.is_empty() => dir = d,
                _ => return Ok(Found { accepted, pending }),
            }
        }
        Err(TOO_LONG.into())
    }

    async fn find_spender(&self, op: &TransactionOutpoint) -> Result<Option<Spend>, String> {
        let seen = self.locate(op).await?;
        let from = Dir::After(seen.time.saturating_sub(MARGIN_MS).max(1));
        let spends = |l: &Listed| l.tx.inputs.iter().any(|i| i.previous_outpoint == *op);
        let found = self.scan(&seen.address, from, spends).await?;
        let Some(first) = found.accepted.first() else {
            return if found.pending { Err(UNSETTLED.into()) } else { Ok(None) };
        };
        if found.accepted.iter().any(|l| l.txid != first.txid) {
            return Err(TWO_SPENDERS.into());
        }
        self.spend_of(first).await.map(Some)
    }

    /// The spend as the walk takes it; its covenant outputs are remembered for the next hop.
    async fn spend_of(&self, l: &Listed) -> Result<Spend, String> {
        let accepted_daa = self.daa(l.accepting_block).await?;
        let outs = l.tx.outputs.iter().zip(&l.addresses).enumerate();
        for (i, (_, address)) in outs.filter(|(_, (o, _))| o.covenant.is_some()) {
            let (Ok(index), Some(address)) = (u32::try_from(i), address) else { continue };
            let seen = Seen { address: address.clone(), time: l.block_time };
            self.remember(TransactionOutpoint::new(l.txid, index), seen);
        }
        let (tx, blocks) = (l.tx.clone(), l.blocks.clone());
        Ok(Spend { tx, txid: l.txid, lock_time: None, accepted_daa, blocks })
    }

    /// The newest accepted coin carrying `cov` paid to `address` (matched on its script).
    async fn find_first(&self, address: &Address, cov: Hash) -> Result<Option<Coin>, String> {
        let spk = pay_to_address_script(address);
        let ours = |o: &TransactionOutput| {
            o.script_public_key == spk && o.covenant.is_some_and(|c| c.covenant_id == cov)
        };
        let addr = address.to_string();
        let found = self.scan(&addr, Dir::Before(None), |l| l.tx.outputs.iter().any(ours)).await?;
        let Some(l) = found.accepted.first() else { return Ok(None) };
        let Some((i, o)) = l.tx.outputs.iter().enumerate().find(|(_, o)| ours(o)) else {
            return Ok(None);
        };
        let index = u32::try_from(i).map_err(|_| "too many outputs")?;
        let op = TransactionOutpoint::new(l.txid, index);
        let daa = self.daa(l.accepting_block).await?;
        self.remember(op, Seen { address: addr, time: l.block_time });
        let entry = UtxoEntry::new(o.value, o.script_public_key.clone(), daa, false, Some(cov));
        Ok(Some((op, entry)))
    }
}

impl SpenderSource for Indexer {
    fn first_coin<'a>(
        &'a self,
        address: &'a Address,
        covenant_id: Hash,
    ) -> BoxFuture<'a, Result<Option<Coin>, String>> {
        self.find_first(address, covenant_id).boxed()
    }

    fn spender<'a>(
        &'a self,
        outpoint: &'a TransactionOutpoint,
    ) -> BoxFuture<'a, Result<Option<Spend>, String>> {
        self.find_spender(outpoint).boxed()
    }
}

fn page_path(address: &str, dir: Dir) -> String {
    let at = match dir {
        Dir::After(t) => format!("&after={t}"),
        Dir::Before(Some(t)) => format!("&before={t}"),
        Dir::Before(None) => String::new(),
    };
    let q = "resolve_previous_outpoints=no";
    format!("/addresses/{address}/full-transactions-page?limit={PAGE_LIMIT}{at}&{q}")
}

/// The next page's position, None at the end; a position that does not move on is refused.
fn next_dir(dir: Dir, page: &Page) -> Result<Option<Dir>, String> {
    let next = match dir {
        Dir::After(t) => page.after.map(|n| (n > t, Dir::After(n))),
        Dir::Before(t) => page.before.map(|n| (t.is_none_or(|t| n < t), Dir::Before(Some(n)))),
    };
    match next {
        Some((true, d)) => Ok(Some(d)),
        Some((false, _)) => Err(STUCK.into()),
        None => Ok(None),
    }
}

/// Paths carry only addresses, hex ids and fixed query words.
fn safe_path(path: &str) -> Result<(), String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || "/:?&=_-".contains(c);
    let fine = path.starts_with('/') && !path.starts_with("//") && path.len() <= 512;
    if fine && path.chars().all(ok) { Ok(()) } else { Err("refused a malformed request".into()) }
}

// ---- parsing (the indexer sends some numbers as strings: parse tolerantly) ----

/// One transaction as the indexer lists it: what the walk reads, and where each output went.
struct Listed {
    txid: Hash,
    accepted: bool,
    block_time: u64,
    accepting_block: Option<Hash>,
    /// The blocks holding the transaction, as the indexer names them (hints for our node).
    blocks: Vec<Hash>,
    /// Inputs and outputs in index order; lock time 0 and gas 0 (not indexed), so `tx.id()` is
    /// not the real id: use `txid`.
    tx: Transaction,
    /// Each output's address as the indexer names it.
    addresses: Vec<Option<String>>,
}

fn parse_page(v: &Value) -> Result<Vec<Listed>, String> {
    v.as_array().ok_or("the history page is not a list")?.iter().map(parse_tx).collect()
}

fn parse_tx(v: &Value) -> Result<Listed, String> {
    let ins: Result<Vec<_>, String> = items(v, "inputs")?.iter().map(parse_input).collect();
    let outs: Result<Vec<_>, String> = items(v, "outputs")?.iter().map(parse_output).collect();
    let (outputs, addresses): (Vec<_>, Vec<_>) = in_order(outs?)?.into_iter().unzip();
    let subnet = v.get("subnetwork_id").and_then(Value::as_str);
    let subnet = subnet.and_then(|s| SubnetworkId::from_str(s).ok());
    let subnet = subnet.unwrap_or(SUBNETWORK_ID_NATIVE);
    let version = u16::try_from(num(v.get("version")).unwrap_or(0)).map_err(|_| "bad version")?;
    let payload = bytes_of(v.get("payload"))?;
    let tx = Transaction::new(version, in_order(ins?)?, outputs, 0, subnet, 0, payload);
    let accepting = v.get("accepting_block_hash").and_then(Value::as_str);
    Ok(Listed {
        txid: hash_of(v.get("transaction_id"))?,
        accepted: v.get("is_accepted").and_then(Value::as_bool).unwrap_or(false),
        block_time: num(v.get("block_time")).unwrap_or(0),
        accepting_block: accepting.and_then(|s| parse_hash(s).ok()),
        blocks: block_hashes(v.get("block_hash")),
        tx,
        addresses,
    })
}

/// The blocks the indexer says hold a transaction: at most `MAX_BLOCKS`, unreadable ones
/// skipped.
fn block_hashes(v: Option<&Value>) -> Vec<Hash> {
    let all = v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let hashes = all.iter().take(MAX_BLOCKS).filter_map(Value::as_str);
    hashes.filter_map(|s| parse_hash(s).ok()).collect()
}

fn parse_input(v: &Value) -> Result<(u64, TransactionInput), String> {
    let index = num(v.get("index")).ok_or("an input has no index")?;
    let prev_index = num(v.get("previous_outpoint_index")).ok_or("an input has no outpoint")?;
    let prev_index = u32::try_from(prev_index).map_err(|_| "outpoint index out of range")?;
    let prev = TransactionOutpoint::new(hash_of(v.get("previous_outpoint_hash"))?, prev_index);
    let script = bytes_of(v.get("signature_script"))?;
    let budget = num(v.get("compute_budget")).and_then(|b| u16::try_from(b).ok());
    let input = match budget.filter(|b| *b > 0) {
        Some(b) => TransactionInput::new_with_compute_budget(prev, script, 0, b),
        None => {
            let ops = u8::try_from(num(v.get("sig_op_count")).unwrap_or(0)).unwrap_or(u8::MAX);
            TransactionInput::new(prev, script, 0, ops)
        }
    };
    Ok((index, input))
}

type Out = (TransactionOutput, Option<String>);

fn parse_output(v: &Value) -> Result<(u64, Out), String> {
    let index = num(v.get("index")).ok_or("an output has no index")?;
    let amount = num(v.get("amount")).ok_or("an output has no amount")?;
    let spk = ScriptPublicKey::from_vec(0, bytes_of(v.get("script_public_key"))?);
    let covenant = match v.get("covenant_id").and_then(Value::as_str) {
        None => None,
        Some(id) => {
            let auth = num(v.get("covenant_authorizing_input")).ok_or("no authorizing input")?;
            let auth = u16::try_from(auth).map_err(|_| "authorizing input out of range")?;
            Some(CovenantBinding::new(auth, parse_hash(id)?))
        }
    };
    let address = v.get("script_public_key_address").and_then(Value::as_str).map(String::from);
    Ok((index, (TransactionOutput::with_covenant(amount, spk, covenant), address)))
}

/// Items sorted by their index, which must run 0, 1, 2...
fn in_order<T>(mut items: Vec<(u64, T)>) -> Result<Vec<T>, String> {
    items.sort_by_key(|(i, _)| *i);
    if items.iter().enumerate().any(|(k, (i, _))| u64::try_from(k).ok() != Some(*i)) {
        return Err("a transaction's inputs or outputs are not numbered in order".into());
    }
    Ok(items.into_iter().map(|(_, t)| t).collect())
}

fn items<'v>(v: &'v Value, key: &str) -> Result<&'v [Value], String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(format!("{key} is not a list")),
    }
}

/// A count the indexer sends as a number or as a decimal string.
fn num(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn hash_of(v: Option<&Value>) -> Result<Hash, String> {
    parse_hash(v.and_then(Value::as_str).ok_or("a transaction id is missing")?)
}

fn parse_hash(s: &str) -> Result<Hash, String> {
    let mut b = [0u8; 32];
    if s.len() != 64 || faster_hex::hex_decode(s.as_bytes(), &mut b).is_err() {
        return Err("a hash is not 64 hex digits".into());
    }
    Ok(Hash::from_bytes(b))
}

/// Hex bytes; a missing or null field is empty.
fn bytes_of(v: Option<&Value>) -> Result<Vec<u8>, String> {
    let Some(s) = v.and_then(Value::as_str) else { return Ok(Vec::new()) };
    let mut out = vec![0u8; s.len() / 2];
    if s.len() % 2 != 0 || faster_hex::hex_decode(s.as_bytes(), &mut out).is_err() {
        return Err("a hex field is malformed".into());
    }
    Ok(out)
}

/// GET one small JSON answer (the market figures) from `network`'s allow-listed service, with the
/// same client rules as the walk (https only, no redirects, time and size bounds); None on 404.
pub(crate) async fn get_json(network: &Network, path: &str) -> Result<Option<Value>, String> {
    safe_path(path)?;
    let r = Http::new(base_for(network))?.retrying(path).await?;
    match r.status {
        200 => {}
        404 => return Ok(None),
        s => return Err(format!("the history service answered {s}")),
    }
    serde_json::from_slice(&r.body).map(Some).map_err(|_| "unreadable answer".to_string())
}

// ---- transport ----

/// One answer: HTTP status, body (at most `MAX_BODY`) and the paging headers.
#[derive(Clone, Debug)]
struct Reply {
    status: u16,
    body: Vec<u8>,
    /// `X-Next-Page-After` / `X-Next-Page-Before`, when they are plain numbers.
    after: Option<u64>,
    before: Option<u64>,
}

/// GET of a path under the service's base URL.
trait Fetch: Send + Sync {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Reply, String>>;
}

struct Http {
    client: reqwest::Client,
    base: &'static str,
}

impl Http {
    fn new(base: &'static str) -> Result<Http, String> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .referer(false)
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| format!("could not start the history client: {e}"))?;
        Ok(Http { client, base })
    }

    async fn retrying(&self, path: &str) -> Result<Reply, String> {
        let mut last = String::new();
        for attempt in 0..=RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * 3u64.pow(attempt - 1))).await;
            }
            match self.once(path).await {
                Ok(r) if retryable(r.status) => last = format!("status {}", r.status),
                Ok(r) => return Ok(r),
                Err((true, e)) => last = e,
                Err((false, e)) => return Err(e),
            }
        }
        Err(format!("the history service did not answer ({last})"))
    }

    /// One request; the error says whether a retry may help.
    async fn once(&self, path: &str) -> Result<Reply, (bool, String)> {
        let url = reqwest::Url::parse(&format!("{}{path}", self.base));
        let url = url.map_err(|_| (false, "refused a malformed request".to_string()))?;
        check_url(&url).map_err(|e| (false, e))?;
        let sent = self.client.get(url).header(reqwest::header::ACCEPT, "application/json");
        let resp = sent.send().await.map_err(|e| (true, e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let after = header_num(resp.headers(), "x-next-page-after");
        let before = header_num(resp.headers(), "x-next-page-before");
        let body = capped(resp).await?;
        Ok(Reply { status, body, after, before })
    }
}

impl Fetch for Http {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Reply, String>> {
        self.retrying(path).boxed()
    }
}

fn retryable(status: u16) -> bool {
    status == 429 || status >= 500
}

/// The final URL must still be an allow-listed origin: https, default port, no credentials.
fn check_url(url: &reqwest::Url) -> Result<(), String> {
    let origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    let plain = url.port().is_none() && url.username().is_empty() && url.password().is_none();
    if plain { allowed(&origin).map(|_| ()) } else { Err(format!("{origin} is not allowed")) }
}

fn header_num(h: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    h.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// The body, refused past `MAX_BODY` (by its stated length, or as it streams in).
async fn capped(mut resp: reqwest::Response) -> Result<Vec<u8>, (bool, String)> {
    if too_big(resp.content_length()) {
        return Err((false, TOO_BIG.into()));
    }
    let mut buf = Vec::new();
    for _ in 0..=MAX_BODY {
        match resp.chunk().await {
            Ok(Some(c)) => grow(&mut buf, &c).map_err(|e| (false, e))?,
            Ok(None) => return Ok(buf),
            Err(e) => return Err((true, e.without_url().to_string())),
        }
    }
    Err((false, TOO_BIG.into()))
}

fn too_big(stated: Option<u64>) -> bool {
    stated.is_some_and(|n| n > MAX_BODY as u64)
}

fn grow(buf: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if buf.len().saturating_add(chunk.len()) > MAX_BODY {
        return Err(TOO_BIG.into());
    }
    buf.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaspa_addresses::Prefix;
    use kaspa_txscript::extract_script_pub_key_address;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    type Route = Box<dyn Fn(&str) -> Reply + Send + Sync>;
    type Calls = Arc<Mutex<Vec<String>>>;

    /// A fake service answering from fixtures and logging every path asked.
    struct Fake {
        answer: Route,
        calls: Calls,
    }

    impl Fetch for Fake {
        fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Reply, String>> {
            self.calls.lock().unwrap().push(path.to_string());
            futures::future::ready(Ok((self.answer)(path))).boxed()
        }
    }

    fn over(answer: Route) -> (Indexer, Calls) {
        let calls = Calls::default();
        (Indexer::over(Box::new(Fake { answer, calls: calls.clone() })), calls)
    }

    fn ok(v: Value) -> Reply {
        Reply { status: 200, body: v.to_string().into_bytes(), after: None, before: None }
    }

    fn h(n: u8) -> Hash {
        Hash::from_bytes([n; 32])
    }

    /// A P2SH script like the vault's (`aa20<hash>87`, as the indexer prints it).
    fn p2sh(n: u8) -> ScriptPublicKey {
        let mut s = vec![0xaa, 0x20];
        s.extend([n; 32]);
        s.push(0x87);
        ScriptPublicKey::from_vec(0, s)
    }

    fn addr(spk: &ScriptPublicKey) -> String {
        extract_script_pub_key_address(spk, Prefix::Testnet).unwrap().to_string()
    }

    const COV: u8 = 0xc0;
    const DAA: u64 = 616_433_484;

    // Fixtures shaped like the doc's TxModel / TxInput / TxOutput (paper-stop-indexer.md, A).
    fn output(tx: Hash, k: u32, amount: u64, spk: &ScriptPublicKey, cov: Option<Hash>) -> Value {
        json!({
            "transaction_id": tx.to_string(), "index": k, "amount": amount,
            "script_public_key": faster_hex::hex_string(spk.script()),
            "script_public_key_address": addr(spk), "script_public_key_type": "scripthash",
            "covenant_authorizing_input": cov.map(|_| 0),
            "covenant_id": cov.map(|c| c.to_string()),
        })
    }

    fn input(tx: Hash, prev: Hash, idx: u32) -> Value {
        json!({
            "transaction_id": tx.to_string(), "index": 0,
            "previous_outpoint_hash": prev.to_string(), "previous_outpoint_index": idx.to_string(),
            "previous_outpoint_address": null, "previous_outpoint_amount": null,
            "signature_script": "41420c", "sig_op_count": 1, "compute_budget": null,
            "covenant_id": null,
        })
    }

    fn tx(id: Hash, accepted: bool, time: u64, ins: Vec<Value>, outs: Vec<Value>) -> Value {
        json!({
            "transaction_id": id.to_string(), "hash": id.to_string(),
            "subnetwork_id": "0000000000000000000000000000000000000000", "mass": "2036",
            "payload": "", "block_hash": [h(9).to_string()], "block_time": time, "version": 0,
            "is_accepted": accepted, "accepting_block_hash": h(0xbb).to_string(),
            "accepting_block_blue_score": 1000, "accepting_block_time": time,
            "inputs": ins, "outputs": outs,
        })
    }

    /// Fund F pays the account at A (output 0). S spends F:0 and continues at B (output 1,
    /// listed first) and pays someone at output 0. X, at A too, spends another outpoint of F.
    fn history() -> (Value, Value, Value) {
        let (f, s, x) = (h(0xf1), h(0x51), h(0x77));
        let cont = output(f, 0, 500_000_000, &p2sh(1), Some(h(COV)));
        let fund = tx(f, true, 1_000_000, vec![], vec![cont]);
        let outs = vec![
            output(s, 1, 400_000_000, &p2sh(2), Some(h(COV))),
            output(s, 0, 90_000_000, &p2sh(3), None),
        ];
        let spend = tx(s, true, 2_000_000, vec![input(s, f, 0)], outs);
        let other = tx(x, true, 2_500_000, vec![input(x, f, 1)], vec![]);
        (fund, spend, other)
    }

    /// Pages for addresses A and B, blocks with a DAA score, 404 for anything else.
    fn service(page_a: Vec<Value>, page_b: Vec<Value>) -> Route {
        let (a, b) = (addr(&p2sh(1)), addr(&p2sh(2)));
        Box::new(move |path: &str| {
            if path.starts_with("/blocks/") {
                ok(json!({"header": {"daaScore": DAA.to_string()}}))
            } else if path.contains(&a) {
                ok(Value::from(page_a.clone()))
            } else if path.contains(&b) {
                ok(Value::from(page_b.clone()))
            } else {
                Reply { status: 404, body: b"{}".to_vec(), after: None, before: None }
            }
        })
    }

    fn account() -> Address {
        Address::try_from(addr(&p2sh(1)).as_str()).unwrap()
    }

    #[test]
    fn parses_doc_shaped_transactions() {
        let (_, spend, _) = history();
        let l = parse_tx(&spend).unwrap();
        assert_eq!((l.txid, l.accepted, l.block_time), (h(0x51), true, 2_000_000));
        assert_eq!(l.accepting_block, Some(h(0xbb)));
        let i = &l.tx.inputs[0];
        assert_eq!(i.previous_outpoint, TransactionOutpoint::new(h(0xf1), 0));
        assert_eq!(i.signature_script, vec![0x41, 0x42, 0x0c]);
        let o = &l.tx.outputs;
        assert_eq!((o[0].value, o[0].covenant), (90_000_000, None));
        assert_eq!(o[1].covenant, Some(CovenantBinding::new(0, h(COV))));
        assert_eq!((o[1].value, &o[1].script_public_key), (400_000_000, &p2sh(2)));
        assert_eq!(l.addresses[1], Some(addr(&p2sh(2))));
        assert_eq!(l.tx.lock_time, 0);
        assert_eq!(num(Some(&json!("616433484"))), Some(DAA));
        let mut gap = spend.clone();
        gap["outputs"][0]["index"] = json!(5);
        assert!(parse_tx(&gap).is_err(), "outputs must be numbered 0, 1, ...");
        let mut bad = spend;
        bad["inputs"][0]["previous_outpoint_hash"] = json!("zz");
        assert!(parse_tx(&bad).is_err());
        assert!(parse_page(&json!({"detail": "x"})).is_err());
    }

    #[tokio::test]
    async fn finds_the_coin_and_its_spender_by_outpoint() {
        let (fund, spend, other) = history();
        let (ix, calls) = over(service(vec![other, spend.clone(), fund], vec![spend]));
        let (op, entry) = ix.first_coin(&account(), h(COV)).await.unwrap().unwrap();
        assert_eq!(op, TransactionOutpoint::new(h(0xf1), 0));
        assert_eq!((entry.amount, entry.block_daa_score), (500_000_000, DAA));
        assert_eq!((&entry.script_public_key, entry.covenant_id), (&p2sh(1), Some(h(COV))));
        assert!(ix.first_coin(&account(), h(1)).await.unwrap().is_none(), "another covenant");
        // X spends F:1 at the same address: matched on the outpoint, only S counts.
        let s = ix.spender(&op).await.unwrap().unwrap();
        assert_eq!((s.txid, s.lock_time, s.accepted_daa), (h(0x51), None, DAA));
        assert_eq!(s.tx.inputs[0].previous_outpoint, op);
        // The continuation was remembered, so no call A; nobody spent it yet.
        assert!(ix.spender(&TransactionOutpoint::new(h(0x51), 1)).await.unwrap().is_none());
        let calls = calls.lock().unwrap();
        assert!(calls.iter().all(|c| !c.starts_with("/transactions/")), "{calls:?}");
        assert!(!calls[0].contains("after=") && !calls[0].contains("before="), "{calls:?}");
        assert!(calls.iter().any(|c| c.contains("&after=400000&")), "{calls:?}");
    }

    #[tokio::test]
    async fn places_an_unseen_coin_with_call_a() {
        let (fund, spend, _) = history();
        let (f, inner) = (fund.clone(), service(vec![spend, fund], vec![]));
        let lookup = format!("/transactions/{}?", h(0xf1));
        let route: Route =
            Box::new(move |p: &str| if p.starts_with(&lookup) { ok(f.clone()) } else { inner(p) });
        let (ix, calls) = over(route);
        let s = ix.spender(&TransactionOutpoint::new(h(0xf1), 0)).await.unwrap().unwrap();
        assert_eq!(s.txid, h(0x51));
        assert!(calls.lock().unwrap()[0].starts_with("/transactions/"));
        let unknown = ix.spender(&TransactionOutpoint::new(h(0x99), 0)).await;
        assert_eq!(unknown.unwrap_err(), NOT_INDEXED);
    }

    #[tokio::test]
    async fn refuses_unsettled_and_double_spenders() {
        let (fund, spend, _) = history();
        let mut waiting = spend.clone();
        waiting["is_accepted"] = json!(false);
        let mut twin = spend.clone();
        twin["transaction_id"] = json!(h(0x52).to_string());
        let cases = [
            (vec![waiting, fund.clone()], UNSETTLED),
            (vec![spend, twin, fund], TWO_SPENDERS),
        ];
        for (page, want) in cases {
            let (ix, _) = over(service(page, vec![]));
            let (op, _) = ix.first_coin(&account(), h(COV)).await.unwrap().unwrap();
            assert_eq!(ix.spender(&op).await.unwrap_err(), want);
        }
    }

    #[tokio::test]
    async fn paging_stops_at_the_cap() {
        let (fund, _, other) = history();
        let next = AtomicU64::new(5_000_000);
        let (ix, calls) = over(Box::new(move |p: &str| {
            if p.starts_with("/transactions/") {
                return ok(fund.clone());
            }
            let mut r = ok(json!([other.clone()]));
            r.after = Some(next.fetch_add(1, Ordering::Relaxed));
            r
        }));
        let err = ix.spender(&TransactionOutpoint::new(h(0xf1), 0)).await.unwrap_err();
        assert_eq!(err, TOO_LONG);
        assert_eq!(calls.lock().unwrap().len(), 1 + MAX_PAGES);
        let page = |after, before| Page { value: Value::Null, after, before };
        assert_eq!(next_dir(Dir::After(10), &page(Some(10), None)), Err(STUCK.to_string()));
        assert_eq!(next_dir(Dir::Before(Some(10)), &page(None, Some(11))), Err(STUCK.into()));
        let first = next_dir(Dir::Before(None), &page(None, Some(5)));
        assert_eq!(first, Ok(Some(Dir::Before(Some(5)))));
        assert_eq!(next_dir(Dir::After(10), &page(None, None)), Ok(None));
    }

    #[test]
    fn refuses_hosts_off_the_allow_list() {
        let bad = [
            "https://evil.example", "http://api.kaspa.org", "https://api.kaspa.org.evil.com",
            "https://api.kaspa.org:8443", "https://api-tn11.kaspa.org", "https://api.kaspa.org/",
        ];
        for base in bad {
            assert!(Indexer::with_base(base).is_err(), "{base}");
        }
        assert!(Indexer::with_base(TESTNET_BASE).is_ok());
        assert_eq!(base_for(&Network::testnet()), TESTNET_BASE);
        assert_eq!(base_for(&Network::mainnet()), MAINNET_BASE);
        let urls = [
            "https://evil.example/x", "https://u@api.kaspa.org/x", "http://api.kaspa.org/x",
            "https://api.kaspa.org:444/x",
        ];
        for url in urls {
            assert!(check_url(&reqwest::Url::parse(url).unwrap()).is_err(), "{url}");
        }
        let good = reqwest::Url::parse("https://api-tn10.kaspa.org/blocks/ab").unwrap();
        assert!(check_url(&good).is_ok());
        for path in ["//evil.example/x", "/a b", "/x?y=%2F", "x", "/x#y", "/x@evil", "/x.y"] {
            assert!(safe_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn refuses_oversize_bodies() {
        assert!(too_big(Some(MAX_BODY as u64 + 1)));
        assert!(!too_big(Some(MAX_BODY as u64)) && !too_big(None));
        let mut buf = vec![0u8; MAX_BODY - 1];
        assert!(grow(&mut buf, &[1]).is_ok());
        assert_eq!(grow(&mut buf, &[1]), Err(TOO_BIG.to_string()));
        assert_eq!(buf.len(), MAX_BODY);
    }

    /// The testnet proof of docs/paper-stop-indexer.md on the live service: 80d253e0…:0 ->
    /// 4648c0fa… -> ef058d34… -> d3ad4d5f…:1 (live). Needs the network, and the record must be
    /// inside the service's ~7 days of history.
    #[tokio::test]
    #[ignore]
    async fn live_testnet_walk() {
        let ix = Indexer::new(&Network::testnet()).unwrap();
        let start = "80d253e086e3b8a07d4bce761d6db900f69d1fa8109ba007c3256c27bc1666e7";
        let mut op = TransactionOutpoint::new(parse_hash(start).unwrap(), 0);
        let mut last = None;
        for _ in 0..6 {
            let Some(s) = ix.spender(&op).await.unwrap() else { break };
            let i = s.tx.outputs.iter().position(|o| o.covenant.is_some()).unwrap();
            op = TransactionOutpoint::new(s.txid, i as u32);
            last = Some(s);
        }
        assert!(op.transaction_id.to_string().starts_with("d3ad4d5f"), "{op:?}");
        let s = last.unwrap();
        let o = &s.tx.outputs[op.index as usize];
        assert_eq!(s.accepted_daa, 616_433_484);
        let a = extract_script_pub_key_address(&o.script_public_key, Prefix::Testnet).unwrap();
        let cov = o.covenant.unwrap().covenant_id;
        let (first, entry) = ix.first_coin(&a, cov).await.unwrap().unwrap();
        assert_eq!((first, entry.block_daa_score), (op, 616_433_484));
    }
}
