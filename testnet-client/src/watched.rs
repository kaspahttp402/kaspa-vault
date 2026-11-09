//! Accounts this phone watches for other people (docs/LINK_DESIGN.md "Helper side: watched accounts"). An
//! owner's `guard.export` text becomes a watch-only folder `<wallet>/watch/<id>/` (core.rs `guard_import`), and
//! this phone's link.json says which role it holds there: guardian (stop slot 2 or 3) and/or heir. The guardian
//! and heir secrets ride in for one call (`Core::call_with_secret`), go into a Ctx on that folder only, and are
//! wiped when the move ends; they never reach the wallet's own Core.

use crate::core::{Core, NOT_GUARD_FILE, guard_build, guard_import, plain, refusal};
use crate::keys::{Keys, Public, try_xonly_hex};
use crate::link::LinkPublic;
use crate::tx::Ctx;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The folder under the wallet folder that holds one folder per watched account.
pub const WATCH_DIR: &str = "watch";
/// `{name, roles, address}` beside a watched account's files. Not `watch.json`: that is the watcher's own file
/// (watch.rs), which a guard run on this folder would write.
pub const WATCHED_FILE: &str = "watched.json";
const ID_LEN: usize = 12;

pub const BAD_ID: &str = "That is not one of the accounts this phone watches.";
pub const NOT_WATCHED: &str = "This phone does not watch that account.";
pub const NOT_GUARDIAN: &str = "This phone is not a guardian of that account.";
pub const WRONG_GUARDIAN: &str = "This phone is not set up as that account's guardian.";
pub const NOT_HEIR: &str = "This phone is not the heir of that account.";
pub const CLAIM_NOT_YET: &str = "This account cannot be claimed yet. The inactivity period ends in about";
pub const CLAIM_ENDED: &str = "This account was already stopped or passed on: there is nothing left to receive.";
pub const NEEDS_KEY: &str = "This needs this phone to be set up as the guardian or heir.";
const WATCH_UNWRITABLE: &str = "The account's folder on this phone could not be written.";
pub const ID_TAKEN: &str = "This phone already watches a different account with the same short \
     code. Nothing was changed. Forget that account first to watch this one.";

/// `watch.*` without a secret (core.rs `Core::call`): import, list, forget offline; status on the network.
pub async fn call(core: &Core, method: &str, args: &Value) -> (u16, Value) {
    let dir = &core.ctx.dir;
    let answer = match method {
        "watch.import" => import(dir, &core.ctx.network, args).inspect(|made| follow(core, made["id"].as_str().unwrap_or(""), true)),
        "watch.list" => {
            let all = list(dir);
            for w in all["watched"].as_array().into_iter().flatten() {
                follow(core, w["id"].as_str().unwrap_or(""), false);
            }
            Ok(all)
        }
        "watch.forget" => {
            let id = args["id"].as_str().unwrap_or("");
            if let Ok(f) = folder(dir, id) {
                crate::core::follow_stop(&f);
            }
            forget(dir, id)
        }
        "watch.status" => {
            let id = args["id"].as_str().unwrap_or("");
            follow(core, id, false);
            return (200, status(core, id).await);
        }
        "watch.code" => {
            let id = args["id"].as_str().unwrap_or("");
            return (200, code(dir, &core.ctx.network, id));
        }
        "watch.stop" | "watch.decline" | "watch.claim" => Err(NEEDS_KEY.into()),
        _ => return (404, refusal("Not found")),
    };
    (200, answer.unwrap_or_else(|e| refusal(&e)))
}

/// `watch.code {id}`: the watched account's watch code again (public data only: guard.export of
/// this phone's copy), so another phone can watch the account or take its funds out. Answers
/// {ok, watch_code, error}; an id this phone does not watch is BAD_ID.
fn code(dir: &Path, network: &crate::network::Network, id: &str) -> Value {
    let fail = |e: &str| json!({"ok": false, "watch_code": null, "error": plain(e)});
    let Ok(f) = folder(dir, id) else { return fail(BAD_ID) };
    if !f.join("keys.json").is_file() {
        return fail(BAD_ID);
    }
    let record = std::fs::read_to_string(f.join(WATCHED_FILE)).ok();
    let record: Option<Value> = record.and_then(|t| serde_json::from_str(&t).ok());
    let name = record.as_ref().and_then(|v| v["name"].as_str()).filter(|n| !n.is_empty());
    match guard_build(&f, network, None, &[], name) {
        Ok(out) => json!({"ok": true, "watch_code": out.to_string(), "error": null}),
        Err(_) => fail(NOT_GUARD_FILE),
    }
}

/// The id as a folder name: exactly 12 lowercase hex, never a path.
fn folder(dir: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != ID_LEN || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(BAD_ID.into());
    }
    Ok(dir.join(WATCH_DIR).join(id))
}

/// This phone's roles on an account with rule block `p`, by its link.json: guardian = the stop slot (2 or 3)
/// holding the link's stop key, heir = the link's heir key is the account's heir. No link = no roles.
pub fn roles_of(link: Option<&LinkPublic>, p: &Public) -> (Option<u8>, bool) {
    let Some(l) = link else { return (None, false) };
    let guardian = if l.stop_x.is_empty() {
        None
    } else if l.stop_x == p.cancel2_x {
        Some(2)
    } else if l.stop_x == p.cancel3_x {
        Some(3)
    } else {
        None
    };
    (guardian, !l.heir_x.is_empty() && l.heir_x == p.heir_x)
}

fn roles_json((guardian, heir): (Option<u8>, bool)) -> Value {
    json!({"guardian": guardian, "heir": heir})
}

/// The live roles of the account in `folder` (link.json may have changed since the import).
fn roles_at(dir: &Path, folder: &Path) -> Result<(Keys, (Option<u8>, bool)), String> {
    let keys = Keys::load_public(&folder.join("keys.json")).map_err(|_| NOT_WATCHED.to_string())?;
    let link = crate::link::load(dir).ok();
    let roles = roles_of(link.as_ref(), &keys.public());
    Ok((keys, roles))
}

/// Whether an import with rule block code `sha` may replace `target`: the folder is free, holds
/// no readable public block (not a watched account; `list` skips it), or holds the same account.
/// Two accounts can share the 12-hex id, never the full code: a matching id alone replaces nothing.
fn may_replace(target: &Path, sha: &str) -> bool {
    match Keys::load_public(&target.join("keys.json")) {
        Ok(k) => k.public().sha256() == sha,
        Err(_) => true,
    }
}

