//! Guardians and heirs on testnet-10 (never mainnet): one owner, one helper, every step through Core::call.
//! Run: cargo test --test link_proof -- --ignored --nocapture
//! Resumable: each finished step leaves run-link/steps/<step>.json and is skipped on the next run.
//! Folders: run-link/owner, run-link/helper, run-link/source (a copy of an old testnet funding key set).
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use vault_core::contract::Dials;
use vault_core::core::{Core, Slot};
use vault_core::keys::Keys;
use vault_core::network::Network;
use vault_core::tx::{self, Ctx};
use zeroize::Zeroizing;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run-link")
}

fn say(step: &str, v: &Value) {
    let line = format!("STEP {step}: {v}");
    println!("{line}");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(root().join("proof.log")) {
        writeln!(f, "{line}").ok();
    }
}

fn done(step: &str) -> Option<Value> {
    std::fs::read_to_string(root().join("steps").join(format!("{step}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn mark(step: &str, v: &Value) {
    std::fs::create_dir_all(root().join("steps")).unwrap();
    std::fs::write(root().join("steps").join(format!("{step}.json")), serde_json::to_string_pretty(v).unwrap()).unwrap();
    say(step, v);
}

fn fail(step: &str, v: &Value) -> ! {
    say(&format!("{step} FAILED"), v);
    panic!("{step} failed: {v}");
}

/// A Core in the phone's shape: public block only; a secret rides in on the call that signs (like main.rs `call`).
fn open(dir: &Path, rpc: &kaspa_wrpc_client::KaspaRpcClient, net: &Network) -> (Core, Keys) {
    let path = dir.join("keys.json");
    let keys = if path.exists() { Keys::load(&path).unwrap() } else { Keys::default() };
    let mut held = keys.clone();
    if held.funding_x.is_none()
        && let Some(s) = &keys.funding
    {
        held.funding_x = Some(faster_hex::hex_string(&vault_core::keys::xonly(&vault_core::keys::keypair(s))));
    }
    held.wipe();
    let core = Core::open(Ctx { dir: dir.to_path_buf(), keys: held, rpc: Some(rpc.clone()), force: false, network: net.clone(), dials: Dials::testnet(), account: 1 }).unwrap();
    (core, keys)
}

async fn call(core: &Core, keys: &Keys, method: &str, args: Value) -> Value {
    let (status, body) = match Slot::of_method(method) {
        Some(slot) => core.call_with_secret(method, &args, slot, slot.block_from(keys).unwrap()).await,
        None => core.call(method, &args).await,
    };
    json!({"status": status, "body": body})
}

async fn balance(rpc: &kaspa_wrpc_client::KaspaRpcClient, net: &Network, address: &str) -> u64 {
    let ctx = Ctx { dir: root(), keys: Keys::default(), rpc: Some(rpc.clone()), force: false, network: net.clone(), dials: Dials::testnet(), account: 1 };
    let a = net.address(address).unwrap();
    ctx.try_utxos(&a).await.map(|c| c.iter().map(|(_, u)| u.amount).sum()).unwrap_or(0)
}

async fn import(owner: &Core, okeys: &Keys, helper: &Core, hkeys: &Keys, owner_dir: &Path) -> Value {
    let ex = call(owner, okeys, "guard.export", json!({"name": "Owner"})).await;
    // Once step 9 stopped the account there is no open record to export: the last export's file still serves.
    if ex["body"]["ok"] == false && done("9b-stop").is_none() {
        fail("export", &ex);
    }
    let text = std::fs::read_to_string(owner_dir.join(vault_core::core::GUARD_FILE)).unwrap();
    call(helper, hkeys, "watch.import", json!({"text": text})).await
}

const KAS: u64 = 100_000_000;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn guardians_and_heirs_on_testnet() {
    let net = Network::testnet();
    tx::use_fee_floor(&net);
    let (owner_dir, helper_dir, source_dir) = (root().join("owner"), root().join("helper"), root().join("source"));
    for d in [&owner_dir, &helper_dir, &source_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let rpc = tx::connect(None, &net).await.unwrap();

    // Owner keys (laptop shape, plain: testnet only).
    if !owner_dir.join("keys.json").exists() {
        Keys::fresh(&net).save(&owner_dir.join("keys.json"), true).unwrap();
    }
    let (owner, okeys) = open(&owner_dir, &rpc, &net);
    let purse = call(&owner, &okeys, "funding", json!({})).await;
    let purse_addr = purse["body"]["address"].as_str().unwrap().to_string();
    say("0 owner purse", &purse);

    // Step 0: money. Pay ~20 TKAS from the old funding key with the most.
    if done("0-pay").is_none() {
        let mut best: Option<(String, f64)> = None;
        for name in ["run-v6", "run", "run-v5", "run-v3"] {
            let from = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name).join("keys.json");
            let to = root().join(format!("src-{name}"));
            std::fs::create_dir_all(&to).unwrap();
            std::fs::copy(&from, to.join("keys.json")).unwrap();
            let (c, k) = open(&to, &rpc, &net);
            let f = call(&c, &k, "funding", json!({})).await;
            say(&format!("0 funding {name}"), &f);
            let kas = f["body"]["kas"].as_f64().unwrap_or(0.0);
            if best.as_ref().is_none_or(|b| kas > b.1) {
                best = Some((name.to_string(), kas));
            }
        }
        let (name, kas) = best.unwrap();
        if kas < 25.0 {
            fail("0-pay", &json!({"why": "no testnet key holds 25 TKAS", "best": name, "kas": kas}));
        }
        std::fs::copy(root().join(format!("src-{name}")).join("keys.json"), source_dir.join("keys.json")).unwrap();
        let (src, skeys) = open(&source_dir, &rpc, &net);
        let paid = call(&src, &skeys, "purse.pay", json!({"to": purse_addr, "kas": 20})).await;
        if paid["body"]["ok"] != true {
            fail("0-pay", &paid);
        }
        mark("0-pay", &json!({"from": name, "answer": paid}));
        for _ in 0..12 {
            if balance(&rpc, &net, &purse_addr).await >= 19 * KAS {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    // Step 1: helper makes the link; owner reads the code.
    let secrets: Vec<u8> = if let Ok(s) = std::fs::read(helper_dir.join("secrets.bin")) {
        s
    } else {
        let (s, link) = vault_core::link::make_link(&helper_dir, &net, "Helper").unwrap();
        std::fs::write(helper_dir.join("secrets.bin"), &s[..64]).unwrap();
        say("1 make_link", &json!({"stop_x": link.stop_x, "heir_x": link.heir_x}));
        s[..64].to_vec()
    };
    let (helper, hkeys) = open(&helper_dir, &rpc, &net);
    let code = call(&helper, &hkeys, "link.code", json!({})).await;
    // Once the block holds the helper, link.read refuses the same code as "own keys": read it only the first time.
    let read = match done("1-link") {
        Some(d) => d["read"].clone(),
        None => call(&owner, &okeys, "link.read", json!({"text": code["body"]["code"]})).await,
    };
    if read["body"]["ok"] != true {
        fail("1-link", &read);
    }
    let (stop_x, heir_x, heir_addr) = (read["body"]["stop_x"].clone(), read["body"]["heir_x"].clone(), read["body"]["heir_address"].as_str().unwrap().to_string());
    if done("1-link").is_none() {
        mark("1-link", &json!({"code": code, "read": read}));
    }

    // Step 2: people in the rules, then the opening applies them.
    if done("2-fund").is_none() {
        let ch = call(&owner, &okeys, "rules.change", json!({"guardian_x": stop_x, "guardian_which": 2, "heir_x": heir_x, "heir_delay": 3000})).await;
        say("2 rules.change", &ch);
        if ch["body"]["ok"] != true {
            fail("2-rules", &ch);
        }
        let st = call(&owner, &okeys, "status", json!({})).await;
        say("2 status people_next", &st["body"]["people_next"]);
        let f = call(&owner, &okeys, "fund", json!({"kas": 8})).await;
        if f["body"]["ok"] != true {
            fail("2-fund", &f);
        }
        let k: Value = serde_json::from_str(&std::fs::read_to_string(owner_dir.join("keys.json")).unwrap()).unwrap();
        let p = &k["public"];
        let ok = f["body"]["rules_block_changed"] == true && p["cancel2_x"] == stop_x && p["heir_x"] == heir_x;
        mark("2-fund", &json!({"ok": ok, "rules_block_changed": f["body"]["rules_block_changed"], "cancel2_x": p["cancel2_x"], "heir_x": p["heir_x"], "answer": f}));
        if !ok {
            fail("2-check", &json!({"keys_public": p}));
        }
    }
    // The rules block changed: reopen the owner on it.
    drop(owner);
    let (owner, okeys) = open(&owner_dir, &rpc, &net);

    // Step 3: export, import, status.
    // From step 11 on the helper must keep its own watched copy (step 11 proves it follows a reopen): no re-import.
    let imp = if done("10c-due").is_some() { json!({"body": {"id": done("10b-early").unwrap()["status"]["id"].clone()}}) } else { import(&owner, &okeys, &helper, &hkeys, &owner_dir).await };
    let id = imp["body"]["id"].as_str().unwrap_or_else(|| fail("3-import", &imp)).to_string();
    if done("3-import").is_none() {
        let ws = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
        let ok = imp["body"]["roles"] == json!({"guardian": 2, "heir": true}) && ws["body"]["stage"] == "open";
        mark("3-import", &json!({"ok": ok, "import": imp, "status": ws}));
        if !ok {
            fail("3-check", &ws);
        }
    }

    // Step 4: a withdrawal above the limit waits for the notice; the helper's follower sees it.
    if done("4-waiting").is_none() {
        let s = call(&owner, &okeys, "send", json!({"to": purse_addr, "kas": 3})).await;
        say("4 send", &s);
        if s["body"]["ok"] != true {
            fail("4-send", &s);
        }
        let mut last = json!(null);
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            last = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
            if last["body"]["stage"] == "waiting" {
                break;
            }
        }
        let ok = last["body"]["stage"] == "waiting" && last["body"]["pending"]["kas"].is_number();
        mark("4-waiting", &json!({"ok": ok, "send": s, "status": last}));
        if !ok {
            fail("4-check", &last);
        }
    }

    // Step 5: the guardian stops it; the coin goes to the owner's safe.
    if done("5-stop").is_none() {
        let st = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
        say("5 before", &st);
        let (c, b) = helper.call_with_secret("watch.stop", &json!({"id": id}), Slot::Guardian, Zeroizing::new(secrets[..32].to_vec())).await;
        let stop = json!({"status": c, "body": b});
        say("5 watch.stop", &stop);
        if stop["body"]["ok"] != true {
            fail("5-stop", &stop);
        }
        let mut last = json!(null);
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            last = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
            if last["body"]["stage"] == "stopped" {
                break;
            }
        }
        let ost = call(&owner, &okeys, "status", json!({})).await;
        let ok = last["body"]["stage"] == "stopped";
        mark("5-stop", &json!({"ok": ok, "stop": stop, "status": last, "owner_status": ost}));
        if !ok {
            fail("5-check", &last);
        }
    }

    // Step 6: open again (the block already holds the helper), re-import, wait out the heir clock, claim.
    if done("6-reopen").is_none() {
        let st = call(&owner, &okeys, "status", json!({})).await;
        say("6 owner status before reopen", &json!({"stopped": st["body"]["stopped"], "stage": st["body"]["stage"], "error": st["body"]["error"], "status": st["status"]}));
        let f = call(&owner, &okeys, "fund", json!({"kas": 5})).await;
        let mut account = 1;
        let mut f2 = json!(null);
        if f["body"]["ok"] != true {
            say("6 fund account 1 REFUSED (engine)", &f);
            // Workaround: a second account of the same key set opens under the same block (helper inside).
            f2 = call(&owner, &okeys, "fund", json!({"kas": 5, "account": 2})).await;
            if f2["body"]["ok"] != true {
                fail("6-fund", &json!({"account1": f, "account2": f2}));
            }
            account = 2;
        }
        mark("6-reopen", &json!({"account": account, "answer1": f, "answer2": f2}));
    }
    let account = done("6-reopen").unwrap()["account"].as_i64().unwrap();
    let imp2 = if done("6-claim").is_some() {
        json!("step 6 done: no re-import")
    } else if account == 1 {
        import(&owner, &okeys, &helper, &hkeys, &owner_dir).await
    } else {
        let ex = vault_core::core::guard_export(&owner_dir, &net, Some(&owner_dir.join(format!("state-{account}.json"))), &[], Some("Owner")).unwrap();
        say("6 export account 2", &json!({"public_sha256": ex["public_sha256"]}));
        let text = std::fs::read_to_string(owner_dir.join(vault_core::core::GUARD_FILE)).unwrap();
        call(&helper, &hkeys, "watch.import", json!({"text": text})).await
    };
    say("6 re-import", &imp2);
    if done("6-claim").is_none() {
        let mut last = json!(null);
        for _ in 0..30 {
            last = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
            say("6 waiting heir", &json!({"stage": last["body"]["stage"], "heir_seconds_left": last["body"]["heir_seconds_left"], "can_claim": last["body"]["can_claim"], "error": last["body"]["error"]}));
            if last["body"]["can_claim"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let before = balance(&rpc, &net, &heir_addr).await;
        let (c, b) = helper.call_with_secret("watch.claim", &json!({"id": id}), Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await;
        let claim = json!({"status": c, "body": b});
        say("6 watch.claim", &claim);
        if claim["body"]["ok"] != true {
            fail("6-claim", &json!({"claim": claim, "status": last}));
        }
        let mut after = before;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            after = balance(&rpc, &net, &heir_addr).await;
            if after > before {
                break;
            }
        }
        let ws = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
        mark("6-claim", &json!({"ok": after > before, "claim": claim, "heir_address": heir_addr, "heir_kas_before": before as f64 / KAS as f64, "heir_kas_after": after as f64 / KAS as f64, "status": ws}));
    }
    // Step 7: the owner after the guardian's stop (account 1, step 5): Update now = take out only the stop's coin
    // from the safe, then open the account again with it. The owner's record never saw the stop.
    if done("7-update").is_none() {
        let st = call(&owner, &okeys, "status", json!({"account": 1})).await;
        let b = &st["body"];
        say("7 owner status", &json!({"stage": b["stage"], "stopped": b["stopped"], "ended": b["ended"], "balance_kas": b["balance_kas"], "error": b["error"], "status": st["status"]}));
        // The phone hands two safe sheets in the block (Slot::Safe), never through keys.json.
        let sheets: Vec<u8> = [okeys.cold.unwrap(), okeys.cold2.unwrap()].concat();
        let (c, b) = owner.call_with_secret("takeout", &json!({"account": 1, "from_stop": true}), Slot::Safe, Zeroizing::new(sheets)).await;
        let out = json!({"status": c, "body": b});
        say("7 takeout from_stop", &out);
        if out["body"]["ok"] != true {
            fail("7-takeout", &json!({"status": st["body"], "takeout": out}));
        }
        let open_kas = out["body"]["open_kas"].as_f64().unwrap();
        for _ in 0..12 {
            if balance(&rpc, &net, &purse_addr).await as f64 >= open_kas * KAS as f64 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let f = call(&owner, &okeys, "fund", json!({"account": 1, "kas": open_kas})).await;
        say("7 fund", &f);
        let after = call(&owner, &okeys, "status", json!({"account": 1})).await;
        let ok = f["body"]["ok"] == true;
        mark("7-update", &json!({"ok": ok, "takeout": out, "fund": f, "status_after": after["body"]["history"]}));
        if !ok {
            fail("7-fund", &f);
        }
    }
    // Step 8: the heir spends what the claim paid (heir.purse, then heir.pay 2 TKAS back to the owner's purse).
    if done("8-heir-pay").is_none() {
        let hp = call(&helper, &hkeys, "heir.purse", json!({})).await;
        say("8 heir.purse", &hp);
        if hp["body"]["ok"] != true {
            fail("8-heir-purse", &hp);
        }
        let before = balance(&rpc, &net, &purse_addr).await;
        let (c, b) = helper.call_with_secret("heir.pay", &json!({"to": purse_addr, "kas": 2}), Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await;
        let paid = json!({"status": c, "body": b});
        say("8 heir.pay", &paid);
        if paid["body"]["ok"] != true {
            fail("8-heir-pay", &json!({"purse": hp, "pay": paid}));
        }
        let mut after = before;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            after = balance(&rpc, &net, &purse_addr).await;
            if after >= before + 2 * KAS {
                break;
            }
        }
        let hp2 = call(&helper, &hkeys, "heir.purse", json!({})).await;
        mark("8-heir-pay", &json!({"ok": after >= before + 2 * KAS, "heir_purse_before": hp["body"], "pay": paid, "heir_purse_after": hp2["body"], "owner_purse_gain_kas": after.saturating_sub(before) as f64 / KAS as f64}));
    }
    // Step 9: the owner removes the guardian and runs Update now (rules.change, stop, take-out, fund).
    // Resumable parts; 9d proves the opening accepts the emptied slot (and repairs a block an older build tore).
    if done("9a-remove").is_none() {
        let ch = call(&owner, &okeys, "rules.change", json!({"account": 1, "guardian_x": "", "guardian_which": 2})).await;
        say("9a rules.change remove", &ch);
        if ch["body"]["ok"] != true {
            fail("9a-remove", &ch);
        }
        mark("9a-remove", &ch);
    }
    if done("9b-stop").is_none() {
        let s = call(&owner, &okeys, "stop", json!({"account": 1})).await;
        say("9b stop", &s);
        if s["body"]["ok"] != true {
            fail("9b-stop", &s);
        }
        mark("9b-stop", &s);
    }
    if done("9c-takeout").is_none() {
        let mut out = json!(null);
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let sheets: Vec<u8> = [okeys.cold.unwrap(), okeys.cold2.unwrap()].concat();
            let (c, b) = owner.call_with_secret("takeout", &json!({"account": 1, "from_stop": true}), Slot::Safe, Zeroizing::new(sheets)).await;
            out = json!({"status": c, "body": b});
            say("9c takeout from_stop", &out);
            if out["body"]["ok"] == true || !out["body"]["error"].as_str().unwrap_or("").ends_with("Trying again in a moment.") {
                break;
            }
        }
        if out["body"]["ok"] != true {
            fail("9c-takeout", &out);
        }
        mark("9c-takeout", &out);
    }
    if done("9d-fund").is_none() {
        let open_kas = done("9c-takeout").unwrap()["body"]["open_kas"].as_f64().unwrap();
        for _ in 0..12 {
            if balance(&rpc, &net, &purse_addr).await as f64 >= open_kas * KAS as f64 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let f = call(&owner, &okeys, "fund", json!({"account": 1, "kas": open_kas})).await;
        say("9d fund", &f);
        let (owner2, _) = open(&owner_dir, &rpc, &net);
        let kp = call(&owner2, &okeys, "keys.public", json!({})).await;
        say("9d keys.public stops", &json!({"stops": kp["body"]["stops"], "cancel2_x": kp["body"]["public"]["cancel2_x"]}));
        if f["body"]["ok"] != true {
            fail("9d-fund", &json!({"fund": f, "stops": kp["body"]["stops"], "cancel2_x": kp["body"]["public"]["cancel2_x"]}));
        }
        mark("9d-fund", &json!({"ok": true, "fund": f, "stops": kp["body"]["stops"]}));
    }
    // Step 10: the claim clock. A top-up gives the account a fresh coin, so the heir clock starts again;
    // the heir presses claim at once and must get the plain "Not yet" with nothing moved, then claims when it is due.
    if done("10a-topup").is_none() {
        let t = call(&owner, &okeys, "topup", json!({"kas": 1})).await;
        say("10a topup", &t);
        if t["body"]["ok"] != true {
            fail("10a-topup", &t);
        }
        mark("10a-topup", &t);
    }
    if done("10b-early").is_none() {
        let mut st = json!(null);
        for _ in 0..12 {
            st = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
            if st["body"]["heir_seconds_left"].as_i64().unwrap_or(0) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        say("10b status", &json!({"stage": st["body"]["stage"], "heir_seconds_left": st["body"]["heir_seconds_left"], "can_claim": st["body"]["can_claim"]}));
        let before = balance(&rpc, &net, &heir_addr).await;
        let (c, b) = helper.call_with_secret("watch.claim", &json!({"id": id}), Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await;
        let early = json!({"status": c, "body": b});
        say("10b early claim", &early);
        tokio::time::sleep(Duration::from_secs(10)).await;
        let after = balance(&rpc, &net, &heir_addr).await;
        let text = early["body"]["error"].as_str().unwrap_or("");
        let ok = early["body"]["ok"] == false && text.starts_with(vault_core::watched::CLAIM_NOT_YET) && after == before;
        mark("10b-early", &json!({"ok": ok, "status": st["body"], "claim": early, "heir_kas_before": before as f64 / KAS as f64, "heir_kas_after": after as f64 / KAS as f64}));
        if !ok {
            fail("10b-early", &json!({"claim": early, "status": st["body"], "moved": after != before}));
        }
    }
    if done("10c-due").is_none() {
        let mut last = json!(null);
        for _ in 0..40 {
            last = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
            say("10c waiting heir", &json!({"heir_seconds_left": last["body"]["heir_seconds_left"], "can_claim": last["body"]["can_claim"]}));
            if last["body"]["can_claim"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let before = balance(&rpc, &net, &heir_addr).await;
        let (c, b) = helper.call_with_secret("watch.claim", &json!({"id": id}), Slot::Heir, Zeroizing::new(secrets[32..64].to_vec())).await;
        let claim = json!({"status": c, "body": b});
        say("10c claim", &claim);
        if claim["body"]["ok"] != true {
            fail("10c-due", &json!({"claim": claim, "status": last["body"]}));
        }
        let mut after = before;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            after = balance(&rpc, &net, &heir_addr).await;
            if after > before {
                break;
            }
        }
        mark("10c-due", &json!({"ok": after > before, "claim": claim, "heir_kas_before": before as f64 / KAS as f64, "heir_kas_after": after as f64 / KAS as f64}));
    }
    // Step 11: after the heir's claim ended the account, the owner opens it again under the same rules; the helper's
    // watched copy must follow the new coin at the same address without a fresh watch code.
    if done("11a-reopen").is_none() {
        let before = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
        say("11a watched before", &json!({"stage": before["body"]["stage"], "kas": before["body"]["kas"]}));
        let f = call(&owner, &okeys, "fund", json!({"account": 1, "kas": 3})).await;
        say("11a fund", &f);
        if f["body"]["ok"] != true {
            fail("11a-reopen", &f);
        }
        mark("11a-reopen", &json!({"ok": true, "watched_before": before["body"], "fund": f}));
    }
    if done("11b-follows").is_none() {
        let mut last = json!(null);
        for _ in 0..18 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            last = call(&helper, &hkeys, "watch.status", json!({"id": id})).await;
            say("11b watched", &json!({"stage": last["body"]["stage"], "kas": last["body"]["kas"], "error": last["body"]["error"]}));
            if last["body"]["stage"] == "open" && last["body"]["kas"].as_f64().unwrap_or(0.0) > 2.9 {
                break;
            }
        }
        let ok = last["body"]["stage"] == "open" && last["body"]["kas"].as_f64().unwrap_or(0.0) > 2.9;
        mark("11b-follows", &json!({"ok": ok, "status": last["body"]}));
        if !ok {
            fail("11b-follows", &last);
        }
    }
    let left = call(&owner, &okeys, "funding", json!({})).await;
    say("end owner purse", &left);
}
