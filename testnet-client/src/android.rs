//! The Android library's edge: the JNI entry points behind `com.kaspavault.wallet.VaultLib`. Each native
//! answers the envelope string `{"status":<int>,"body":<json>}`; every body comes from `Core::call` or
//! `Core::call_with_secret`, so the phone and the laptop's https service say the same words. The keys on
//! the phone are the public block only (`Keys::load_public`); a secret reaches Rust in `callWithSecret` for
//! one call (D3), or leaves it once in `keysNew` as the bytes Java seals (D4), and is zeroed here on every
//! path. Every entry runs inside `catch_unwind`: a panic never crosses into Java. Nothing here logs a key.
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jbyteArray, jstring};
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::commands::{refuse_existing, write_public_export};
use crate::contract::Dials;
use crate::core::{Core, Slot, guard};
use crate::keys::{Keys, OfflineExport};
use crate::network::Network;
use crate::tx::{self, Ctx};

/// The one tokio runtime of the process: made by the first `init`, kept even when that init's
/// connection fails, so a retry only has to connect again.
static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
/// The wallet's folder and network from the first `init`: what `keysNew` needs, connection or not.
static HOME: OnceLock<(PathBuf, Network)> = OnceLock::new();
/// The opened service; set once a connection and the key checks have both passed, and replaced once by
/// `keysNew` so the new rule block is served without a restart.
static CORE: Mutex<Option<Arc<Core>>> = Mutex::new(None);
/// True while a background connect runs; the last failed connect's words, for the page.
static CONNECTING: AtomicBool = AtomicBool::new(false);
static LAST_ERR: Mutex<Option<String>> = Mutex::new(None);

const NOT_CONNECTED: &str = "The wallet engine is not connected yet.";
/// The wRPC url of the last node that answered, in the wallet folder: tried first at the next start (the public
/// resolver took ~20 s on the phone), forgotten when it fails. Not a key file: `keys.reset` leaves it.
const NODE_FILE: &str = "node.txt";
/// How long the remembered node may take before the resolver is asked instead.
const REMEMBERED_NODE_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
const NOT_OPENED: &str = "Open the wallet first.";
const FAULT: &str = "The wallet engine hit a fault.";
const NO_SECRET_METHOD: &str = "That move takes no key.";

/// The envelope every native answers with.
fn envelope(status: u16, body: Value) -> String {
    json!({"status": status, "body": body}).to_string()
}

fn error_envelope(status: u16, words: &str) -> String {
    envelope(status, json!({"error": words}))
}

/// Hands a Rust string to Java; a null on the rare failure to allocate (Java sees `null` and treats it as a fault).
fn to_java(env: &mut JNIEnv, s: String) -> jstring {
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn read_java_string(env: &mut JNIEnv, s: &JString) -> Result<String, String> {
    env.get_string(s).map(|js| js.into()).map_err(|e| format!("bad string from Java: {e}"))
}

fn runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    if let Some(rt) = RT.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| format!("The wallet engine could not start: {e}"))?;
    // A second init racing the first loses the race harmlessly; the stored one wins.
    let _ = RT.set(rt);
    Ok(RT.get().expect("set just above"))
}

fn core() -> Option<Arc<Core>> {
    guard(&CORE).clone()
}

/// What `init` does once it has its two strings: the body of the native, so the FFI wrapper is only
/// the unwind guard around it. Answers (status, body).
fn init_inner(dir: &Path, network: &str) -> (u16, Value) {
    if core().is_some() {
        return (200, json!({"ok": true}));
    }
    // Everything that needs no network happens now, so keys can be made and the page is never kept
    // waiting; the connect (up to a minute through the resolver) runs in the background.
    match prepare(dir, network) {
        Ok(()) => {
            start_connect();
            (200, json!({"ok": true, "connecting": true}))
        }
        Err(e) => (503, json!({"error": e})),
    }
}

fn prepare(dir: &Path, network: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|_| "The wallet's folder on this phone could not be made.".to_string())?;
    let net = Network::from_flag(network)?;
    let _ = HOME.set((dir.to_path_buf(), net.clone()));
    tx::use_fee_floor(&net);
    Dials::preset(net.dials, net.daa_per_sec)?;
    load_keys(dir, &net)?;
    runtime()?;
    Ok(())
}

