//! The phone's engine surface under hostile or missing input, offline (testnet, no node): every
//! method the phone can reach answers with a refusal or its documented offline answer, never
//! panics, writes nothing, and never puts secret material in an answer.
//! Methods: Bridge.java ENGINE_METHODS, then what Java calls directly (GuardService's guard.*,
//! keys.burned); `python -I -B tools/surface_check.py --table` lists them with their callers.
//! Run: cargo test --test surface_methods
//! A test marked `#[ignore = "BUG: ..."]` pins an engine bug found here and fails until it is
//! fixed: cargo test --test surface_methods -- --ignored
use kaspa_wrpc_client::{KaspaRpcClient, WrpcEncoding};
use serde_json::{Value, json};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use vault_core::contract::Dials;
use vault_core::core::{Core, Slot};
use vault_core::keys::{self, Keys};
use vault_core::network::Network;
use vault_core::tx::Ctx;
use zeroize::Zeroizing;

/// Bridge.java ENGINE_METHODS, then what Java calls directly: GuardService's guard.*, keys.burned.
const PHONE_METHODS: [&str; 34] = [
    "status", "send", "finish", "stop", "decline", "funding", "fund", "topup", "takeout",
    "purse.pay", "keys.reset", "keys.public", "keys.sheets", "keys.burn", "keys.burned",
    "rules.change", "heir.set", "guard.export", "link.code", "link.read", "link.heir_words",
    "watch.import", "watch.list", "watch.status", "watch.stop", "watch.decline", "watch.claim",
    "watch.forget", "heir.purse", "heir.pay", "guard.start", "guard.stop", "guard.wait",
    "guard.poll",
];

/// Names the engine does not know (the laptop-only methods are known, so they are not here).
const UNKNOWN: [&str; 12] = [
    "", "nope", "STATUS", "status ", " send", "keys.new", "keys", "../status", "status\u{0}",
    "link.nope", "guard.nope", "watch.nope",
];

/// Asks the node before its key gate, so with no node link it panics (BUG test
/// `no_node_link_is_a_refusal_not_a_panic`); covered with an unreachable node instead.
const NODE_FIRST: [&str; 1] = ["heir.purse"];

/// Every slot a secret can ride in on.
const SLOTS: [Slot; 8] = [
    Slot::Card, Slot::Stop, Slot::Funding, Slot::Topup, Slot::Sheets, Slot::Safe, Slot::Guardian,
    Slot::Heir,
];

/// Answer fields that would carry secret words or keys.
const SECRET_FIELDS: [&str; 10] = [
    "seed", "seeds", "words", "mnemonic", "secret", "secrets", "private", "sheets", "heir_words",
    "wif",
];

/// The engine's words (crate-private consts in core.rs).
const BAD_REQUEST: &str = "Bad request";
const WRONG_KEY: &str = "That key does not sign this move.";
const KEY_SIZE: &str = "The unlocked key is not the right size for this move.";

/// What the hostile tables may leave behind: watch.import makes `watch/` for its staging copy
/// before it knows the text is no guard file, and removes only the staging copy (watched.rs).
const LEFT_BEHIND: [&str; 1] = ["watch/"];

/// A text shaped like a helper's link code, with keys that are not keys.
const LINK_LIKE: &str = concat!(
    r#"{"kind":"kv-link","v":1,"network":"testnet-10","name":"x","#,
    r#""stop_x":"00","heir_x":"00","heir_address":"x"}"#
);

/// Bounds: JSON depth walked, folder entries walked, and the wait for one answer.
const MAX_DEPTH: usize = 32;
const MAX_WALK: usize = 10_000;
const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// A fresh empty folder under the temp dir, removed when dropped.
struct Folder(PathBuf);

