//! The guard's heartbeat while a withdrawal waits, on testnet-10 (never mainnet).
//! Run: cargo test --test guard_proof -- --ignored --nocapture
//! Uses run-link/owner as link_proof leaves it (account open, nothing waiting) and ends
//! the same way. The phone calls the guard deaf when its heartbeat is older than 100 s;
//! this run measures the oldest heartbeat seen with nothing waiting, with a withdrawal
//! waiting, and after the withdrawal is declined.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use vault_core::contract::Dials;
use vault_core::core::{Core, Slot};
use vault_core::keys::Keys;
use vault_core::network::Network;
use vault_core::tx::{self, Ctx};

/// The phone's own limit (GuardService DEAF_S).
const DEAF_S: u64 = 100;
const POLL: Duration = Duration::from_secs(3);

fn owner_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run-link").join("owner")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn open(dir: &Path, rpc: &kaspa_wrpc_client::KaspaRpcClient, net: &Network) -> (Core, Keys) {
    let keys = Keys::load(&dir.join("keys.json")).unwrap();
    let mut held = keys.clone();
    if held.funding_x.is_none()
        && let Some(s) = &keys.funding
    {
        let x = vault_core::keys::xonly(&vault_core::keys::keypair(s));
        held.funding_x = Some(faster_hex::hex_string(&x));
    }
    held.wipe();
    let (dir, rpc, network) = (dir.to_path_buf(), Some(rpc.clone()), net.clone());
    let dials = Dials::testnet();
    let ctx = Ctx { dir, keys: held, rpc, force: false, network, dials, account: 1 };
    (Core::open(ctx).unwrap(), keys)
}

async fn call(core: &Core, keys: &Keys, method: &str, args: Value) -> Value {
    let (status, body) = match Slot::of_method(method) {
        Some(slot) => {
            let block = slot.block_from(keys).unwrap();
            core.call_with_secret(method, &args, slot, block).await
        }
        None => core.call(method, &args).await,
    };
    println!("STEP {method}: {status} {}", body.to_string().chars().take(300).collect::<String>());
    json!({"status": status, "body": body})
}

/// How old the guard's heartbeat is right now; None before its first good answer.
async fn age(core: &Core) -> Option<u64> {
    let (_, body) = core.call("guard.status", &json!({})).await;
    body["seen_at"].as_u64().map(|t| now().saturating_sub(t))
}

/// Reads the heartbeat every 3 s for `secs` seconds; answers the oldest one seen.
async fn oldest(core: &Core, secs: u64, label: &str) -> u64 {
    let mut worst = 0;
    for _ in 0..secs / POLL.as_secs() {
        tokio::time::sleep(POLL).await;
        let a = age(core).await.unwrap_or(u64::MAX);
        if a > 30 && a > worst {
            println!("SLOW {label}: heartbeat {a} s old");
        }
        worst = worst.max(a);
    }
    println!("PHASE {label}: oldest heartbeat {worst} s over {secs} s");
    worst
}

/// Polls status (at most 18 times, 10 s apart) until a withdrawal is waiting or not, as asked.
async fn settle(core: &Core, keys: &Keys, want_pending: bool) -> Value {
    let mut last = json!(null);
    for _ in 0..18 {
        last = core.call("status", &json!({"account": 1})).await.1;
        let open = last["stopped"] != true && last["balance_kas"].is_number();
        if open && last["pending"].is_null() != want_pending {
            break;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let _ = keys;
    let (b, p, s) = (&last["balance_kas"], &last["pending"], &last["stopped"]);
    println!("STATUS balance {b} pending {p} stopped {s}");
    last
}

/// Starts the guard the way the phone does and waits (at most 90 s) for its first fresh heartbeat.
async fn start_guard(core: &Core, keys: &Keys) {
    let started = call(core, keys, "guard.start", json!({})).await;
    assert_eq!(started["body"]["ok"], true, "guard.start refused");
    for _ in 0..30 {
        if age(core).await.is_some_and(|a| a < 10) {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("the guard gave no heartbeat in 90 s");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn heartbeat_holds_while_a_withdrawal_waits() {
    let net = Network::testnet();
    tx::use_fee_floor(&net);
    let rpc = tx::connect(None, &net).await.unwrap();
    let (owner, keys) = open(&owner_dir(), &rpc, &net);
    let first = settle(&owner, &keys, false).await;
    let open = first["pending"].is_null() && first["stopped"] != true;
    assert!(open, "the account must start open with nothing waiting");
    let funding = call(&owner, &keys, "funding", json!({})).await;
    let purse = funding["body"]["address"].as_str().unwrap().to_string();
    start_guard(&owner, &keys).await;
    let quiet = oldest(&owner, 60, "nothing waiting").await;
    let sent = call(&owner, &keys, "send", json!({"account": 1, "to": purse, "kas": 1.5})).await;
    assert_eq!(sent["status"], 200, "the request was refused");
    let waiting = oldest(&owner, 420, "withdrawal waiting").await;
    let seen = settle(&owner, &keys, true).await;
    assert!(!seen["pending"].is_null(), "the withdrawal never showed as waiting");
    let back = call(&owner, &keys, "decline", json!({"account": 1})).await;
    assert_eq!(back["body"]["ok"], true, "the decline was refused");
    let after = oldest(&owner, 120, "after the decline").await;
    let end = settle(&owner, &keys, false).await;
    call(&owner, &keys, "guard.stop", json!({})).await;
    let ages = format!("nothing waiting {quiet} s, waiting {waiting} s, after decline {after} s");
    println!("RESULT oldest heartbeat: {ages} (deaf at {DEAF_S} s)");
    assert!(end["pending"].is_null() && end["stopped"] != true, "the account did not end open");
    assert!(quiet < DEAF_S && waiting < DEAF_S && after < DEAF_S, "the guard went deaf");
}