fn read_watched(folder: &Path) -> Value {
    std::fs::read_to_string(folder.join(WATCHED_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
}

/// `watch.import {text, name?}`: the owner's guard.export text into `<dir>/watch/<id>/`, id = the first 12 hex
/// of its rule block code. The same account again refreshes it: the new copy is made beside the old one and
/// replaces it only once whole. `name` overrides the owner's `owner_name`. Answers `{ok, id, name, roles, address}`.
pub fn import(dir: &Path, network: &crate::network::Network, args: &Value) -> Result<Value, String> {
    let text = args["text"].as_str().ok_or_else(|| NOT_GUARD_FILE.to_string())?;
    let file: Value = serde_json::from_str(text.trim()).map_err(|_| NOT_GUARD_FILE.to_string())?;
    let sha = file["public_sha256"].as_str().unwrap_or("").to_ascii_lowercase();
    let id = sha.get(..ID_LEN).ok_or_else(|| NOT_GUARD_FILE.to_string())?;
    let target = folder(dir, id).map_err(|_| NOT_GUARD_FILE.to_string())?;
    if !may_replace(&target, &sha) {
        return Err(ID_TAKEN.into());
    }
    let fresh = dir.join(WATCH_DIR).join(format!("{id}.new"));
    std::fs::remove_dir_all(&fresh).ok();
    std::fs::create_dir_all(&fresh).map_err(|_| WATCH_UNWRITABLE.to_string())?;
    let made = match guard_import(&fresh, network, text) {
        Ok(m) => m,
        Err(e) => {
            std::fs::remove_dir_all(&fresh).ok();
            return Err(e);
        }
    };
    // A name is kept only when it is plain (crate::link::name_ok); otherwise the account has none.
    let plain_name = |n: &&str| crate::link::name_ok(n);
    let given = args["name"].as_str().map(str::trim).filter(plain_name);
    let owner = file["owner_name"].as_str().map(str::trim).filter(plain_name);
    let name = given.or(owner).unwrap_or("").to_string();
    let link = crate::link::load(dir).ok();
    let public: Public = serde_json::from_value(file["public"].clone()).map_err(|_| NOT_GUARD_FILE.to_string())?;
    let roles = roles_json(roles_of(link.as_ref(), &public));
    let address = made["address"].clone();
    let record = json!({"name": name, "roles": roles, "address": address});
    std::fs::write(fresh.join(WATCHED_FILE), record.to_string()).map_err(|_| WATCH_UNWRITABLE.to_string())?;
    // The old copy's follower stops before the swap, so it can never write a move into the fresh copy.
    crate::core::follow_stop(&target);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|_| WATCH_UNWRITABLE.to_string())?;
    }
    std::fs::rename(&fresh, &target).map_err(|_| WATCH_UNWRITABLE.to_string())?;
    Ok(json!({"ok": true, "id": id, "name": name, "roles": roles, "address": address}))
}

/// `watch.list {}`: `{ok, watched: [{id, name, roles, address}]}`, sorted by id; roles read live from link.json.
pub fn list(dir: &Path) -> Value {
    let mut watched = vec![];
    if let Ok(entries) = std::fs::read_dir(dir.join(WATCH_DIR)) {
        for e in entries.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            let Ok(f) = folder(dir, &id) else { continue };
            let Ok((_, roles)) = roles_at(dir, &f) else { continue };
            let w = read_watched(&f);
            watched.push(json!({"id": id, "name": w["name"].as_str().unwrap_or(""), "roles": roles_json(roles), "address": w["address"]}));
        }
    }
    watched.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    json!({"ok": true, "watched": watched})
}

/// `watch.forget {id}`: deletes the watched account's folder (public files only; a watch folder holds no secret).
pub fn forget(dir: &Path, id: &str) -> Result<Value, String> {
    let f = folder(dir, id)?;
    if !f.is_dir() {
        return Err(NOT_WATCHED.into());
    }
    std::fs::remove_dir_all(&f).map_err(|_| WATCH_UNWRITABLE.to_string())?;
    Ok(json!({"ok": true, "id": id}))
}

/// Starts the account's follow-only guard (core.rs `follow_start`), which writes each move of the owner's coin
/// into the folder's state.json (watch.rs `follow_record`); `restart` replaces a running one (a fresh import).
/// Nothing starts for an account whose record already ended (a stop, an inheritance, an emptied vault), or
/// without the wallet's node link.
fn follow(core: &Core, id: &str, restart: bool) {
    let Ok(f) = folder(&core.ctx.dir, id) else { return };
    if core.ctx.rpc.is_none() || !f.join(WATCHED_FILE).exists() {
        return;
    }
    let Ok((keys, _)) = roles_at(&core.ctx.dir, &f) else { return };
    let ctx = watch_ctx(core, &f, keys);
    match ctx.try_load_vault() {
        Ok(Some(v)) if !record_ended(&v) => {}
        _ => return,
    }
    crate::core::follow_start(ctx, restart);
}

/// A Ctx on a watched account's folder: its public keys, this wallet's network and node link, account 1.
pub(crate) fn watch_ctx(core: &Core, folder: &Path, keys: Keys) -> Ctx {
    Ctx { dir: folder.to_path_buf(), keys, rpc: core.ctx.rpc.clone(), force: false, network: core.ctx.network.clone(), dials: core.ctx.dials.clone(), account: 1 }
}

/// The account's record ended: a stop, an inheritance, or an emptied vault.
fn record_ended(v: &crate::contract::Vault) -> bool {
    crate::contract::is_stopped(v) || v.history.last().is_some_and(|h| h.ends_with(crate::watch::EMPTIED))
}

fn status_failed(id: &str, name: &str, roles: Value, address: Value, error: &str) -> Value {
    json!({"ok": false, "id": id, "name": name, "roles": roles, "address": address, "kas": null, "stage": null, "pending": null,
        "heir_seconds_left": null, "heir_due": false, "can_stop": false, "can_claim": false,
        "alerts": [], "error": error})
}

/// `watch.status {id}`: the account's own status (core.rs `status`, run on a Core over the watch folder) in
/// the helper's shape (`map_status`).
pub async fn status(core: &Core, id: &str) -> Value {
    let f = match folder(&core.ctx.dir, id) {
        Ok(f) => f,
        Err(e) => return status_failed(id, "", Value::Null, Value::Null, &e),
    };
    let (keys, roles) = match roles_at(&core.ctx.dir, &f) {
        Ok(r) => r,
        Err(e) => return status_failed(id, "", Value::Null, Value::Null, &e),
    };
    let w = read_watched(&f);
    let name = w["name"].as_str().unwrap_or("").to_string();
    if let Err(e) = core.connected().await {
        return status_failed(id, &name, roles_json(roles), w["address"].clone(), &plain(&e));
    }
    let (code, st) = core.beside(watch_ctx(core, &f, keys)).status().await;
    if code != 200 {
        return status_failed(id, &name, roles_json(roles), w["address"].clone(), st["error"].as_str().unwrap_or("The account could not be read right now."));
    }
    map_status(id, &name, roles, &w["address"], &st)
}