impl Folder {
    fn new(tag: &str) -> Folder {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("kv-surface-{tag}-{}-{nanos}", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).unwrap();
        Folder(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn ctx(dir: &Path, keys: Keys, rpc: Option<KaspaRpcClient>) -> Ctx {
    let (network, dials) = (Network::testnet(), Dials::testnet());
    Ctx { dir: dir.to_path_buf(), keys, rpc, force: false, network, dials, account: 1 }
}

/// A phone before setup: no keys, no node link.
fn bare(dir: &Path) -> Core {
    Core::open(ctx(dir, Keys::default(), None)).unwrap()
}

/// A set-up phone in the phone's shape: the public block only. The full set comes back for the
/// leak checks.
fn keyed(dir: &Path, rpc: Option<KaspaRpcClient>) -> (Core, Keys) {
    let full = Keys::fresh(&Network::testnet());
    let mut held = full.clone();
    held.wipe();
    (Core::open(ctx(dir, held, rpc)).unwrap(), full)
}

/// A node link to a port nothing listens on: what a phone holds while the network is down.
fn dead_node() -> KaspaRpcClient {
    let id = Network::testnet().resolver_id();
    KaspaRpcClient::new(WrpcEncoding::Borsh, Some("ws://127.0.0.1:9"), None, Some(id), None)
        .unwrap()
}

/// A key block of `len` bytes 1, 2 .. 250, 1 ..: every 32-byte word is a valid secret key.
fn block(len: usize) -> Zeroizing<Vec<u8>> {
    Zeroizing::new((0..len).map(|i| (i % 250 + 1) as u8).collect())
}

/// A block length the slot accepts.
fn fitting(slot: Slot) -> usize {
    match slot {
        Slot::Stop | Slot::Topup | Slot::Safe => 64,
        Slot::Sheets => 200,
        _ => 32,
    }
}

/// The slot a method signs with, or Card for one that takes no secret.
fn own_slot(method: &str) -> Slot {
    Slot::of_method(method).unwrap_or(Slot::Card)
}

/// The hex of each 32-byte word of `bytes` (a tail of 16 bytes or more too).
fn hexes_of(bytes: &[u8]) -> Vec<String> {
    bytes.chunks(32).filter(|c| c.len() >= 16).map(faster_hex::hex_string).collect()
}

/// The hex of every secret of a full key set.
fn key_hexes(full: &Keys) -> Vec<String> {
    let held = [full.spend, full.cancel, full.cancel2, full.cancel3, full.heir, full.funding];
    let cold = [full.cold, full.cold2, full.cold3];
    held.iter().chain(cold.iter()).flatten().map(|s| faster_hex::hex_string(&s[..])).collect()
}

/// A sheet as typed from paper: its 24 words on one line.
fn line(secret: &[u8; 32]) -> String {
    keys::sheet_words(secret).join(" ")
}

/// Why an answer leaks: the hex of a known secret in its text, or a field that carries words or
/// keys (SECRET_FIELDS). None when it is clean.
fn leak(answer: &Value, hexes: &[String]) -> Option<String> {
    let text = answer.to_string().to_ascii_lowercase();
    if let Some(h) = hexes.iter().find(|h| text.contains(h.as_str())) {
        return Some(format!("the secret {}..", &h[..8]));
    }
    secret_field(answer, 0)
}

fn secret_field(v: &Value, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    match v {
        Value::Object(map) => map.iter().find_map(|(k, child)| field_or_child(k, child, depth)),
        Value::Array(items) => items.iter().find_map(|c| secret_field(c, depth + 1)),
        _ => None,
    }
}

fn field_or_child(key: &str, child: &Value, depth: usize) -> Option<String> {
    let key = key.to_ascii_lowercase();
    if SECRET_FIELDS.contains(&key.as_str()) {
        return Some(format!("the field `{key}`"));
    }
    secret_field(child, depth + 1)
}

/// Whether the answer repeats three typed words in a row.
fn echoes(answer: &Value, typed: &[u8]) -> bool {
    let typed = String::from_utf8_lossy(typed);
    let words: Vec<&str> = typed.split_whitespace().collect();
    let said = answer.to_string();
    words.windows(3).take(MAX_WALK).any(|w| said.contains(&w.join(" ")))
}

/// Every path under `dir`, folders ending in '/', sorted; the walk is bounded by MAX_WALK.
fn tree(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    for _ in 0..MAX_WALK {
        let Some(at) = todo.pop() else { break };
        let Ok(list) = std::fs::read_dir(&at) else { continue };
        for path in list.flatten().take(MAX_WALK).map(|e| e.path()) {
            let rel = path.strip_prefix(dir).map(|p| p.to_string_lossy().replace('\\', "/"));
            let rel = rel.unwrap_or_default();
            if path.is_dir() {
                found.push(format!("{rel}/"));
                todo.push(path);
            } else {
                found.push(rel);
            }
        }
    }
    found.sort();
    found
}

/// Only LEFT_BEHIND (or nothing) under `dir`.
fn wrote_nothing_else(dir: &Path) -> Result<(), String> {
    let left = tree(dir);
    if left.iter().all(|p| LEFT_BEHIND.contains(&p.as_str())) {
        return Ok(());
    }
    Err(format!("written: {left:?}"))
}

/// One plain call, or 599 when no answer came within ANSWER_WAIT.
async fn ask(core: &Core, method: &str, args: &Value) -> (u16, Value) {
    let answer = tokio::time::timeout(ANSWER_WAIT, core.call(method, args)).await;
    answer.unwrap_or_else(|_| (599, json!({"error": "no answer in time"})))
}

/// One call with a secret block, or 599 when no answer came within ANSWER_WAIT.
async fn ask_armed(
    core: &Core,
    method: &str,
    args: &Value,
    slot: Slot,
    secret: Zeroizing<Vec<u8>>,
) -> (u16, Value) {
    let call = core.call_with_secret(method, args, slot, secret);
    let answer = tokio::time::timeout(ANSWER_WAIT, call).await;
    answer.unwrap_or_else(|_| (599, json!({"error": "no answer in time"})))
}

/// Runs `job` on its own task: Err when it panicked or gave no answer within ANSWER_WAIT.
async fn on_task<F>(job: F) -> Result<(u16, Value), String>
where
    F: Future<Output = (u16, Value)> + Send + 'static,
{
    match tokio::time::timeout(ANSWER_WAIT, tokio::spawn(job)).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(e)) => Err(format!("panicked ({e})")),
        Err(_) => Err("no answer in time".into()),
    }
}

/// What an answer should be.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Want {
    /// The no-keys refusal, with the flag the page reads.
    NoKeys,
    /// A plain refusal: ok false, words, no flag.
    Refused,
    /// A harmless offline answer with ok true (guard.stop/wait, watch.list, keys.burned, ..).
    Fine,
    /// The node cannot be reached: a refusal with words, as 503 or 200.
    Down,
}

/// What a phone with no keys and no node answers a plain call.
fn want_bare(method: &str) -> Want {
    match method {
        "guard.stop" | "guard.wait" | "watch.list" | "keys.burned" => Want::Fine,
        "guard.start" | "guard.poll" | "guard.export" | "link.code" | "link.read" => Want::Refused,
        m if m.starts_with("watch.") => Want::Refused,
        _ => Want::NoKeys,
    }
}

/// What it answers a money method armed with a fitting block of the method's own slot.
fn want_bare_armed(method: &str) -> Want {
    match method {
        "send" | "stop" | "decline" | "fund" | "purse.pay" | "topup" => Want::NoKeys,
        _ => Want::Refused,
    }
}