/// Starts one background connect unless one is running or the engine is already connected.
fn start_connect() {
    if core().is_some() || CONNECTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(rt) = RT.get() else {
        CONNECTING.store(false, Ordering::SeqCst);
        return;
    };
    rt.spawn(async {
        match connect_core().await {
            Ok(core) => {
                *guard(&CORE) = Some(core);
                *guard(&LAST_ERR) = None;
            }
            Err(e) => *guard(&LAST_ERR) = Some(e),
        }
        CONNECTING.store(false, Ordering::SeqCst);
    });
}

async fn connect_core() -> Result<Arc<Core>, String> {
    let (dir, net) = HOME.get().cloned().ok_or_else(|| NOT_OPENED.to_string())?;
    let dials = Dials::preset(net.dials, net.daa_per_sec)?;
    let rpc = connect_remembered(&dir, &net).await?;
    // Keys are read after the connect, so keys made while it ran are the ones served.
    let keys = load_keys(&dir, &net)?;
    let ctx = Ctx { dir, keys, rpc: Some(rpc), force: false, network: net, dials, account: 1 };
    Core::open(ctx).map(Arc::new)
}

/// Connects through the remembered node (`node.txt`, REMEMBERED_NODE_WAIT at most), else through the public
/// resolver, and remembers whichever node answered. A remembered node that fails is forgotten.
async fn connect_remembered(dir: &Path, net: &Network) -> Result<kaspa_wrpc_client::KaspaRpcClient, String> {
    let path = dir.join(NODE_FILE);
    let remembered = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| s.starts_with("ws"));
    let mut rpc = None;
    if let Some(url) = remembered.as_deref() {
        match tokio::time::timeout(REMEMBERED_NODE_WAIT, tx::connect(Some(url), net)).await {
            Ok(Ok(r)) => rpc = Some(r),
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    let rpc = match rpc {
        Some(r) => r,
        None => tx::connect(None, net).await.map_err(|e| crate::core::plain(&e))?,
    };
    if let Some(url) = rpc.url() {
        if remembered.as_deref() != Some(url.as_str()) {
            let _ = std::fs::write(&path, url);
        }
    }
    Ok(rpc)
}

/// This folder's keys.json, public block only; empty keys before `keysNew` has run.
fn load_keys(dir: &Path, net: &Network) -> Result<Keys, String> {
    let keys_path = dir.join("keys.json");
    if !keys_path.exists() {
        return Ok(Keys::default());
    }
    let keys = Keys::load_public(&keys_path).map_err(|_| "The wallet's key file on this phone could not be read.".to_string())?;
    // A folder made for one network never runs under the other.
    net.check_recorded(&keys.public().network, "keys.json")?;
    Ok(keys)
}

/// The status the engine last built in this folder (core.rs writes it), if the file reads as JSON.
fn stale_status(account: u32) -> Option<Value> {
    let (dir, _) = HOME.get()?;
    let text = std::fs::read_to_string(dir.join(crate::core::status_cache_name(account))).ok()?;
    serde_json::from_str(&text).ok().filter(Value::is_object)
}

fn parse_args(args: &str) -> Result<Value, (u16, Value)> {
    serde_json::from_str(args).map_err(|_| (400, json!({"error": "Bad request"})))
}

/// The body of `call`: route through `Core::call` on the runtime.
fn call_inner(method: &str, args: &str) -> (u16, Value) {
    let Some(core) = core() else {
        start_connect(); // a failed connect is retried on the next call
        let connecting = CONNECTING.load(Ordering::SeqCst);
        // The last run's balances, marked stale, beat a blank Home while the connect runs.
        if method == "status" {
            let account = serde_json::from_str::<Value>(args).ok().and_then(|a| a["account"].as_u64()).unwrap_or(1) as u32;
            if let Some(mut v) = stale_status(account) {
                if let Some(o) = v.as_object_mut() {
                    o.insert("stale".into(), json!(true));
                    o.insert("connecting".into(), json!(connecting));
                }
                return (200, v);
            }
        }
        let words = guard(&LAST_ERR).clone().unwrap_or_else(|| NOT_CONNECTED.to_string());
        return (503, json!({"error": words, "connecting": connecting}));
    };
    let args = match parse_args(args) {
        Ok(v) => v,
        Err(refused) => return refused,
    };
    let Some(rt) = RT.get() else {
        return (503, json!({"error": NOT_CONNECTED}));
    };
    let answer = rt.block_on(core.call(method, &args));
    // keys.reset moved the key files away: serve empty keys on the same connection, so the page's next keysNew
    // finds a Core to reopen (keys_new_inner) and nothing of the old block is answered meanwhile.
    // heir.set rewrote the public block: the Core reopens on the keys.json now on disk.
    if (method == "keys.reset" || method == "heir.set") && answer.1["ok"] == json!(true) {
        let mut ctx = core.ctx.clone();
        ctx.keys = if method == "heir.set" {
            match load_keys(&ctx.dir, &ctx.network) {
                Ok(k) => k,
                Err(e) => return (500, json!({"error": e})),
            }
        } else {
            Keys::default()
        };
        match Core::open(ctx) {
            Ok(fresh) => *guard(&CORE) = Some(Arc::new(fresh)),
            Err(e) => return (500, json!({"error": e})),
        }
    }
    answer
}

/// The body of `callWithSecret` (D3): the slot follows the method (send: the card; stop and decline: stop
/// key 1 with the funding key; fund and purse.pay: the funding key; topup: spend + funding; takeout: two typed
/// sheets as text, or the phone's sealed safe keys when the block is 0, 64 or 96 bytes; keys.sheets: the sealed
/// safe keys), and the block is armed for this one call only.
fn call_with_secret_inner(method: &str, args: &str, block: Zeroizing<Vec<u8>>) -> (u16, Value) {
    let Some(slot) = Slot::of_method_block(method, block.len()) else {
        return (400, json!({"error": NO_SECRET_METHOD}));
    };
    let Some(core) = core() else {
        return (503, json!({"error": NOT_CONNECTED}));
    };
    let args = match parse_args(args) {
        Ok(v) => v,
        Err(refused) => return refused,
    };
    let Some(rt) = RT.get() else {
        return (503, json!({"error": NOT_CONNECTED}));
    };
    rt.block_on(core.call_with_secret(method, &args, slot, block))
}

/// The body of `keysNew` (D4): the phone-born secrets for Java to seal, with the public-only keys.json and
/// vault-public.json written in the wallet's folder. Args: `{mode, offline_public: <offline-public.json text>,
/// heir_x, auto_heir, hard_limit}`. `mode` absent or "drawer": stop keys 2 and 3 and the safe come from the
/// drawer device's file; the block is [spend 32][stop 32][funding 32], 96 bytes. "phone": everything is made on
/// this phone (`Keys::fresh_phone_all`, no file, heir_x may be empty); the block is 192 bytes, [spend 32][stop 32]
/// [funding 32][cold 32][cold2 32][cold3 32] (the phone keeps the safe's keys sealed too), followed by
/// the three safe sheets as UTF-8 JSON `{"sheets":[[24 words],[24 words],[24 words]]}`; the marker file
/// `sheets-on-phone` is written in the folder (status `sheets_on_phone`, removed by `keys.burned`). An opened
/// Core, which was serving empty keys, is reopened on the new block at once.
fn keys_new_inner(args: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let (dir, net) = HOME.get().ok_or(NOT_OPENED)?;
    let a: Value = serde_json::from_str(args).map_err(|_| "Bad request".to_string())?;
    let heir_x = a["heir_x"].as_str().unwrap_or("").trim();
    let (auto_heir, hard_limit) = (a["auto_heir"].as_bool().unwrap_or(false), a["hard_limit"].as_bool().unwrap_or(false));
    let keys_path = dir.join("keys.json");
    let mode = a["mode"].as_str().unwrap_or("drawer");
    let (keys, secrets, sheets) = match mode {
        "phone" => {
            refuse_existing(&keys_path).map_err(|_| "This phone already has keys. Start a new wallet folder first.".to_string())?;
            let (keys, mut secrets, sheets) = Keys::fresh_phone_all(net, heir_x, auto_heir, hard_limit)?;
            let block = Zeroizing::new(secrets.to_vec());
            secrets.zeroize();
            (keys, block, Some(sheets))
        }
        "drawer" => {
            let export_text = a["offline_public"].as_str().ok_or("The drawer device's file is missing from the request.")?;
            let export: OfflineExport = serde_json::from_str(export_text).map_err(|_| "That is not the file the drawer device writes (offline-public.json).".to_string())?;
            refuse_existing(&keys_path).map_err(|_| "This phone already has keys. Start a new wallet folder first.".to_string())?;
            let (keys, mut secrets) = Keys::fresh_phone(net, &export, heir_x, auto_heir, hard_limit)?;
            let block = Zeroizing::new(secrets.to_vec());
            secrets.zeroize();
            (keys, block, None)
        }
        _ => return Err("Bad request".into()),
    };
    // [96 or 192 secret bytes][sheets JSON when this phone made the safe]; the words are secrets too and are zeroed here.
    let mut block = secrets;
    if let Some(mut sheets) = sheets {
        std::fs::write(dir.join(crate::core::SHEETS_ON_PHONE_FILE), b"").map_err(|_| "The wallet's folder on this phone could not be written.".to_string())?;
        let mut text = Zeroizing::new(serde_json::to_string(&json!({"sheets": sheets})).map_err(|_| "Bad request".to_string())?);
        block.extend_from_slice(text.as_bytes());
        text.zeroize();
        for w in sheets.iter_mut().flatten() {
            w.zeroize();
        }
    }
    // Nothing secret is in `keys` now, so the plain form is right (the locked form would ask for a passphrase).
    keys.save(&keys_path, true)?;
    write_public_export(dir, &keys)?;
    if let Some(old) = core() {
        let mut ctx = old.ctx.clone();
        ctx.keys = Keys::load_public(&keys_path)?;
        *guard(&CORE) = Some(Arc::new(Core::open(ctx)?));
    }
    Ok(block)
}

/// Runs `f` behind the unwind guard and turns its answer into the envelope string for Java.
fn guarded(env: &mut JNIEnv, f: impl FnOnce(&mut JNIEnv) -> Result<(u16, Value), String>) -> jstring {
    let out = catch_unwind(AssertUnwindSafe(|| f(env)));
    let s = match out {
        Ok(Ok((status, body))) => envelope(status, body),
        Ok(Err(words)) => error_envelope(400, &words),
        Err(_) => error_envelope(500, FAULT),
    };
    match catch_unwind(AssertUnwindSafe(|| to_java(env, s))) {
        Ok(p) => p,
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kaspavault_wallet_VaultLib_init<'l>(mut env: JNIEnv<'l>, _class: JClass<'l>, dir: JString<'l>, network: JString<'l>) -> jstring {
    guarded(&mut env, |env| {
        let dir = PathBuf::from(read_java_string(env, &dir)?);
        let network = read_java_string(env, &network)?;
        Ok(init_inner(&dir, &network))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kaspavault_wallet_VaultLib_call<'l>(mut env: JNIEnv<'l>, _class: JClass<'l>, method: JString<'l>, args: JString<'l>) -> jstring {
    guarded(&mut env, |env| {
        let method = read_java_string(env, &method)?;
        let args = read_java_string(env, &args)?;
        Ok(call_inner(&method, &args))
    })
}

/// D3: the secret bytes are copied once into a zeroizing buffer and armed for this one call; Java zeroes its own copy.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kaspavault_wallet_VaultLib_callWithSecret<'l>(mut env: JNIEnv<'l>, _class: JClass<'l>, method: JString<'l>, args: JString<'l>, secret: JByteArray<'l>) -> jstring {
    guarded(&mut env, |env| {
        let method = read_java_string(env, &method)?;
        let args = read_java_string(env, &args)?;
        let block = Zeroizing::new(env.convert_byte_array(&secret).map_err(|_| "bad bytes from Java".to_string())?);
        Ok(call_with_secret_inner(&method, &args, block))
    })
}

/// D4: the 96 secret bytes for Java to seal (then the safe sheets as JSON when this phone made them), never a
/// String; an empty byte[] and one plain-words log line on a refusal (the bytes are never logged).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kaspavault_wallet_VaultLib_keysNew<'l>(mut env: JNIEnv<'l>, _class: JClass<'l>, args: JString<'l>) -> jbyteArray {
    let out = catch_unwind(AssertUnwindSafe(|| {
        let args = read_java_string(&mut env, &args)?;
        keys_new_inner(&args)
    }));
    let bytes: Zeroizing<Vec<u8>> = match out {
        Ok(Ok(block)) => block,
        Ok(Err(words)) => {
            eprintln!("keysNew: {words}");
            Zeroizing::new(vec![])
        }
        Err(_) => {
            eprintln!("keysNew: {FAULT}");
            Zeroizing::new(vec![])
        }
    };
    match catch_unwind(AssertUnwindSafe(|| env.byte_array_from_slice(&bytes))) {
        Ok(Ok(arr)) => arr.into_raw(),
        _ => std::ptr::null_mut(),
    }
}