/// core.rs's status answer in the shape GuardService.java reads. Stage: `stopped` when the account's last
/// record line is a stop (its money went to the owner's safe), `closed` when it ended any other way (passed to
/// the heir, emptied, or gone from the chain), `waiting` while a withdrawal waits, else `open`.
/// `alerts`: the follow-only guard's alerts.json for this folder as core.rs `with_watcher`
/// put them (newest first, at most 10).
pub fn map_status(id: &str, name: &str, roles: (Option<u8>, bool), address: &Value, st: &Value) -> Value {
    let stopped = st["stopped"].as_bool().unwrap_or(false);
    let pending = &st["pending"];
    let last = st["history"].as_array().and_then(|h| h.last()).and_then(|h| h["text"].as_str()).unwrap_or("");
    let stage = match (stopped, pending.is_null()) {
        (true, _) if last.starts_with("cancel") => "stopped",
        (true, _) => "closed",
        // the follower's record of a withdrawal that emptied the vault (watch.rs `follow_record`)
        (false, _) if last.ends_with(crate::watch::EMPTIED) => "closed",
        (false, false) => "waiting",
        (false, true) => "open",
    };
    let pending = if stopped || pending.is_null() {
        Value::Null
    } else {
        json!({"kas": pending["amount_kas"], "to": pending["to"], "ends_in_seconds": pending["seconds_left"]})
    };
    let heir_left = if stopped { Value::Null } else { st["heir_seconds_left"].clone() };
    let heir_due = heir_left.as_i64().is_some_and(|s| s <= 0);
    let can_stop = roles.0.is_some() && matches!(stage, "open" | "waiting");
    json!({"ok": true, "id": id, "name": name, "roles": roles_json(roles), "address": address, "kas": st["balance_kas"],
        "stage": stage, "pending": pending, "heir_seconds_left": heir_left, "heir_due": heir_due,
        "can_stop": can_stop, "can_claim": roles.1 && heir_due, "alerts": alerts_of(st),
        "error": null})
}

/// The status's alerts (an array), else none.
fn alerts_of(st: &Value) -> Value {
    st["alerts"].as_array().map_or_else(|| json!([]), |a| Value::Array(a.clone()))
}

/// A Ctx whose keys hold one helper secret; the secret is wiped when it drops, on every path.
pub(crate) struct Armed(pub(crate) Ctx);
impl Drop for Armed {
    fn drop(&mut self) {
        self.0.keys.wipe();
    }
}

/// The watched account's keys armed with this phone's guardian secret in its stop slot (2 or 3), after the
/// checks: the account is watched, this phone is its guardian, and the secret is the link's guardian key.
fn arm_guardian(dir: &Path, id: &str, secret: &[u8; 32]) -> Result<(Keys, u8), String> {
    let f = folder(dir, id)?;
    let (mut keys, (guardian, _)) = roles_at(dir, &f)?;
    let which = guardian.ok_or(NOT_GUARDIAN)?;
    let link = crate::link::load(dir).map_err(|_| NOT_GUARDIAN.to_string())?;
    if try_xonly_hex(secret).as_ref() != Some(&link.stop_x) {
        return Err(WRONG_GUARDIAN.into());
    }
    if which == 2 {
        keys.cancel2 = Some(*secret);
    } else {
        keys.cancel3 = Some(*secret);
    }
    Ok((keys, which))
}

/// The watched account's keys armed with this phone's heir secret, after the same kind of checks.
fn arm_heir(dir: &Path, id: &str, secret: &[u8; 32]) -> Result<Keys, String> {
    let f = folder(dir, id)?;
    let (mut keys, (_, heir)) = roles_at(dir, &f)?;
    if !heir {
        return Err(NOT_HEIR.into());
    }
    let link = crate::link::load(dir).map_err(|_| NOT_HEIR.to_string())?;
    if try_xonly_hex(secret).as_ref() != Some(&link.heir_x) {
        return Err(crate::link::WRONG_HEIR.into());
    }
    keys.heir = Some(*secret);
    Ok(keys)
}

/// `watch.stop {id}` / `watch.decline {id}` (Slot Guardian) and `watch.claim {id}` (Slot Heir). Stop pays the
/// owner's safe (commands.rs do_cancel; the contract allows nothing else), decline sends a waiting withdrawal
/// back, claim pays the heir (do_inherit; run on the Ctx directly, as a Core never holds a heir key). Each runs
/// under the wallet's money lock. Answers `{ok, tx, error}`.
pub async fn call_with_secret(core: &Core, method: &str, args: &Value, secret: Zeroizing<[u8; 32]>) -> (u16, Value) {
    let dir = &core.ctx.dir;
    let id = args["id"].as_str().unwrap_or("");
    let armed = match method {
        "watch.stop" | "watch.decline" => arm_guardian(dir, id, &secret),
        "watch.claim" => arm_heir(dir, id, &secret).map(|k| (k, 0)),
        _ => return (404, refusal("Not found")),
    };
    drop(secret);
    let (keys, which) = match armed {
        Ok(a) => a,
        Err(e) => return (200, refusal(&e)),
    };
    let ctx = Armed(watch_ctx(core, &dir.join(WATCH_DIR).join(id), keys));
    if let Err(e) = core.connected().await {
        return (200, json!({"ok": false, "tx": null, "error": plain(&e)}));
    }
    // The heir clock first, in plain words (the node would only answer a failed script).
    if method == "watch.claim"
        && let Some(e) = claim_too_early(&status(core, id).await)
    {
        return (200, json!({"ok": false, "tx": null, "error": e}));
    }
    let lock = core.money_lock();
    let _money = lock.lock().await;
    let done = match method {
        "watch.stop" => ctx.0.do_cancel(which).await,
        "watch.decline" => ctx.0.do_decline(which).await,
        _ => ctx.0.do_inherit().await,
    };
    drop(ctx);
    (200, match done {
        Ok(tx) => json!({"ok": true, "tx": tx, "error": null}),
        Err(e) => json!({"ok": false, "tx": null, "error": plain(&e)}),
    })
}