/// What a set-up phone answers before its node gate; None for a method behind the gate.
fn want_offline(method: &str) -> Option<Want> {
    let early = matches!(method, "keys.public" | "keys.burned" | "link.code" | "link.read");
    match method {
        "keys.public" => Some(Want::Fine),
        m if early || m.starts_with("guard.") || m.starts_with("watch.") => Some(want_bare(m)),
        _ => None,
    }
}

/// The phone methods a set-up phone answers only after its node gate.
fn gated() -> Vec<&'static str> {
    PHONE_METHODS.iter().copied().filter(|m| want_offline(m).is_none()).collect()
}

/// Err(why) when `got` is not what `want` names, or leaks one of `hexes` or a secret field.
fn judge(what: &str, got: &(u16, Value), want: Want, hexes: &[String]) -> Result<(), String> {
    let (status, body) = got;
    if let Some(why) = leak(body, hexes) {
        return Err(format!("{what}: leaks {why}"));
    }
    let ok = body["ok"] == json!(true);
    let words = body["error"].as_str().is_some_and(|e| !e.trim().is_empty());
    let flag = body["no_keys"] == json!(true);
    let fits = match want {
        Want::NoKeys => *status == 200 && !ok && words && flag,
        Want::Refused => *status == 200 && !ok && words && !flag,
        Want::Fine => *status == 200 && ok,
        Want::Down => matches!(*status, 200 | 503) && !ok && words,
    };
    if fits {
        return Ok(());
    }
    Err(format!("{what}: want {want:?}, got {status} {}", clip(body)))
}

fn clip(v: &Value) -> String {
    v.to_string().chars().take(200).collect()
}

/// Fails with every collected complaint (the first 25 shown).
fn settle(test: &str, bad: &[String]) {
    let shown: Vec<&str> = bad.iter().take(25).map(String::as_str).collect();
    assert!(bad.is_empty(), "{test}: {} bad answers:\n{}", bad.len(), shown.join("\n"));
}

/// Object args a hostile or broken page could send: empty, unknown fields, numbers as strings,
/// out-of-range numbers (NaN and infinity reach serde as null), ~1 MB strings, wrong types, path
/// tricks, and texts shaped like the files watch.import and link.read take.
fn hostile_args() -> Vec<Value> {
    let big = "9".repeat(1 << 20);
    let odd = "\u{202e}x\u{0}\u{feff}..".repeat(8);
    let guard_like = r#"{"public_sha256": "aaaaaaaaaaaa00", "kind": "x"}"#;
    vec![
        json!({}),
        json!({"zzz": 1, "__proto__": {"admin": true}, "constructor": "x", "ok": true}),
        json!({"kas": "100", "amount": "1e3", "sompi": "-1", "to": 5, "account": "2", "id": 7,
            "text": 7, "seconds": "5", "timeout_ms": "5", "nodes": "wss://x", "name": 9,
            "delay": "60", "window": "1", "cap_kas": "10"}),
        json!({"kas": -1, "amount": -0.5, "sompi": u64::MAX, "account": -1, "seconds": i64::MIN,
            "timeout_ms": 1e300, "fee": f64::NAN, "delay": f64::INFINITY, "cap_kas": 1e308,
            "heir_delay": 0, "window": -86400}),
        json!({"to": big, "text": big, "id": big, "name": big, "kas": big, "nodes": [big, big]}),
        json!({"to": [], "kas": {}, "text": {"a": 1}, "id": [], "nodes": {}, "account": [1],
            "seconds": true, "timeout_ms": [1], "name": null}),
        json!({"id": "../../../../windows", "name": "..\\..\\x", "text": "../keys.json",
            "to": "kaspa:qqqq", "nodes": ["file:///etc/passwd", "file:///etc/passwd2"]}),
        json!({"id": "aaaaaaaaaaaa", "text": "{}", "to": "kaspatest:qq", "kas": 1}),
        json!({"id": "AAAAAAAAAAAA", "account": 4_294_967_296u64, "text": odd, "name": odd}),
        json!({"text": guard_like, "nodes": ["ws://127.0.0.1:9", "ws://127.0.0.1:10"],
            "seconds": 0}),
        json!({"text": LINK_LIKE, "account": 2}),
    ]
}

/// Typed take-out texts that must be refused before anything signs (none is a valid pair):
/// unreadable bytes, one line, three lines, 23 words, non-words, a foreign sheet first, one
/// sheet twice, a NUL and a direction mark in the words, and one 4096-byte line.
fn typed_texts(full: &Keys) -> Vec<Vec<u8>> {
    let cold = line(&full.cold.unwrap());
    let foreign = line(&[7u8; 32]);
    let short = foreign.split(' ').take(23).collect::<Vec<_>>().join(" ");
    let junk = ["zzzz"; 24].join(" ");
    vec![
        b"\xff\xfe\x80 not text".to_vec(),
        cold.clone().into_bytes(),
        format!("{cold}\n{foreign}\n{foreign}").into_bytes(),
        format!("{short}\n{short}").into_bytes(),
        format!("{junk}\n{junk}").into_bytes(),
        format!("{foreign}\n{cold}").into_bytes(),
        format!("{cold}\n{cold}").into_bytes(),
        format!("{cold}\u{0}\n\u{202e}{foreign}").into_bytes(),
        "a ".repeat(2048).into_bytes(),
    ]
}

/// Sealed safe-key blocks that are not this account's pair: two foreign keys, one own key and a
/// foreign one, one own key twice.
fn sealed_blocks(full: &Keys) -> Vec<Zeroizing<Vec<u8>>> {
    let (cold, foreign) = (full.cold.unwrap(), [7u8; 32]);
    let sets = [vec![foreign, [8u8; 32]], vec![cold, foreign], vec![cold, cold, foreign]];
    sets.iter().map(|s| Zeroizing::new(s.concat())).collect()
}

/// Both doors' answers to non-object args, as complaints.
async fn shape_complaints(core: &Core, method: &str, args: &Value) -> Vec<String> {
    let want = (400, json!({"ok": false, "error": BAD_REQUEST}));
    let slot = own_slot(method);
    let plain = ask(core, method, args).await;
    let armed = ask_armed(core, method, args, slot, block(fitting(slot))).await;
    let mut out = Vec::new();
    for (door, got) in [("call", plain), ("call_with_secret", armed)] {
        if got != want {
            out.push(format!("{door} {method:?} {args}: {} {}", got.0, clip(&got.1)));
        }
    }
    out
}

/// Args that are not an object: 400 "Bad request" on both doors for every name, nothing written.
#[tokio::test]
async fn non_object_args_are_bad_requests_and_write_nothing() {
    let dir = Folder::new("shape");
    let core = bare(&dir.0);
    let shapes = [json!([]), json!([1, "a"]), json!("status"), json!(42), json!(-1.5), json!(true)];
    let mut bad = Vec::new();
    for method in PHONE_METHODS.iter().chain(UNKNOWN.iter()) {
        for args in shapes.iter().chain([Value::Null].iter()) {
            bad.extend(shape_complaints(&core, method, args).await);
        }
    }
    // 1e999 cannot ride in as a number: the phone's parse refuses it (android.rs parse_args), or,
    // with arbitrary precision on, it is no integer a method reads.
    if let Ok(v) = serde_json::from_str::<Value>(r#"{"kas": 1e999}"#) {
        assert!(v["kas"].as_u64().is_none() && v["kas"].as_i64().is_none());
    }
    assert_eq!(tree(&dir.0), Vec::<String>::new(), "a bad request wrote to the folder");
    settle("non-object args", &bad);
}

/// A phone before setup, every method, every hostile object: the no-keys answer, a plain
/// refusal, or a harmless offline answer; nothing secret in any; nothing but LEFT_BEHIND written.
#[tokio::test]
async fn bare_phone_answers_hostile_args_safely() {
    let dir = Folder::new("bare");
    let core = bare(&dir.0);
    let table = hostile_args();
    let mut bad = Vec::new();
    for method in PHONE_METHODS.iter().filter(|m| !NODE_FIRST.contains(*m)) {
        for (i, args) in table.iter().enumerate() {
            let got = ask(&core, method, args).await;
            bad.extend(judge(&format!("{method} args#{i}"), &got, want_bare(method), &[]).err());
        }
    }
    bad.extend(wrote_nothing_else(&dir.0).err());
    settle("bare phone", &bad);
}

/// A phone before setup, each money method armed with a fitting block of its own slot, every
/// hostile object: refused (keys.sheets, which shows its block's words, has its own test).
#[tokio::test]
async fn bare_phone_refuses_armed_money_methods() {
    let dir = Folder::new("bare-armed");
    let core = bare(&dir.0);
    let table = hostile_args();
    let mut bad = Vec::new();
    let armed = PHONE_METHODS.iter().filter(|m| Slot::of_method(m).is_some());
    for method in armed.filter(|m| **m != "keys.sheets") {
        let slot = own_slot(method);
        let hexes = hexes_of(&block(fitting(slot)));
        for (i, args) in table.iter().enumerate() {
            let got = ask_armed(&core, method, args, slot, block(fitting(slot))).await;
            let what = format!("{method} armed args#{i}");
            bad.extend(judge(&what, &got, want_bare_armed(method), &hexes).err());
        }
    }
    // takeout also takes the phone's own sealed safe keys (Slot::Safe, 64 or 96 bytes).
    let got = ask_armed(&core, "takeout", &json!({}), Slot::Safe, block(96)).await;
    bad.extend(judge("takeout safe", &got, Want::Refused, &hexes_of(&block(96))).err());
    assert_eq!(tree(&dir.0), Vec::<String>::new(), "an armed refusal wrote to the folder");
    settle("bare phone armed", &bad);
}

/// A secret in a slot that does not sign the method: 400 WRONG_KEY before anything is armed.
#[tokio::test]
async fn wrong_slot_is_refused_before_arming() {
    let dir = Folder::new("slot");
    let core = bare(&dir.0);
    let want = (400, json!({"ok": false, "error": WRONG_KEY}));
    let mut bad = Vec::new();
    for method in PHONE_METHODS.iter().chain(UNKNOWN.iter()) {
        for slot in SLOTS.iter().copied().filter(|s| !s.serves(method)) {
            let got = ask_armed(&core, method, &json!({}), slot, block(fitting(slot))).await;
            if got != want {
                bad.push(format!("{method:?} in {slot:?}: {} {}", got.0, clip(&got.1)));
            }
        }
    }
    assert_eq!(tree(&dir.0), Vec::<String>::new());
    settle("wrong slot", &bad);
}

/// A block of a size its slot does not take (up to 1 MB): 400 KEY_SIZE, nothing armed.
#[tokio::test]
async fn wrong_size_secret_is_refused() {
    let dir = Folder::new("size");
    let core = bare(&dir.0);
    let sizes = [0, 1, 31, 33, 63, 65, 95, 97, 128, 4097, 1 << 20];
    let want = (400, json!({"ok": false, "error": KEY_SIZE}));
    let mut pairs: Vec<(&str, Slot)> =
        PHONE_METHODS.iter().filter_map(|m| Slot::of_method(m).map(|s| (*m, s))).collect();
    pairs.push(("takeout", Slot::Safe));
    let mut bad = Vec::new();
    for (method, slot) in pairs {
        for n in sizes.iter().copied().filter(|n| !slot.accepts(*n)) {
            let got = ask_armed(&core, method, &json!({}), slot, block(n)).await;
            if got != want {
                bad.push(format!("{method} {slot:?} {n} bytes: {} {}", got.0, clip(&got.1)));
            }
        }
    }
    assert_eq!(tree(&dir.0), Vec::<String>::new());
    settle("wrong size", &bad);
}