/// `watch.claim` asked while the watched account says the heir clock still runs, or after it ended: the plain
/// refusal. None lets the claim go to the node (due, or the status could not tell). Pure.
fn claim_too_early(st: &Value) -> Option<String> {
    if st["stage"] == "stopped" {
        return Some(CLAIM_ENDED.into());
    }
    match st["heir_seconds_left"].as_i64() {
        Some(s) if s > 0 => Some(format!("{CLAIM_NOT_YET} {}.", crate::contract::human_time(s as f64))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_waits_for_the_heir_clock() {
        let early = claim_too_early(&json!({"stage": "open", "heir_seconds_left": 3 * 86_400})).unwrap();
        assert!(early.starts_with(CLAIM_NOT_YET) && early.ends_with("3 days."), "{early}");
        assert_eq!(claim_too_early(&json!({"stage": "stopped", "heir_seconds_left": null})), Some(CLAIM_ENDED.to_string()));
        assert_eq!(claim_too_early(&json!({"stage": "waiting", "heir_seconds_left": 0})), None, "due");
        assert_eq!(claim_too_early(&json!({"stage": "open", "heir_seconds_left": null, "error": "x"})), None, "unknown: the node decides");
    }
    use crate::contract::*;
    use crate::core::{GUARD_FILE, Slot, guard_export};
    use crate::network::Network;
    use kaspa_consensus_core::Hash;
    use kaspa_consensus_core::tx::{TransactionOutpoint, TransactionOutput, UtxoEntry};
    use silverscript_abi::ArtifactValue as V;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kv-watched-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An owner's folder (full keys, a Locked account) and its guard.export text, named "Owner One".
    fn owner(tag: &str) -> (PathBuf, Keys, String) {
        let net = Network::testnet();
        let a = tmp(tag);
        let full = Keys::fresh(&net);
        full.save(&a.join("keys.json"), true).unwrap();
        let sha = pin_source(&a).unwrap();
        let v = Vault { covenant_id: Hash::from_bytes([9u8; 32]), state: locked(), pending_to: None, history: vec!["fund".into()], contract_sha256: Some(sha), network: net.name.into(), dials: Dials::testnet() };
        save_vault_anchored(&a.join("state.json"), &v, &full.public().sha256()).unwrap();
        guard_export(&a, &net, None, &[], Some("Owner One")).unwrap();
        let text = std::fs::read_to_string(a.join(GUARD_FILE)).unwrap();
        (a, full, text)
    }

    fn write_link(dir: &Path, stop_x: &str, heir_x: &str) {
        let l = LinkPublic { kind: crate::link::LINK_PUBLIC_KIND.into(), v: 1, network: "testnet-10".into(), name: "Helper".into(), stop_x: stop_x.into(), heir_x: heir_x.into(), heir_address: String::new(), already: Default::default() };
        std::fs::write(dir.join(crate::link::LINK_FILE), serde_json::to_string(&l).unwrap()).unwrap();
    }

    fn link(stop_x: &str, heir_x: &str) -> LinkPublic {
        LinkPublic { kind: String::new(), v: 1, network: String::new(), name: String::new(), stop_x: stop_x.into(), heir_x: heir_x.into(), heir_address: String::new(), already: Default::default() }
    }

    #[test]
    fn roles_follow_the_link() {
        let p = Keys::fresh(&Network::testnet()).public();
        let other = "ab".repeat(32);
        assert_eq!(roles_of(Some(&link(&p.cancel2_x, &other)), &p), (Some(2), false));
        assert_eq!(roles_of(Some(&link(&p.cancel3_x, &other)), &p), (Some(3), false));
        assert_eq!(roles_of(Some(&link(&other, &p.heir_x)), &p), (None, true));
        assert_eq!(roles_of(Some(&link(&p.cancel_x, "")), &p), (None, false), "stop key 1 is the owner's own");
        assert_eq!(roles_of(None, &p), (None, false), "no link.json, no roles");
    }

    #[test]
    fn import_list_refresh_forget() {
        let net = Network::testnet();
        let (a, full, text) = owner("imp-a");
        let w = tmp("imp-w");
        let p = full.public();
        // no link.json: a plain watcher
        let r = import(&w, &net, &json!({"text": text})).unwrap();
        let id = r["id"].as_str().unwrap().to_string();
        assert_eq!(id, p.sha256()[..12]);
        assert_eq!((r["name"].as_str(), &r["roles"]), (Some("Owner One"), &json!({"guardian": null, "heir": false})));
        assert!(r["address"].as_str().unwrap().starts_with("kaspatest:"));
        // guardian in slot 2 and heir: the same account again refreshes the folder, `name` overrides
        write_link(&w, &p.cancel2_x, &p.heir_x);
        std::fs::write(w.join(WATCH_DIR).join(&id).join("junk"), "x").unwrap();
        let r = import(&w, &net, &json!({"text": text, "name": "Mom"})).unwrap();
        assert_eq!((r["id"].as_str(), r["name"].as_str(), &r["roles"]), (Some(id.as_str()), Some("Mom"), &json!({"guardian": 2, "heir": true})));
        assert!(!w.join(WATCH_DIR).join(&id).join("junk").exists(), "a refresh replaces the old files");
        assert!(!Keys::load_public(&w.join(WATCH_DIR).join(&id).join("keys.json")).unwrap().has_secret());
        write_link(&w, &p.cancel3_x, "");
        let l = list(&w);
        assert_eq!(l["watched"], json!([{"id": id, "name": "Mom", "roles": {"guardian": 3, "heir": false}, "address": r["address"]}]), "roles read live");
        assert_eq!(import(&w, &net, &json!({"text": "nope"})).unwrap_err(), NOT_GUARD_FILE);
        for bad in ["../x", "..", "", "ABCDEF012345", "0123456789abc", "../../keys.j"] {
            assert_eq!(forget(&w, bad).unwrap_err(), BAD_ID, "{bad}");
        }
        assert_eq!(forget(&w, &id).unwrap(), json!({"ok": true, "id": id}));
        assert_eq!(list(&w)["watched"], json!([]));
        assert_eq!(forget(&w, &id).unwrap_err(), NOT_WATCHED);
        for d in [&a, &w] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn import_never_replaces_another_account_that_shares_the_short_id() {
        let net = Network::testnet();
        let (a, full_a, text_a) = owner("clash-a");
        let (b, full_b, text_b) = owner("clash-b");
        let w = tmp("clash-w");
        // account A's folder sits under B's 12-hex id, as a crafted prefix collision would put it
        let id_a = imported(&w, &text_a);
        let id_b = full_b.public().sha256()[..ID_LEN].to_string();
        std::fs::rename(w.join(WATCH_DIR).join(&id_a), w.join(WATCH_DIR).join(&id_b)).unwrap();
        assert_eq!(import(&w, &net, &json!({"text": text_b})).unwrap_err(), ID_TAKEN);
        let kept = Keys::load_public(&w.join(WATCH_DIR).join(&id_b).join("keys.json")).unwrap();
        assert_eq!(kept.public().sha256(), full_a.public().sha256(), "A's folder is untouched");
        let half = w.join(WATCH_DIR).join(format!("{id_b}.new"));
        assert!(!half.exists(), "nothing half-made is left");
        // the same account again still refreshes; a folder with no readable keys is no account
        std::fs::rename(w.join(WATCH_DIR).join(&id_b), w.join(WATCH_DIR).join(&id_a)).unwrap();
        assert_eq!(imported(&w, &text_a), id_a);
        std::fs::remove_file(w.join(WATCH_DIR).join(&id_a).join("keys.json")).unwrap();
        assert_eq!(imported(&w, &text_a), id_a);
        for d in [&a, &b, &w] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    #[test]
    fn status_maps_to_the_helpers_shape() {
        let addr = json!("kaspatest:qx");
        let open = json!({"stopped": false, "balance_kas": 5.0, "pending": null, "heir_seconds_left": 3600, "history": [{"text": "fund", "tx": null}]});
        let s = map_status("abc", "Owner One", (Some(2), false), &addr, &open);
        assert_eq!(s, json!({"ok": true, "id": "abc", "name": "Owner One", "roles": {"guardian": 2, "heir": false}, "address": "kaspatest:qx", "kas": 5.0,
            "stage": "open", "pending": null, "heir_seconds_left": 3600, "heir_due": false,
            "can_stop": true, "can_claim": false, "alerts": [], "error": null}));
        let bell = json!({"at": 9, "kind": "withdrawal_pending", "text": "A withdrawal started."});
        let mut alerted = open.clone();
        alerted["alerts"] = json!([bell]);
        let s = map_status("abc", "", (Some(2), false), &addr, &alerted);
        assert_eq!(s["alerts"], json!([bell]), "the watched folder's alerts pass through");
        let waiting = json!({"stopped": false, "balance_kas": 5.0, "pending": {"to": "kaspatest:qthief", "amount_kas": 2.0, "seconds_left": 600, "tx": "t"}, "heir_seconds_left": 0, "history": []});
        let s = map_status("abc", "", (None, true), &addr, &waiting);
        assert_eq!((s["stage"].as_str(), &s["pending"]), (Some("waiting"), &json!({"kas": 2.0, "to": "kaspatest:qthief", "ends_in_seconds": 600})));
        assert_eq!((s["heir_due"].as_bool(), s["can_claim"].as_bool(), s["can_stop"].as_bool()), (Some(true), Some(true), Some(false)), "heir clock passed; no guardian role");
        let s = map_status("abc", "", (Some(3), false), &addr, &waiting);
        assert_eq!((s["can_stop"].as_bool(), s["can_claim"].as_bool()), (Some(true), Some(false)), "a guardian can stop a waiting withdrawal; due but not the heir");
        let stopped = json!({"stopped": true, "balance_kas": 0.0, "pending": null, "heir_seconds_left": null, "history": [{"text": "fund", "tx": null}, {"text": "cancel (stop key 2): everything to cold", "tx": "t"}]});
        let s = map_status("abc", "", (Some(2), true), &addr, &stopped);
        assert_eq!((s["stage"].as_str(), s["can_stop"].as_bool(), s["can_claim"].as_bool(), &s["heir_seconds_left"]), (Some("stopped"), Some(false), Some(false), &Value::Null));
        let gone = json!({"stopped": true, "balance_kas": 0.0, "pending": null, "heir_seconds_left": null, "history": []});
        assert_eq!(map_status("abc", "", (Some(2), true), &addr, &gone)["stage"], "closed");
        let inherited = json!({"stopped": true, "pending": null, "heir_seconds_left": null, "history": [{"text": "inherit everything to heir", "tx": "t"}]});
        assert_eq!(map_status("abc", "", (None, true), &addr, &inherited)["stage"], "closed");
    }

    /// stop/decline/claim refuse before any network: no role, a wrong secret, a bad id. Then the guardian-signed
    /// stop with stop slot 2 builds and passes the script engine offline, paying the owner's safe.
    #[tokio::test]
    async fn guardian_moves_refuse_without_the_role_or_key_and_build_with_them() {
        let net = Network::testnet();
        let (a, full, text) = owner("stop-a");
        let w = tmp("stop-w");
        let p = full.public();
        let core = Core::open(Ctx { dir: w.clone(), keys: Keys::default(), rpc: None, force: false, network: net.clone(), dials: Dials::testnet(), account: 1 }).unwrap();
        let id = import(&w, &net, &json!({"text": text})).unwrap()["id"].as_str().unwrap().to_string();
        let g = full.cancel2.unwrap();
        let call = |m: &'static str, id: String, s: [u8; 32]| {
            let core = &core;
            async move { core.call_with_secret(m, &json!({"id": id}), if m == "watch.claim" { Slot::Heir } else { Slot::Guardian }, Zeroizing::new(s.to_vec())).await }
        };
        assert_eq!(call("watch.stop", id.clone(), g).await, (200, refusal(NOT_GUARDIAN)), "no link.json: no role");
        write_link(&w, &p.heir_x, &"cd".repeat(32));
        assert_eq!(call("watch.decline", id.clone(), g).await, (200, refusal(NOT_GUARDIAN)), "a link that holds no stop slot");
        assert_eq!(call("watch.claim", id.clone(), full.heir.unwrap()).await, (200, refusal(NOT_HEIR)));
        write_link(&w, &p.cancel2_x, &p.heir_x);
        assert_eq!(call("watch.stop", id.clone(), full.cancel3.unwrap()).await, (200, refusal(WRONG_GUARDIAN)));
        assert_eq!(call("watch.claim", id.clone(), g).await, (200, refusal(crate::link::WRONG_HEIR)));
        assert_eq!(call("watch.stop", "../x".into(), g).await, (200, refusal(BAD_ID)));
        assert_eq!(call("watch.stop", "0123456789ab".into(), g).await, (200, refusal(NOT_WATCHED)));
        assert_eq!(core.call("watch.stop", &json!({"id": id})).await, (200, refusal(NEEDS_KEY)));
        assert!(!core.ctx.keys.has_secret());
        // armed: slot 2 holds the guardian secret, and the cancel it signs passes the engine
        let (keys, which) = arm_guardian(&w, &id, &g).unwrap();
        assert_eq!((which, keys.cancel2, keys.cancel3), (2, Some(g), None));
        let ctx = Armed(watch_ctx(&core, &w.join(WATCH_DIR).join(&id), keys));
        let v = ctx.0.try_load_vault().unwrap().unwrap();
        let coin: Coin = (TransactionOutpoint::new(Hash::from_bytes([5u8; 32]), 0), UtxoEntry::new(3 * KAS, ctx.0.vault_spk(&v.state), 1_000, false, Some(v.covenant_id)));
        let pay = || vec![TransactionOutput::new(3 * KAS - 10_000, ctx.0.cold_spk())];
        let key = ctx.0.keys.try_stop_key(1).unwrap();
        let tx = ctx.0.vault_tx(&v, &coin, "cancel", Some(&key), &[V::Int(1)], pay(), 0, 0).unwrap();
        assert_eq!(tx.outputs[0].script_public_key, ctx.0.cold_spk(), "everything to the owner's safe");
        assert!(ctx.0.vault_tx(&v, &coin, "cancel", Some(&key), &[V::Int(2)], pay(), 0, 0).is_err(), "the guardian key does not sign for slot 3");
        drop(ctx);
        for d in [&a, &w] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    // Hostile watch codes and ids: guard.export text from another phone, ids from the page. Each is
    // refused with the engine's plain error, never a panic, and nothing lands outside the wallet.

    const MISMATCH: &str = concat!(
        "The covenant in this file does not match the code written in it. ",
        "Export it again from the account."
    );

    /// Text cut to its first 60 characters for an assert message (never sliced by bytes).
    fn head(s: &str) -> String {
        s.chars().take(60).collect()
    }

    /// A wallet folder inside a base folder of its own: a write outside the wallet shows there.
    fn nest(tag: &str) -> (PathBuf, PathBuf) {
        let base = tmp(tag);
        let wallet = base.join("wallet");
        std::fs::create_dir_all(&wallet).unwrap();
        (base, wallet)
    }

    /// The names in a folder, sorted; none when it does not exist.
    fn names(dir: &Path) -> Vec<String> {
        let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
        let mut v: Vec<String> =
            entries.map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn clean(dirs: &[&PathBuf]) {
        for d in dirs {
            std::fs::remove_dir_all(d).ok();
        }
    }

    fn set(code: &Value, k: &str, v: Value) -> String {
        let mut c = code.clone();
        c[k] = v;
        c.to_string()
    }

    fn unset(code: &Value, k: &str) -> String {
        let mut c = code.clone();
        c.as_object_mut().unwrap().remove(k);
        c.to_string()
    }

    fn err_of<T>(r: Result<T, String>) -> String {
        r.err().unwrap_or_else(|| "accepted".into())
    }

    /// A call's refusal ("accepted" when it answered), or None when it panicked.
    fn caught<T>(f: impl FnOnce() -> Result<T, String>) -> Option<String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| err_of(f()))).ok()
    }

    /// watch.code gives back a code another phone imports as the same account (public data only,
    /// the name kept); an id this phone does not watch, or no id, is BAD_ID.
    #[test]
    fn watch_code_round_trips_an_imported_code() {
        let net = Network::testnet();
        let (a, _, text) = owner("code-a");
        let (w, w2) = (tmp("code-w"), tmp("code-w2"));
        let id = imported(&w, &text);
        let gp = w.join(WATCH_DIR).join(&id).join(GUARD_FILE);
        std::fs::remove_file(&gp).ok();
        let out = code(&w, &net, &id);
        assert!(!gp.exists(), "watch.code writes nothing");
        assert_eq!((&out["ok"], &out["error"]), (&json!(true), &Value::Null), "{out}");
        let again = out["watch_code"].as_str().unwrap();
        assert!(!again.contains("\"spend\"") && !again.contains("cold2\""), "no secret field");
        let made = import(&w2, &net, &json!({"text": again})).unwrap();
        let got = (made["id"].as_str(), made["name"].as_str());
        assert_eq!(got, (Some(id.as_str()), Some("Owner One")));
        let original: Value = serde_json::from_str(&text).unwrap();
        let back: Value = serde_json::from_str(again).unwrap();
        assert_eq!(back["public_sha256"], original["public_sha256"]);
        for bad in ["0123456789ab", "../x", "", "ABCDEF012345"] {
            let r = code(&w, &net, bad);
            assert_eq!((&r["ok"], r["error"].as_str()), (&json!(false), Some(BAD_ID)), "{bad}");
        }
        for d in [&a, &w, &w2] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    /// The account's watch id after a plain import of its watch code.
    fn imported(w: &Path, text: &str) -> String {
        let r = import(w, &Network::testnet(), &json!({"text": text})).unwrap();
        r["id"].as_str().unwrap().to_string()
    }

    /// watch.import of `text`, a panic turned into the refusal "PANIC".
    fn try_import(w: &Path, net: &Network, text: &str) -> Result<Value, String> {
        let run = std::panic::AssertUnwindSafe(|| import(w, net, &json!({"text": text})));
        std::panic::catch_unwind(run).unwrap_or_else(|_| Err("PANIC".into()))
    }

    /// Each text is refused with its plain error, and the watch folder holds nothing new.
    fn import_refused(w: &Path, net: &Network, cases: &[(String, String)]) {
        let before = names(&w.join(WATCH_DIR));
        for (text, want) in cases {
            let got = try_import(w, net, text);
            assert_eq!(got.err().as_deref(), Some(want.as_str()), "{}", head(text));
        }
        assert_eq!(names(&w.join(WATCH_DIR)), before, "a refused import leaves nothing behind");
    }

    #[test]
    fn import_refuses_text_that_is_not_a_watch_code() {
        let net = Network::testnet();
        let (a, _, text) = owner("nw-a");
        let (base, w) = nest("nw-w");
        let code: Value = serde_json::from_str(&text).unwrap();
        let not = NOT_GUARD_FILE.to_string();
        let junk = ["", " \n ", "not json", "null", "[]", "{}", "\"guard-public\""];
        let mut cases: Vec<(String, String)> =
            junk.iter().map(|t| (t.to_string(), not.clone())).collect();
        let walks = [
            json!(5),
            json!(""),
            json!("../../../../x"),
            json!("..\\..\\..\\..\\x"),
            json!("0123456789a\u{e9}00"),
        ];
        cases.extend(walks.into_iter().map(|s| (set(&code, "public_sha256", s), not.clone())));
        cases.extend([
            ("x".repeat(1 << 20), not.clone()),
            ("[".repeat(100_000), not.clone()),
            (unset(&code, "public_sha256"), not.clone()),
            (format!("{text}{text}"), not.clone()),
        ]);
        import_refused(&w, &net, &cases);
        let bad_args = [
            json!({}),
            json!({"text": 5}),
            json!({"text": null}),
            json!("text"),
            json!(null),
        ];
        for args in &bad_args {
            assert_eq!(import(&w, &net, args).unwrap_err(), NOT_GUARD_FILE, "{args}");
        }
        assert_eq!(names(&base), ["wallet"], "nothing written outside the wallet");
        assert!(names(&w.join(WATCH_DIR)).is_empty(), "no watch folder made");
        clean(&[&a, &base]);
    }

    #[test]
    fn import_refuses_tampered_watch_codes() {
        let net = Network::testnet();
        let (a, _, text) = owner("tw-a");
        let (base, w) = nest("tw-w");
        let code: Value = serde_json::from_str(&text).unwrap();
        let sha = code["public_sha256"].as_str().unwrap().to_string();
        let flipped = format!("{}{}", &sha[..63], if sha.ends_with('0') { '1' } else { '0' });
        let walk = format!("{}/../../x", &sha[..12]);
        let mut swapped = code.clone();
        swapped["public"]["heir_x"] = json!("ab".repeat(32));
        let (not, bad) = (NOT_GUARD_FILE.to_string(), MISMATCH.to_string());
        let other_net = net.check_recorded("mainnet", GUARD_FILE).unwrap_err();
        let cases = [
            (set(&code, "kind", json!("kv-link")), not.clone()),
            (unset(&code, "kind"), not.clone()),
            (set(&code, "network", json!("mainnet")), other_net),
            (set(&code, "public_sha256", json!(flipped)), bad.clone()),
            (set(&code, "public_sha256", json!(sha.to_uppercase())), bad.clone()),
            (set(&code, "public_sha256", json!(walk)), bad.clone()),
            (swapped.to_string(), bad.clone()),
            (set(&code, "public", json!("x")), not.clone()),
            (unset(&code, "vault"), not.clone()),
            (set(&code, "vault", json!("locked")), not.clone()),
            (set(&code, "vault", json!({})), not.clone()),
            (unset(&code, "contract_sha256"), not.clone()),
        ];
        import_refused(&w, &net, &cases);
        let main = Network::mainnet();
        let to_main = main.check_recorded("testnet", GUARD_FILE).unwrap_err();
        assert_eq!(import(&w, &main, &json!({"text": text})).unwrap_err(), to_main);
        for rules in ["", "\u{e9}", "../../../etc/passwd", "00"] {
            let got = try_import(&w, &net, &set(&code, "contract_sha256", json!(rules)));
            assert!(got.is_err_and(|e| e != "PANIC"), "unknown rules {rules:?} refused");
        }
        assert_eq!(names(&base), ["wallet"], "nothing written outside the wallet");
        assert!(names(&w.join(WATCH_DIR)).is_empty(), "no watch folder left");
        clean(&[&a, &base]);
    }

    #[test]
    fn import_with_a_multibyte_contract_sha_refuses_without_a_panic() {
        let net = Network::testnet();
        let (a, _, text) = owner("mb-a");
        let (base, w) = nest("mb-w");
        let code: Value = serde_json::from_str(&text).unwrap();
        let got = try_import(&w, &net, &set(&code, "contract_sha256", json!("1234567\u{e9}")));
        clean(&[&a, &base]);
        assert!(got.is_err_and(|e| e != "PANIC"), "a refusal, never a crash");
    }

    /// The watch code with one public field replaced and its code recomputed: a hostile owner.
    fn resealed(code: &Value, k: &str, v: Value) -> String {
        let mut c = code.clone();
        c["public"][k] = v;
        let p: Public = serde_json::from_value(c["public"].clone()).unwrap();
        c["public_sha256"] = json!(p.sha256());
        c.to_string()
    }

    /// The import outcome of each resealed field: the refusal, "accepted" or "PANIC".
    fn resealed_outcomes(tag: &str, fields: &[(&str, Value)]) -> Vec<String> {
        let net = Network::testnet();
        let (a, _, text) = owner(&format!("{tag}-a"));
        let (base, w) = nest(&format!("{tag}-w"));
        let code: Value = serde_json::from_str(&text).unwrap();
        let mut got = vec![];
        for (k, v) in fields {
            got.push(err_of(try_import(&w, &net, &resealed(&code, k, v.clone()))));
        }
        clean(&[&a, &base]);
        got
    }

    #[test]
    fn import_of_a_resealed_file_with_malformed_keys_never_panics() {
        let fields = [("spend_x", json!("zz".repeat(32))), ("heir_x", json!("ab"))];
        let got = resealed_outcomes("rm", &fields);
        assert!(!got.iter().any(|e| e == "PANIC"), "{got:?}");
    }

    #[test]
    fn import_of_a_resealed_file_with_invalid_keys_is_refused() {
        let fields = [("cancel_x", json!("ff".repeat(32))), ("cold_x", json!(["00", "11"]))];
        let got = resealed_outcomes("ri", &fields);
        assert!(!got.iter().any(|e| e == "PANIC" || e == "accepted"), "{got:?}");
    }

    #[test]
    fn import_with_huge_dials_never_panics() {
        let net = Network::testnet();
        let (a, _, text) = owner("hd-a");
        let (base, w) = nest("hd-w");
        let mut huge: Value = serde_json::from_str(&text).unwrap();
        let dials = huge["vault"]["dials"].as_object_mut().expect("the record's dials");
        for v in dials.values_mut().filter(|v| v.is_u64()) {
            *v = json!(u64::MAX);
        }
        let dialed = err_of(try_import(&w, &net, &huge.to_string()));
        clean(&[&a, &base]);
        assert_ne!(dialed, "PANIC", "huge dials: a refusal or a watch, never a crash");
    }

    #[test]
    fn import_name_with_path_characters_is_only_a_label() {
        let net = Network::testnet();
        let (a, _, text) = owner("pn-a");
        let (base, w) = nest("pn-w");
        let code: Value = serde_json::from_str(&text).unwrap();
        let paths = ["../../evil", "..\\..\\evil", "/etc/passwd", "C:\\Windows\\evil", "watch/x"];
        for name in paths {
            let t = set(&code, "owner_name", json!(name));
            let r = import(&w, &net, &json!({"text": t})).unwrap();
            let id = r["id"].as_str().unwrap().to_string();
            assert_eq!(r["name"], json!(name));
            assert_eq!(names(&w.join(WATCH_DIR)), [id], "{name}: only the account's own folder");
            assert_eq!(list(&w)["watched"][0]["name"], json!(name));
        }
        assert_eq!(names(&base), ["wallet"], "nothing written outside the wallet");
        assert_eq!(names(&w), [WATCH_DIR], "nothing beside the watch folder");
        clean(&[&a, &base]);
    }

    /// A character a shown name may hold: no control, no bidi override or isolate.
    fn shown_plain(c: char) -> bool {
        let overrides = ('\u{202a}'..='\u{202e}').contains(&c);
        let isolates = ('\u{2066}'..='\u{2069}').contains(&c);
        !c.is_control() && !overrides && !isolates
    }

    #[test]
    fn import_refuses_or_cleans_hostile_owner_names() {
        let net = Network::testnet();
        let (a, _, text) = owner("hn-a");
        let (base, w) = nest("hn-w");
        let code: Value = serde_json::from_str(&text).unwrap();
        let huge = "x".repeat(1 << 20);
        let hostile = ["Mom\u{0}", "Mom\nDad", "\u{1b}[2J", "Mom\u{202e}", huge.as_str()];
        let mut kept = vec![];
        for name in hostile {
            let t = set(&code, "owner_name", json!(name));
            let Ok(r) = import(&w, &net, &json!({"text": t})) else { continue };
            let n = r["name"].as_str().unwrap_or("");
            if n.chars().count() > crate::link::LINK_NAME_MAX || !n.chars().all(shown_plain) {
                kept.push(head(name));
            }
        }
        clean(&[&a, &base]);
        assert!(kept.is_empty(), "hostile names kept as given: {kept:?}");
    }

    #[test]
    fn import_refusal_never_echoes_a_hostile_network() {
        let (a, _, text) = owner("ec-a");
        let (base, w) = nest("ec-w");
        let mut code: Value = serde_json::from_str(&text).unwrap();
        code["network"] = json!(format!("<img src=x onerror=alert(1)>{}", "y".repeat(1 << 20)));
        let got = err_of(import(&w, &Network::testnet(), &json!({"text": code.to_string()})));
        clean(&[&a, &base]);
        assert!(got.len() < 1000 && !got.contains('<'), "{}", head(&got));
    }

    /// Ids that are not 12 lower-case hex: walks out of the watch folder, other separators, misses.
    fn hostile_ids() -> Vec<String> {
        let ids = [
            "", ".", "..", "../x", "../../victim", "..\\..\\victim", "C:\\x\\0123456",
            "0123456789ab/", "/0123456789a", "0123456789ab\0", "0123456789a\0",
            "0123456789AB", "0123456789abc", "0123456789a", "0123456789\u{e9}",
            "%2e%2e%2f%2e%2e%2f",
        ];
        let mut v: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        v.push("\u{ff10}\u{ff11}\u{ff12}\u{ff13}\u{ff14}\u{ff15}".repeat(2));
        v.push("a".repeat(1 << 20));
        v
    }

    #[test]
    fn hostile_ids_never_leave_the_watch_folder() {
        let (a, _, text) = owner("id-a");
        let (base, w) = nest("id-w");
        std::fs::create_dir_all(base.join("victim")).unwrap();
        std::fs::write(base.join("victim").join("keep"), "x").unwrap();
        let id = imported(&w, &text);
        for bad in hostile_ids() {
            let label = head(&bad);
            assert_eq!(err_of(super::folder(&w, &bad)), BAD_ID, "{label}");
            assert_eq!(err_of(forget(&w, &bad)), BAD_ID, "{label}");
            assert_eq!(err_of(super::arm_guardian(&w, &bad, &[7u8; 32])), BAD_ID, "{label}");
            assert_eq!(err_of(super::arm_heir(&w, &bad, &[7u8; 32])), BAD_ID, "{label}");
        }
        assert!(base.join("victim").join("keep").exists(), "the folder beside the wallet stays");
        assert_eq!(names(&base), ["victim", "wallet"]);
        assert_eq!(names(&w.join(WATCH_DIR)), [id]);
        clean(&[&a, &base]);
    }

    #[tokio::test]
    async fn hostile_ids_through_the_watch_calls() {
        let net = Network::testnet();
        let (base, w) = nest("call-w");
        let mut keys = Keys::fresh(&net);
        keys.wipe();
        let ctx = crate::tx::Ctx {
            dir: w.clone(),
            keys,
            rpc: None,
            force: false,
            network: net,
            dials: Dials::testnet(),
            account: 1,
        };
        let core = crate::core::Core::open(ctx).unwrap();
        let mut ids: Vec<Value> = hostile_ids().into_iter().map(Value::from).collect();
        ids.extend([json!(5), json!(["../x"]), Value::Null]);
        let bad = (200, crate::core::refusal(BAD_ID));
        for id in &ids {
            let args = json!({"id": id});
            let (code, st) = super::call(&core, "watch.status", &args).await;
            assert_eq!((code, &st["ok"], &st["error"]), (200, &json!(false), &json!(BAD_ID)));
            assert_eq!(super::call(&core, "watch.forget", &args).await, bad);
            for m in ["watch.stop", "watch.decline", "watch.claim"] {
                let secret = zeroize::Zeroizing::new([7u8; 32]);
                assert_eq!(super::call_with_secret(&core, m, &args, secret).await, bad, "{m}");
            }
        }
        let needs = (200, crate::core::refusal(NEEDS_KEY));
        assert_eq!(super::call(&core, "watch.claim", &json!({})).await, needs);
        assert_eq!(names(&base), ["wallet"]);
        clean(&[&base]);
    }

    #[test]
    fn stop_and_claim_refuse_without_the_role_or_the_key() {
        let (a, full, text) = owner("arm-a");
        let (base, w) = nest("arm-w");
        let p = full.public();
        let id = imported(&w, &text);
        let lj = w.join(crate::link::LINK_FILE);
        let other = [7u8; 32];
        let refuse = |at: &str, g: &str, h: &str, why: &str| {
            assert_eq!(err_of(super::arm_guardian(&w, at, &other)), g, "{why}");
            assert_eq!(err_of(super::arm_heir(&w, at, &other)), h, "{why}");
        };
        let unwatched = if id == "0123456789ab" { "ba9876543210" } else { "0123456789ab" };
        refuse(unwatched, NOT_WATCHED, NOT_WATCHED, "not watched");
        refuse(&id, NOT_GUARDIAN, NOT_HEIR, "no link.json");
        std::fs::write(&lj, "not json").unwrap();
        refuse(&id, NOT_GUARDIAN, NOT_HEIR, "a garbage link.json");
        write_link(&w, &p.cancel_x, "");
        refuse(&id, NOT_GUARDIAN, NOT_HEIR, "stop key 1 is the owner's own");
        write_link(&w, &p.cancel2_x, &p.heir_x);
        let saved = std::fs::read_to_string(&lj).unwrap();
        let mut as_code: Value = serde_json::from_str(&saved).unwrap();
        as_code["kind"] = json!(crate::link::LINK_KIND);
        std::fs::write(&lj, as_code.to_string()).unwrap();
        refuse(&id, NOT_GUARDIAN, NOT_HEIR, "a link code saved as link.json");
        write_link(&w, &p.cancel2_x, &p.heir_x);
        refuse(&id, WRONG_GUARDIAN, crate::link::WRONG_HEIR, "the role, not the key");
        clean(&[&a, &base]);
    }

    #[test]
    fn stop_and_claim_with_an_invalid_secret_refuse_without_a_panic() {
        let (a, full, text) = owner("inv-a");
        let (base, w) = nest("inv-w");
        let p = full.public();
        let id = imported(&w, &text);
        write_link(&w, &p.cancel2_x, &p.heir_x);
        let mut got = vec![];
        for secret in [[0u8; 32], [0xffu8; 32]] {
            let g = caught(|| super::arm_guardian(&w, &id, &secret));
            let h = caught(|| super::arm_heir(&w, &id, &secret));
            got.push((g, h));
        }
        clean(&[&a, &base]);
        let want = (Some(WRONG_GUARDIAN.to_string()), Some(crate::link::WRONG_HEIR.to_string()));
        assert_eq!(got, [want.clone(), want], "a refusal, never a panic");
    }

    #[test]
    fn list_survives_garbage_in_the_watch_folder() {
        let (a, _, text) = owner("gl-a");
        let (base, w) = nest("gl-w");
        let id = imported(&w, &text);
        let wd = w.join(WATCH_DIR);
        for junk in ["zzzzzzzzzzzz", "0123456789ab.new", "abcdefabcdee"] {
            std::fs::create_dir_all(wd.join(junk)).unwrap();
        }
        std::fs::write(wd.join("abcdefabcdee").join("keys.json"), "not json").unwrap();
        std::fs::write(wd.join("abcdefabcdef"), "a file, not a folder").unwrap();
        let texts = ["", "not json", "[1,2]", "null", "\"str\"", "{\"name\":5}", "{\"name\":{}}"];
        let mut garbage: Vec<Vec<u8>> = texts.iter().map(|t| t.as_bytes().to_vec()).collect();
        garbage.push(vec![0xff, 0xfe, 0]);
        garbage.push("[".repeat(100_000).into_bytes());
        let f = wd.join(&id);
        for bytes in &garbage {
            std::fs::write(f.join(WATCHED_FILE), bytes).unwrap();
            let l = list(&w);
            let label = String::from_utf8_lossy(&bytes[..bytes.len().min(20)]).into_owned();
            assert_eq!(l["watched"].as_array().map(Vec::len), Some(1), "{label}");
            let first = &l["watched"][0];
            assert_eq!((&first["id"], &first["name"]), (&json!(id), &json!("")), "{label}");
        }
        assert_eq!(super::read_watched(&f), json!({}), "an unreadable record reads as none");
        clean(&[&a, &base]);
    }
}