/// Names the engine does not know are refused, never answered ok; under guard. and watch. they
/// are a 404 even offline (every other name meets the gates first: the test below).
#[tokio::test]
async fn unknown_methods_are_refused() {
    let dir = Folder::new("unknown");
    let core = bare(&dir.0);
    let not_found = (404, json!({"ok": false, "error": "Not found"}));
    let mut bad = Vec::new();
    for name in UNKNOWN {
        let got = ask(&core, name, &json!({})).await;
        let scoped = name.starts_with("guard.") || name.starts_with("watch.");
        let no_keys = !scoped && judge(name, &got, Want::NoKeys, &[]).is_ok();
        if got != not_found && !no_keys {
            bad.push(format!("{name:?}: {} {}", got.0, clip(&got.1)));
        }
    }
    assert_eq!(tree(&dir.0), Vec::<String>::new());
    settle("unknown names", &bad);
}

/// An unknown name outside guard./watch. meets the gates before the method match (Core::call's
/// doc): the no-keys answer on a bare phone, the network-down 503 on a set-up phone offline.
/// Bridge.java answers unknown names 404 itself, so the phone never sees either.
#[tokio::test]
async fn unknown_method_meets_the_gates_first() {
    let (dir, set_dir) = (Folder::new("unknown404"), Folder::new("unknown503"));
    let core = bare(&dir.0);
    let not_found = (404, json!({"ok": false, "error": "Not found"}));
    let mut bad = Vec::new();
    for name in UNKNOWN {
        let got = ask(&core, name, &json!({})).await;
        if !name.starts_with("guard.") && !name.starts_with("watch.") {
            bad.extend(judge(name, &got, Want::NoKeys, &[]).err());
        } else if got != not_found {
            bad.push(format!("{name:?}: {} {}", got.0, clip(&got.1)));
        }
    }
    let set_up = keyed(&set_dir.0, Some(dead_node())).0;
    let got = ask(&set_up, "nope", &json!({})).await;
    if got.0 != 503 {
        bad.push(format!("set-up \"nope\": want 503, got {} {}", got.0, clip(&got.1)));
    }
    bad.extend(judge("set-up nope", &got, Want::Down, &[]).err());
    settle("unknown names meet the gates", &bad);
}

/// keys.sheets shows the words of the sealed block it is handed, and only through Slot::Safe:
/// the leak check flags that answer (so it would flag a stray one); the raw block never shows.
#[tokio::test]
async fn keys_sheets_shows_only_its_own_block() {
    let dir = Folder::new("sheets");
    let core = bare(&dir.0);
    let sealed = block(96);
    let got = ask_armed(&core, "keys.sheets", &json!({"zzz": 1}), Slot::Safe, block(96)).await;
    assert_eq!((got.0, &got.1["ok"]), (200, &json!(true)), "{}", clip(&got.1));
    let sheets = got.1["sheets"].as_array().cloned().unwrap_or_default();
    assert_eq!(sheets.len(), 3, "one sheet per 32-byte word");
    for (word, sheet) in sealed.chunks(32).zip(&sheets) {
        let word: [u8; 32] = word.try_into().unwrap();
        assert_eq!(sheet, &json!(keys::sheet_words(&word)));
    }
    let flagged = leak(&got.1, &[]);
    assert!(flagged.is_some_and(|w| w.contains("sheets")), "the leak check missed the words");
    let text = got.1.to_string();
    assert!(!hexes_of(&sealed).iter().any(|h| text.contains(h.as_str())), "raw block shown");
    let burned = ask_armed(&core, "keys.sheets", &json!({}), Slot::Safe, block(0)).await;
    assert_eq!(judge("burned", &burned, Want::Refused, &[]), Ok(()));
    assert_eq!(tree(&dir.0), Vec::<String>::new());
}

/// A set-up phone (public block only, no node link), the methods that answer before the node,
/// every hostile object: keys.public shows the block and no secret of the set; the rest answer as
/// on a bare phone.
#[tokio::test]
async fn set_up_phone_offline_methods_leak_nothing() {
    let dir = Folder::new("keyed");
    let (core, full) = keyed(&dir.0, None);
    let hexes = key_hexes(&full);
    let table = hostile_args();
    let mut bad = Vec::new();
    for (method, want) in PHONE_METHODS.iter().filter_map(|m| want_offline(m).map(|w| (*m, w))) {
        for (i, args) in table.iter().enumerate() {
            let got = ask(&core, method, args).await;
            bad.extend(judge(&format!("{method} args#{i}"), &got, want, &hexes).err());
        }
    }
    let public = ask(&core, "keys.public", &json!({})).await;
    if !public.1["public"].is_object() {
        bad.push(format!("keys.public shows no block: {}", clip(&public.1)));
    }
    bad.extend(wrote_nothing_else(&dir.0).err());
    settle("set-up phone offline", &bad);
}

/// Take-out on a set-up phone with typed sheets or sealed safe keys that are not this account's
/// pair: refused in plain words that never echo what was typed and carry no secret.
#[tokio::test]
async fn typed_sheets_are_refused_without_echo() {
    let dir = Folder::new("typed");
    let (core, full) = keyed(&dir.0, None);
    let hexes = key_hexes(&full);
    let mut bad = Vec::new();
    for (i, text) in typed_texts(&full).into_iter().enumerate() {
        let typed = Zeroizing::new(text.clone());
        let got = ask_armed(&core, "takeout", &json!({}), Slot::Sheets, typed).await;
        bad.extend(judge(&format!("typed#{i}"), &got, Want::Refused, &hexes).err());
        if echoes(&got.1, &text) {
            bad.push(format!("typed#{i}: the answer echoes the typed words"));
        }
    }
    for (i, sealed) in sealed_blocks(&full).into_iter().enumerate() {
        let got = ask_armed(&core, "takeout", &json!({}), Slot::Safe, sealed).await;
        bad.extend(judge(&format!("sealed#{i}"), &got, Want::Refused, &hexes).err());
    }
    assert_eq!(tree(&dir.0), Vec::<String>::new());
    settle("typed sheets", &bad);
}

/// A helper phone (its link code made, no keys of its own): link.code shows no secret of the
/// link; heir.pay and link.heir_words refuse a key that is not the link's; heir.pay with the
/// right key refuses hostile payees before the node; the watch moves refuse with no account.
#[tokio::test]
async fn helper_phone_refuses_wrong_keys_and_leaks_nothing() {
    let dir = Folder::new("helper");
    let net = Network::testnet();
    let (secrets, _) = vault_core::link::make_link(&dir.0, &net, "Helper").unwrap();
    let core = bare(&dir.0);
    let before = tree(&dir.0);
    let mut hexes = hexes_of(&secrets);
    hexes.extend(hexes_of(&block(32)));
    let mut bad = Vec::new();
    let code = ask(&core, "link.code", &json!({})).await;
    bad.extend(judge("link.code", &code, Want::Fine, &hexes).err());
    let heir = heir_key(&dir.0, &secrets);
    let zero = Zeroizing::new(vec![0u8; 32]);
    let moves = [
        ("heir.pay", heir.clone()), ("heir.pay", block(32)), ("link.heir_words", block(32)),
        ("link.heir_words", zero), ("watch.stop", heir.clone()), ("watch.decline", block(32)),
        ("watch.claim", heir),
    ];
    for (i, args) in hostile_args().iter().enumerate() {
        for (method, key) in &moves {
            let got = ask_armed(&core, method, args, own_slot(method), key.clone()).await;
            bad.extend(judge(&format!("{method} #{i}"), &got, Want::Refused, &hexes).err());
        }
    }
    assert_eq!(tree(&dir.0), before, "a refusal wrote to the helper's folder");
    settle("helper phone", &bad);
}

/// The link's heir secret: the 32-byte word of `secrets` whose x-only key is link.json's heir_x.
fn heir_key(dir: &Path, secrets: &[u8]) -> Zeroizing<Vec<u8>> {
    let text = std::fs::read_to_string(dir.join(vault_core::link::LINK_FILE)).unwrap();
    let link: Value = serde_json::from_str(&text).unwrap();
    let heir_x = link["heir_x"].as_str().unwrap_or("").to_ascii_lowercase();
    let is_heir = |w: &&[u8]| {
        let Ok(word) = <[u8; 32]>::try_from(*w) else { return false };
        faster_hex::hex_string(&keys::xonly(&keys::keypair(&word))) == heir_x
    };
    let word = secrets.chunks(32).find(is_heir).expect("the link's heir key among its secrets");
    Zeroizing::new(word.to_vec())
}

/// A set-up phone whose node is down (the phone's real offline state): every gated method plain,
/// each money method armed with a fitting block of its own slot, and heir.purse on a bare phone
/// refuse with words; none answers ok, none leaks a secret. One phone per method, run side by
/// side: each refused connect takes about 2 s on Windows.
#[tokio::test]
async fn unreachable_node_is_a_plain_refusal() {
    let dir = Folder::new("down");
    let bare_dir = Folder::new("down-bare");
    let mut runs = Vec::new();
    for method in gated() {
        let (core, full) = keyed(&dir.0, Some(dead_node()));
        runs.push(tokio::spawn(down_complaints(core, key_hexes(&full), method)));
    }
    let lone = Core::open(ctx(&bare_dir.0, Keys::default(), Some(dead_node()))).unwrap();
    let got = ask(&lone, "heir.purse", &json!({})).await;
    let mut bad = Vec::new();
    bad.extend(judge("heir.purse bare", &got, Want::Down, &[]).err());
    for run in runs {
        match tokio::time::timeout(ANSWER_WAIT * 2, run).await {
            Ok(Ok(complaints)) => bad.extend(complaints),
            other => bad.push(format!("a node-down run ended badly: {other:?}")),
        }
    }
    assert_eq!(tree(&dir.0), Vec::<String>::new());
    settle("node down", &bad);
}

/// One gated method on its own set-up phone whose node is down: plain, then armed with a fitting
/// block of its own slot (keys.sheets answers offline by design, so it is left out armed).
async fn down_complaints(core: Core, hexes: Vec<String>, method: &'static str) -> Vec<String> {
    let mut bad = Vec::new();
    let got = ask(&core, method, &json!({"kas": -1, "to": "x"})).await;
    bad.extend(judge(&format!("{method} plain"), &got, Want::Down, &hexes).err());
    let Some(slot) = Slot::of_method(method).filter(|_| method != "keys.sheets") else {
        return bad;
    };
    let mut known = hexes;
    known.extend(hexes_of(&block(fitting(slot))));
    let got = ask_armed(&core, method, &json!({}), slot, block(fitting(slot))).await;
    bad.extend(judge(&format!("{method} armed"), &got, Want::Down, &known).err());
    bad
}

#[tokio::test]
#[ignore = "by design: the phone always has a node link (android.rs passes Some(rpc)); \
            a Core opened with Ctx.rpc None panics in Ctx::rpc() (tx.rs), which only a test \
            can reach."]
async fn no_node_link_is_a_refusal_not_a_panic() {
    let (bare_dir, dir) = (Folder::new("nolink-bare"), Folder::new("nolink"));
    let lone = Arc::new(bare(&bare_dir.0));
    let set_up = Arc::new(keyed(&dir.0, None).0);
    let mut jobs = vec![(lone, "heir.purse")];
    jobs.extend(gated().into_iter().map(|m| (set_up.clone(), m)));
    let mut bad = Vec::new();
    for (core, method) in jobs {
        let answer = on_task(async move { core.call(method, &json!({})).await }).await;
        match answer {
            Ok(got) => bad.extend(judge(method, &got, Want::Down, &[]).err()),
            Err(why) => bad.push(format!("{method}: {why}")),
        }
    }
    settle("no node link", &bad);
}

#[tokio::test]
async fn invalid_secret_words_are_refused_not_panicked() {
    let (dir, helper) = (Folder::new("zero"), Folder::new("zero-helper"));
    let (core, full) = keyed(&dir.0, None);
    vault_core::link::make_link(&helper.0, &Network::testnet(), "Helper").unwrap();
    let (core, lone) = (Arc::new(core), Arc::new(bare(&helper.0)));
    let cold = line(&full.cold.unwrap());
    let zero_first = format!("{}\n{cold}", line(&[0u8; 32])).into_bytes();
    let past_order = format!("{cold}\n{}", line(&[0xFF; 32])).into_bytes();
    let jobs = [
        (core.clone(), "takeout", Slot::Sheets, zero_first),
        (core.clone(), "takeout", Slot::Sheets, past_order),
        (core, "takeout", Slot::Safe, [[0u8; 32], full.cold.unwrap()].concat()),
        (lone.clone(), "heir.pay", Slot::Heir, vec![0u8; 32]),
        (lone, "heir.pay", Slot::Heir, vec![0xFF; 32]),
    ];
    let mut bad = Vec::new();
    for (i, (core, method, slot, secret)) in jobs.into_iter().enumerate() {
        let job = async move {
            core.call_with_secret(method, &json!({}), slot, Zeroizing::new(secret)).await
        };
        match on_task(job).await {
            Ok(got) => bad.extend(judge(&format!("#{i}"), &got, Want::Refused, &[]).err()),
            Err(why) => bad.push(format!("#{i} {method} {slot:?}: {why}")),
        }
    }
    settle("invalid secret words", &bad);
}

// ---- Amounts as digit strings (the phone page sends "12.5", never a JSON number) ----

/// The engine's amount refusal (core.rs BAD_AMOUNT).
const BAD_AMOUNT_WORDS: &str = "That amount doesn't look right";
/// How long a call may sit on the silent node: an amount refusal comes before any network word,
/// so a call still waiting after this got past its arguments.
const PAST_ARGS_WAIT: Duration = Duration::from_secs(3);

/// Amounts the page may send: whole, half, all 8 decimals, padded with spaces.
fn good_amounts() -> Vec<String> {
    ["1", "0.5", "12.34567890", " 2 "].map(String::from).to_vec()
}

/// Amounts the engine must refuse: empty, words, sign, exponent, comma, zero, a 9th decimal,
/// NaN, and 1 MB of digits.
fn bad_amounts() -> Vec<String> {
    let few = ["", "abc", "-1", "1e3", "1,5", "0", "0.000000001", "NaN"];
    let mut out: Vec<String> = few.map(String::from).to_vec();
    out.push("1".repeat(1 << 20));
    out
}

/// SHA-1 (FIPS 180-4), only for the accept key the silent node's websocket upgrade answers.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        sha1_block(&mut h, chunk);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// One 64-byte block of SHA-1 into the state `h`.
fn sha1_block(h: &mut [u32; 5], chunk: &[u8]) {
    let mut w = [0u32; 80];
    for (i, word) in chunk.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes(word.try_into().unwrap());
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A827999u32),
            20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
            _ => (b ^ c ^ d, 0xCA62C1D6),
        };
        let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k);
        (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t.wrapping_add(*wi));
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(y);
    }
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            let pad = i > c.len();
            out.push(if pad { '=' } else { ABC[(n >> (18 - 6 * i)) as usize & 63] as char });
        }
    }
    out
}

/// The websocket accept key for a client's Sec-WebSocket-Key (RFC 6455 4.2.2).
fn ws_accept(key: &str) -> String {
    base64(&sha1(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes()))
}

/// A node link to a local socket that accepts the websocket upgrade and then never answers: the
/// node gate passes, so a move that got past its arguments waits on the network.
async fn silent_node() -> KaspaRpcClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(hold_socket(stream));
        }
    });
    let id = Network::testnet().resolver_id();
    KaspaRpcClient::new(WrpcEncoding::Borsh, Some(&url), None, Some(id), None).unwrap()
}

/// Answers one websocket upgrade, then reads and drops every frame until the client leaves.
async fn hold_socket(mut stream: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut head, mut buf) = (Vec::new(), [0u8; 4096]);
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let key = text.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        name.trim().eq_ignore_ascii_case("sec-websocket-key").then(|| value.trim().to_string())
    });
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        ws_accept(&key.unwrap_or_default())
    );
    if stream.write_all(reply.as_bytes()).await.is_err() {
        return;
    }
    while matches!(stream.read(&mut buf).await, Ok(n) if n > 0) {}
}

/// One money method as the phone calls it: the slot and block it arms with (None = a plain
/// call), the args around the amount, and the field the amount rides in.
struct Move {
    method: &'static str,
    slot: Option<Slot>,
    block: Zeroizing<Vec<u8>>,
    args: Value,
    field: &'static str,
}

/// How the phone calls `method`, armed with the set-up phone's own secrets; `to` is a payee.
fn phone_move(method: &'static str, full: &Keys, heir: &Zeroizing<Vec<u8>>, to: &str) -> Move {
    let sealed = |words: &[Option<[u8; 32]>]| {
        let held = words.iter().map(|w| w.expect("a fresh key set holds every secret"));
        Zeroizing::new(held.flat_map(|w| w.to_vec()).collect::<Vec<u8>>())
    };
    let (slot, block, args) = match method {
        "send" => (Some(Slot::Card), sealed(&[full.spend]), json!({"to": to})),
        "fund" => (Some(Slot::Funding), sealed(&[full.funding]), json!({})),
        "topup" => (Some(Slot::Topup), sealed(&[full.spend, full.funding]), json!({})),
        "takeout" => (Some(Slot::Safe), sealed(&[full.cold, full.cold2]), json!({"to": to})),
        "purse.pay" => (Some(Slot::Funding), sealed(&[full.funding]), json!({"to": to})),
        "heir.pay" => (Some(Slot::Heir), heir.clone(), json!({"to": to})),
        _ => (None, Zeroizing::new(Vec::new()), json!({"kind": "send", "to": to})),
    };
    let field = if method == "quote" { "amount" } else { "kas" };
    Move { method, slot, block, args, field }
}

/// What one amount got: an answer, a panic, or still waiting on the node after PAST_ARGS_WAIT.
#[derive(Debug)]
enum Got {
    Answer(u16, Value),
    Panicked(String),
    Waited,
}

/// Calls the move with `amount` on its own task, through call or call_with_secret.
async fn try_amount(core: Arc<Core>, mv: Arc<Move>, amount: &str) -> Got {
    let mut args = mv.args.clone();
    args[mv.field] = json!(amount);
    let job = async move {
        match mv.slot {
            Some(slot) => core.call_with_secret(mv.method, &args, slot, mv.block.clone()).await,
            None => core.call(mv.method, &args).await,
        }
    };
    let task = tokio::spawn(job);
    let abort = task.abort_handle();
    match tokio::time::timeout(PAST_ARGS_WAIT, task).await {
        Ok(Ok((status, body))) => Got::Answer(status, body),
        Ok(Err(e)) => Got::Panicked(e.to_string()),
        Err(_) => {
            abort.abort();
            Got::Waited
        }
    }
}

/// The amount refusal: 200, ok false, the engine's amount words.
fn amount_refused(got: &Got) -> bool {
    let Got::Answer(200, body) = got else { return false };
    let words = body["error"].as_str().unwrap_or("");
    body["ok"] == json!(false) && words.contains(BAD_AMOUNT_WORDS)
}

/// A set-up phone on the silent node with its link made, the full key set, and the heir key.
async fn amount_phone(dir: &Path) -> (Arc<Core>, Keys, Zeroizing<Vec<u8>>) {
    let (secrets, _) = vault_core::link::make_link(dir, &Network::testnet(), "Heir").unwrap();
    let heir = heir_key(dir, &secrets);
    let (core, full) = keyed(dir, Some(silent_node().await));
    (Arc::new(core), full, heir)
}

fn shown(amount: &str) -> String {
    format!("{:?}", amount.chars().take(16).collect::<String>())
}

fn clip_got(got: &Got) -> String {
    match got {
        Got::Answer(status, body) => format!("{status} {}", clip(body)),
        Got::Panicked(why) => format!("panicked: {why}"),
        Got::Waited => "still waiting on the node".into(),
    }
}

/// Every bad amount meets the amount refusal and every good one gets past argument parsing
/// (waits on the node or meets a later refusal), through the phone's own door, with no panic.
async fn amount_strings_parse(method: &'static str) {
    let dir = Folder::new("amount");
    let (core, full, heir) = amount_phone(&dir.0).await;
    let to = Network::testnet().p2pk(&keys::xonly(&keys::keypair(&[3u8; 32]))).to_string();
    let mv = Arc::new(phone_move(method, &full, &heir, &to));
    let mut bad = Vec::new();
    for amount in bad_amounts() {
        let got = try_amount(core.clone(), mv.clone(), &amount).await;
        if !amount_refused(&got) {
            bad.push(format!("{method} {} not refused: {}", shown(&amount), clip_got(&got)));
        }
    }
    let goods = good_amounts();
    let runs = goods.iter().map(|a| try_amount(core.clone(), mv.clone(), a));
    for (amount, got) in goods.iter().zip(futures::future::join_all(runs).await) {
        if amount_refused(&got) || matches!(got, Got::Panicked(_)) {
            bad.push(format!("{method} {} refused: {}", shown(amount), clip_got(&got)));
        }
    }
    settle(&format!("{method} amount strings"), &bad);
}

/// The silent node's accept key matches RFC 6455's worked example.
#[test]
fn silent_node_accept_key_is_rfc_6455() {
    assert_eq!(ws_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_amount_strings_parse() {
    amount_strings_parse("send").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fund_amount_strings_parse() {
    amount_strings_parse("fund").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topup_amount_strings_parse() {
    amount_strings_parse("topup").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn takeout_amount_strings_parse() {
    amount_strings_parse("takeout").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn purse_pay_amount_strings_parse() {
    amount_strings_parse("purse.pay").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn heir_pay_amount_strings_parse() {
    amount_strings_parse("heir.pay").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quote_amount_strings_parse() {
    amount_strings_parse("quote").await;
}
