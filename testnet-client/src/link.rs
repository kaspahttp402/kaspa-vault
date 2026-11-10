//! The helper's link (docs/LINK_DESIGN.md): one guardian key and one heir key born on the helper's phone, reused
//! for every owner that phone helps. Only the public half crosses: `<dir>/link.json` holds the two x-only keys and
//! the heir's pay-to-pubkey address; the link code (the QR the owner scans) is that block with kind `kv-link`.
//! The two secrets go out once from `make_link` for the phone to seal and are never written here.

use crate::keys::{Keys, Public, sheet_words, try_xonly_hex};
use crate::network::Network;
use secp256k1::{Secp256k1, SecretKey, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use zeroize::{Zeroize, Zeroizing};

pub const LINK_FILE: &str = "link.json";
pub const LINK_PUBLIC_KIND: &str = "kv-link-public";
pub const LINK_KIND: &str = "kv-link";
pub const LINK_VERSION: u32 = 1;
pub const LINK_NAME_MAX: usize = 40;

pub const LINK_EXISTS: &str = "This phone already made its link code.";
pub const NO_LINK: &str = "This phone has no link code yet.";
pub const LINK_NAME: &str = "Give your link code a name of 1 to 40 characters.";
pub const LINK_UNREADABLE: &str = "This phone's link code could not be read.";
pub const LINK_UNWRITABLE: &str = "The account's folder on this phone could not be written.";
pub const NOT_A_LINK: &str = "That is not a link code.";
pub const LINK_NEWER: &str = "That link code was made by a newer app. Update this app first.";
pub const LINK_DAMAGED: &str = "That link code is damaged. Ask for the link code again.";
pub const LINK_ADDRESS: &str = "That link code is damaged: its heir address does not match. Ask for the link code again.";
pub const LINK_OWN_KEY: &str = "That link code belongs to this account itself. A guardian or heir sends the link code from their own phone.";
pub const WRONG_HEIR: &str = "This phone is not set up as that account's heir.";
pub const LINK_THIS_PHONE: &str = "That link code is this phone's own. A guardian or heir shows \
their link code on their own phone, so that they can step in without this phone.";

/// `link.json`, and (kind `kv-link`) the link code itself. Public only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LinkPublic {
    pub kind: String,
    pub v: u32,
    pub network: String,
    pub name: String,
    /// The guardian key: the stop key an owner puts in a stop slot.
    pub stop_x: String,
    pub heir_x: String,
    /// Pay-to-pubkey address of `heir_x` on `network`.
    pub heir_address: String,
    /// Set by `read_link` against the owner's rule block: whether this helper is already in the rules. Never stored.
    #[serde(skip)]
    pub already: Already,
}

/// Whether a read link is already in the owner's rules: the stop slot holding its guardian key (2 or 3), and
/// whether its heir key is the heir.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Already {
    pub guardian: Option<u8>,
    pub heir: bool,
}

impl LinkPublic {
    /// The answer of `link.code` / `link.read` without the code: `{ok, name, stop_x, heir_x, heir_address,
    /// already: {guardian: 2|3|null, heir}}`.
    pub fn answer(&self) -> Value {
        json!({"ok": true, "name": self.name, "stop_x": self.stop_x, "heir_x": self.heir_x, "heir_address": self.heir_address,
            "already": {"guardian": self.already.guardian, "heir": self.already.heir}})
    }
}

/// A valid x-only key in 64 hex characters, lower-cased.
fn x_only(s: &str) -> Option<String> {
    let mut v = [0u8; 32];
    (s.len() == 64 && faster_hex::hex_decode(s.as_bytes(), &mut v).is_ok() && XOnlyPublicKey::from_slice(&v).is_ok()).then(|| s.to_ascii_lowercase())
}

/// The key `secret` makes; empty (no key's text) when it is no secp256k1 secret.
fn x_of(secret: &[u8; 32]) -> String {
    try_xonly_hex(secret).unwrap_or_default()
}

/// A shown name: 1 to LINK_NAME_MAX characters, no control and no bidi mark, override or isolate.
pub fn name_ok(name: &str) -> bool {
    let bidi = |c: char| {
        let marks = matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}');
        marks || ('\u{202a}'..='\u{202e}').contains(&c) || ('\u{2066}'..='\u{2069}').contains(&c)
    };
    let plain = !name.chars().any(|c| c.is_control() || bidi(c));
    (1..=LINK_NAME_MAX).contains(&name.chars().count()) && plain
}

/// This phone's `link.json`, or the refusal (none yet; unreadable).
pub fn load(dir: &Path) -> Result<LinkPublic, String> {
    let text = match std::fs::read_to_string(dir.join(LINK_FILE)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(NO_LINK.into()),
        Err(_) => return Err(LINK_UNREADABLE.into()),
    };
    let link: LinkPublic = serde_json::from_str(&text).map_err(|_| LINK_UNREADABLE.to_string())?;
    if link.kind != LINK_PUBLIC_KIND {
        return Err(LINK_UNREADABLE.into());
    }
    Ok(link)
}

/// `linkMake {name}`: two fresh secrets, `link.json` written (refused when it exists). Answers the secret block
/// `[guardian 32][heir 32]` followed by UTF-8 JSON `{"heir_words":[24 words]}` (the heir secret's sheet), and the
/// public block. The words and secrets are wiped here once copied into the block.
pub fn make_link(dir: &Path, net: &Network, name: &str) -> Result<(Zeroizing<Vec<u8>>, LinkPublic), String> {
    let name = name.trim();
    if !name_ok(name) {
        return Err(LINK_NAME.into());
    }
    if dir.join(LINK_FILE).exists() {
        return Err(LINK_EXISTS.into());
    }
    let secp = Secp256k1::new();
    let guardian = Zeroizing::new(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
    let heir = Zeroizing::new(secp.generate_keypair(&mut secp256k1::rand::thread_rng()).0.secret_bytes());
    let (heir_x, heir_address) = Keys::heir_of(net, &x_of(&heir))?;
    let link = LinkPublic { kind: LINK_PUBLIC_KIND.into(), v: LINK_VERSION, network: net.name.into(), name: name.into(), stop_x: x_of(&guardian), heir_x, heir_address, already: Already::default() };
    let text = serde_json::to_string_pretty(&link).map_err(|_| LINK_UNWRITABLE.to_string())?;
    // create_new: two makes racing cannot both win.
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(LINK_FILE)).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { LINK_EXISTS.to_string() } else { LINK_UNWRITABLE.to_string() })?;
    if f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).is_err() {
        drop(f);
        std::fs::remove_file(dir.join(LINK_FILE)).ok();
        return Err(LINK_UNWRITABLE.into());
    }
    let mut words = sheet_words(&heir);
    let mut tail = Zeroizing::new(json!({"heir_words": words}).to_string());
    let mut block = Zeroizing::new(Vec::with_capacity(64 + tail.len()));
    block.extend_from_slice(&guardian[..]);
    block.extend_from_slice(&heir[..]);
    block.extend_from_slice(tail.as_bytes());
    tail.zeroize();
    for w in words.iter_mut() {
        w.zeroize();
    }
    Ok((block, link))
}

/// `link.code {}`: `{ok, code, name, stop_x, heir_x, heir_address}`; `code` is the compact `kv-link` JSON for the QR.
pub fn link_code(dir: &Path, net: &Network) -> Result<Value, String> {
    let link = load(dir)?;
    if link.network != net.name {
        return Err(format!("This phone's link code was made for the {} network; this account is on the {} network.", link.network, net.name));
    }
    let code = serde_json::to_string(&LinkPublic { kind: LINK_KIND.into(), ..link.clone() }).map_err(|_| LINK_UNREADABLE.to_string())?;
    let mut answer = link.answer();
    answer["code"] = json!(code);
    Ok(answer)
}

/// `link.read {text}` on the owner's phone: a scanned or pasted link code, checked (kind, network, both keys valid
/// and distinct, the heir address is the pay-to-pubkey of heir_x on this network, neither key one of this account's
/// own: the wallet's spend key, stop key 1 and the safe keys). `own` is this wallet's rule block when it has one.
/// Answers the code with its keys lower-cased and `already` saying whether this helper is in the rules now (a
/// guardian in stop slot 2 or 3, the heir): reading a helper's code again is not a refusal.
pub fn read_link(net: &Network, text: &str, own: Option<&Public>) -> Result<LinkPublic, String> {
    let link: LinkPublic = serde_json::from_str(text.trim()).map_err(|_| NOT_A_LINK.to_string())?;
    if link.kind != LINK_KIND {
        return Err(NOT_A_LINK.into());
    }
    if link.v > LINK_VERSION {
        return Err(LINK_NEWER.into());
    }
    if link.v != LINK_VERSION || !name_ok(link.name.trim()) {
        return Err(NOT_A_LINK.into());
    }
    if link.network != net.name {
        let (theirs, ours) = (crate::network::quoted(&link.network), net.name);
        return Err(format!(
            "That link code is for the {theirs} network; this account is on the {ours} network."
        ));
    }
    let (Some(stop_x), Some(heir_x)) = (x_only(&link.stop_x), x_only(&link.heir_x)) else {
        return Err(LINK_DAMAGED.into());
    };
    if stop_x == heir_x {
        return Err(LINK_DAMAGED.into());
    }
    let (heir_x, heir_address) = Keys::heir_of(net, &heir_x).map_err(|_| LINK_DAMAGED.to_string())?;
    if heir_address != link.heir_address {
        return Err(LINK_ADDRESS.into());
    }
    let mut already = Already::default();
    if let Some(p) = own {
        let mine: Vec<String> = [&p.spend_x, &p.cancel_x].into_iter().chain(p.cold_x.iter()).filter(|k| !k.is_empty()).map(|k| k.to_ascii_lowercase()).collect();
        if mine.contains(&stop_x) || mine.contains(&heir_x) {
            return Err(LINK_OWN_KEY.into());
        }
        already.guardian = [(2u8, &p.cancel2_x), (3u8, &p.cancel3_x)].into_iter().find(|(_, k)| k.eq_ignore_ascii_case(&stop_x)).map(|(n, _)| n);
        already.heir = p.heir_x.eq_ignore_ascii_case(&heir_x);
    }
    Ok(LinkPublic { kind: LINK_KIND.into(), v: LINK_VERSION, network: link.network, name: link.name.trim().into(), stop_x, heir_x, heir_address, already })
}

/// A link code this very phone made (`<dir>/link.json`) is refused: an owner who scans their own
/// code makes themselves guardian and heir, and nobody else can step in.
pub fn not_this_phone(dir: &Path, link: &LinkPublic) -> Result<(), String> {
    let Ok(mine) = load(dir) else { return Ok(()) };
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if same(&mine.stop_x, &link.stop_x) || same(&mine.heir_x, &link.heir_x) {
        return Err(LINK_THIS_PHONE.into());
    }
    Ok(())
}

/// `link.heir_words {}` with the sealed heir secret: `{ok, heir_words: [24 words]}`, only when the secret is the one
/// `link.json` names. Honest limit: the words go out in the answer, which the caller shows and drops.
pub fn heir_words(dir: &Path, secret: &[u8; 32]) -> Result<Value, String> {
    let link = load(dir)?;
    if SecretKey::from_slice(secret).is_err() || x_of(secret) != link.heir_x {
        return Err(WRONG_HEIR.into());
    }
    let mut words = sheet_words(secret);
    let answer = json!({"ok": true, "heir_words": words});
    for w in words.iter_mut() {
        w.zeroize();
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kv-link-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn split(block: &[u8]) -> ([u8; 32], [u8; 32], Value) {
        let g: [u8; 32] = block[..32].try_into().unwrap();
        let h: [u8; 32] = block[32..64].try_into().unwrap();
        (g, h, serde_json::from_slice(&block[64..]).unwrap())
    }

    #[test]
    fn own_link_code_is_refused_on_the_phone_that_made_it() {
        let (dir, other) = (tmp("self"), tmp("self-other"));
        let net = Network::testnet();
        make_link(&dir, &net, "Ada").unwrap();
        make_link(&other, &net, "Ben").unwrap();
        let own = link_code(&dir, &net).unwrap()["code"].as_str().unwrap().to_string();
        let theirs = link_code(&other, &net).unwrap()["code"].as_str().unwrap().to_string();
        let own = read_link(&net, &own, None).unwrap();
        let theirs = read_link(&net, &theirs, None).unwrap();
        assert_eq!(not_this_phone(&dir, &own).unwrap_err(), LINK_THIS_PHONE);
        assert_eq!(not_this_phone(&dir, &theirs), Ok(()));
        assert_eq!(not_this_phone(&tmp("self-none"), &own), Ok(()), "no link of its own: any code");
    }

    #[test]
    fn make_then_code_round_trip_and_second_make_refused() {
        let dir = tmp("make");
        let net = Network::testnet();
        assert_eq!(make_link(&dir, &net, "  ").unwrap_err(), LINK_NAME);
        assert_eq!(make_link(&dir, &net, &"x".repeat(41)).unwrap_err(), LINK_NAME);
        assert_eq!(link_code(&dir, &net).unwrap_err(), NO_LINK);
        let (block, link) = make_link(&dir, &net, " Helper One ").unwrap();
        let (g, h, tail) = split(&block);
        assert_eq!((x_of(&g), x_of(&h)), (link.stop_x.clone(), link.heir_x.clone()), "the secrets are the keys link.json names");
        let words: Vec<String> = serde_json::from_value(tail["heir_words"].clone()).unwrap();
        assert_eq!(words, sheet_words(&h).to_vec());
        assert_eq!(load(&dir).unwrap(), link);
        assert_eq!((link.name.as_str(), link.network.as_str(), link.kind.as_str()), ("Helper One", "testnet", LINK_PUBLIC_KIND));
        let file = std::fs::read_to_string(dir.join(LINK_FILE)).unwrap();
        assert!(!file.contains(&faster_hex::hex_string(&h)) && !file.contains(&faster_hex::hex_string(&g)), "no secret in link.json");

        let code = link_code(&dir, &net).unwrap();
        assert_eq!((code["ok"].clone(), code["stop_x"].clone(), code["heir_address"].clone()), (json!(true), json!(link.stop_x), json!(link.heir_address)));
        let read = read_link(&net, code["code"].as_str().unwrap(), None).unwrap();
        assert_eq!(read, LinkPublic { kind: LINK_KIND.into(), ..link.clone() });
        assert_eq!(link_code(&dir, &Network::mainnet()).unwrap_err(), "This phone's link code was made for the testnet network; this account is on the mainnet network.");

        assert_eq!(make_link(&dir, &net, "Again").unwrap_err(), LINK_EXISTS);
        assert_eq!(load(&dir).unwrap(), link, "the refused make left link.json alone");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_link_refusals() {
        let dir = tmp("read");
        let net = Network::testnet();
        make_link(&dir, &net, "Helper").unwrap();
        let code: Value = serde_json::from_str(link_code(&dir, &net).unwrap()["code"].as_str().unwrap()).unwrap();
        let with = |k: &str, v: Value| {
            let mut c = code.clone();
            c[k] = v;
            c.to_string()
        };
        assert_eq!(read_link(&Network::mainnet(), &code.to_string(), None).unwrap_err(), "That link code is for the testnet network; this account is on the mainnet network.");
        assert_eq!(read_link(&net, "hello", None).unwrap_err(), NOT_A_LINK);
        assert_eq!(read_link(&net, &with("kind", json!("kv-link-public")), None).unwrap_err(), NOT_A_LINK);
        assert_eq!(read_link(&net, &with("v", json!(2)), None).unwrap_err(), LINK_NEWER);
        assert_eq!(read_link(&net, &with("stop_x", json!("ff".repeat(32))), None).unwrap_err(), LINK_DAMAGED, "not a point on the curve");
        assert_eq!(read_link(&net, &with("heir_x", json!("abc")), None).unwrap_err(), LINK_DAMAGED);
        assert_eq!(read_link(&net, &with("heir_x", code["stop_x"].clone()), None).unwrap_err(), LINK_DAMAGED);
        let other = Keys::fresh(&net);
        let p = other.public.clone().unwrap();
        assert_eq!(read_link(&net, &with("heir_address", json!(p.heir_address)), None).unwrap_err(), LINK_ADDRESS);
        assert_eq!(read_link(&net, &with("stop_x", json!(p.cancel_x.to_ascii_uppercase())), Some(&p)).unwrap_err(), LINK_OWN_KEY, "this wallet's own stop key");
        let fresh = read_link(&net, &code.to_string(), Some(&p)).unwrap();
        assert_eq!(fresh.answer()["already"], json!({"guardian": null, "heir": false}), "someone else's link reads, not in the rules");
        // The same helper already in the rules (guardian in stop slot 3, the heir) reads with `already`, not as an own key.
        let mut q = p.clone();
        q.cancel3_x = fresh.stop_x.to_ascii_uppercase();
        q.heir_x = fresh.heir_x.clone();
        assert_eq!(read_link(&net, &code.to_string(), Some(&q)).unwrap().answer()["already"], json!({"guardian": 3, "heir": true}));
        q.cancel3_x = String::new();
        q.cancel2_x = fresh.stop_x.clone();
        assert_eq!(read_link(&net, &code.to_string(), Some(&q)).unwrap().already, Already { guardian: Some(2), heir: true });
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn heir_words_only_for_the_heir_secret() {
        let dir = tmp("words");
        let net = Network::testnet();
        assert_eq!(heir_words(&dir, &[7u8; 32]).unwrap_err(), NO_LINK);
        let (block, _) = make_link(&dir, &net, "Helper").unwrap();
        let (g, h, _) = split(&block);
        assert_eq!(heir_words(&dir, &g).unwrap_err(), WRONG_HEIR, "the guardian key is not the heir key");
        assert_eq!(heir_words(&dir, &[0u8; 32]).unwrap_err(), WRONG_HEIR);
        let words: Vec<String> = serde_json::from_value(heir_words(&dir, &h).unwrap()["heir_words"].clone()).unwrap();
        assert_eq!(words, sheet_words(&h).to_vec());
        std::fs::remove_dir_all(&dir).ok();
    }

    // Hostile link codes: text another phone hands this one. Each is refused with the engine's
    // plain error, never a panic, and a read writes nothing.

    /// Text cut to its first 60 characters for an assert message (never sliced by bytes).
    fn head(s: &str) -> String {
        s.chars().take(60).collect()
    }

    /// A fresh helper's link code on `net`, as JSON; its folder is gone again (reading needs none).
    fn code_on(tag: &str, net: &Network) -> Value {
        let dir = tmp(tag);
        make_link(&dir, net, "Helper").unwrap();
        let code = link_code(&dir, net).unwrap()["code"].as_str().unwrap().to_string();
        std::fs::remove_dir_all(&dir).ok();
        serde_json::from_str(&code).unwrap()
    }

    fn set(code: &Value, k: &str, v: Value) -> String {
        let mut c = code.clone();
        c[k] = v;
        c.to_string()
    }

    /// Each text is refused with its plain error, never read as a code.
    fn refused(net: &Network, own: Option<&Public>, cases: &[(String, &str)]) {
        for (text, want) in cases {
            let got = read_link(net, text, own);
            assert_eq!(got.as_ref().err().map(String::as_str), Some(*want), "{}", head(text));
        }
    }

    #[test]
    fn read_link_refuses_text_that_is_not_a_code() {
        let net = Network::testnet();
        let dir = tmp("shape");
        make_link(&dir, &net, "Helper").unwrap();
        let text = link_code(&dir, &net).unwrap()["code"].as_str().unwrap().to_string();
        let open = &text[..text.len() - 1];
        let other = "ab".repeat(32);
        let deep = "[".repeat(100_000);
        let junk = ["", " \n\t\r ", "not json", "null", "[]", "{}", "42", "\"kv-link\""];
        let mut cases: Vec<(String, &str)> =
            junk.iter().map(|t| (t.to_string(), NOT_A_LINK)).collect();
        cases.extend([
            ("x".repeat(1 << 20), NOT_A_LINK),
            (" ".repeat(1 << 20), NOT_A_LINK),
            (deep.clone(), NOT_A_LINK),
            (format!("{{\"kind\":{deep}"), NOT_A_LINK),
            (format!("{open},\"x\":{deep}"), NOT_A_LINK),
            (format!("{{\"stop_x\":\"{other}\",{}", &text[1..]), NOT_A_LINK),
            (format!("{text}{text}"), NOT_A_LINK),
            (format!("{text} trailing"), NOT_A_LINK),
        ]);
        refused(&net, None, &cases);
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, [LINK_FILE], "a refused read writes nothing");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_link_refuses_wrong_types_kinds_and_versions() {
        let net = Network::testnet();
        let code = code_on("types", &net);
        let mut gone = code.clone();
        gone.as_object_mut().unwrap().remove("heir_address");
        let cases = [
            (set(&code, "v", json!("1")), NOT_A_LINK),
            (set(&code, "v", json!(-1)), NOT_A_LINK),
            (set(&code, "v", json!(0)), NOT_A_LINK),
            (set(&code, "v", json!(1.5)), NOT_A_LINK),
            (set(&code, "v", json!(u64::from(u32::MAX) + 1)), NOT_A_LINK),
            (set(&code, "v", json!(u32::MAX)), LINK_NEWER),
            (set(&code, "stop_x", json!(5)), NOT_A_LINK),
            (set(&code, "name", Value::Null), NOT_A_LINK),
            (set(&code, "heir_x", json!(["ab"])), NOT_A_LINK),
            (set(&code, "kind", json!("KV-LINK")), NOT_A_LINK),
            (set(&code, "kind", json!("kv-link ")), NOT_A_LINK),
            (set(&code, "kind", json!(LINK_PUBLIC_KIND)), NOT_A_LINK),
            (gone.to_string(), NOT_A_LINK),
        ];
        refused(&net, None, &cases);
    }

    #[test]
    fn read_link_names_must_be_short_and_plain() {
        let net = Network::testnet();
        let code = code_on("names", &net);
        let long = [
            "x".repeat(41),
            "\u{e9}".repeat(41),
            "\u{1f475}".repeat(41),
            "x".repeat(1 << 20),
        ];
        let bad = ["", "   ", "Mom\u{0}", "Mom\nDad", "\u{1b}[31mMom", "a\u{7f}", "a\u{9b}b"];
        let cases: Vec<(String, &str)> = bad
            .iter()
            .map(|n| n.to_string())
            .chain(long)
            .map(|n| (set(&code, "name", json!(n)), NOT_A_LINK))
            .collect();
        refused(&net, None, &cases);
        let forty = "\u{e9}".repeat(40);
        for name in ["Zo\u{eb} \u{1f475}", " Mom ", "\u{540d}\u{524d}", forty.as_str()] {
            let read = read_link(&net, &set(&code, "name", json!(name)), None).unwrap();
            assert_eq!(read.name, name.trim(), "a plain name of 1 to 40 characters reads");
        }
    }

    #[test]
    fn read_link_refuses_bidi_controls_in_the_name() {
        let net = Network::testnet();
        let code = code_on("bidi", &net);
        let cases: Vec<(String, &str)> = ["Mom\u{202e}", "\u{2066}Mom", "Mom\u{202d}x"]
            .iter()
            .map(|n| (set(&code, "name", json!(n)), NOT_A_LINK))
            .collect();
        refused(&net, None, &cases);
    }

    #[test]
    fn read_link_refuses_keys_that_are_not_keys() {
        let net = Network::testnet();
        let code = code_on("keys", &net);
        let good = code["stop_x"].as_str().unwrap().to_string();
        let heir = code["heir_x"].as_str().unwrap().to_string();
        let bad = [
            String::new(),
            "00".repeat(32),
            format!("{}fffffffefffffc2f", "f".repeat(48)),
            format!("02{good}"),
            format!("0x{}", &good[2..]),
            good[..62].to_string(),
            "zz".repeat(32),
            "\u{e9}".repeat(32),
            format!("{good}\n"),
            format!(" {}", &good[1..]),
            format!("{}\u{0}", &good[..63]),
        ];
        let mut cases: Vec<(String, &str)> = vec![];
        for k in &bad {
            cases.push((set(&code, "stop_x", json!(k)), LINK_DAMAGED));
            cases.push((set(&code, "heir_x", json!(k)), LINK_DAMAGED));
        }
        cases.push((set(&code, "stop_x", json!(heir.to_uppercase())), LINK_DAMAGED));
        let addr = code["heir_address"].as_str().unwrap();
        let (_, of_stop) = Keys::heir_of(&net, &good).unwrap();
        for a in [String::new(), addr.to_uppercase(), format!("{addr} "), of_stop] {
            cases.push((set(&code, "heir_address", json!(a)), LINK_ADDRESS));
        }
        refused(&net, None, &cases);
    }

    #[test]
    fn read_link_refuses_the_other_network() {
        let (test, main) = (Network::testnet(), Network::mainnet());
        let (t_code, m_code) = (code_on("net-t", &test), code_on("net-m", &main));
        let to_test = concat!(
            "That link code is for the mainnet network; ",
            "this account is on the testnet network."
        );
        assert_eq!(read_link(&test, &m_code.to_string(), None).unwrap_err(), to_test);
        let to_main = concat!(
            "That link code is for the testnet network; ",
            "this account is on the mainnet network."
        );
        assert_eq!(read_link(&main, &t_code.to_string(), None).unwrap_err(), to_main);
        // Relabeled to the reader's network: the heir address still names the other one.
        let relabeled = set(&t_code, "network", json!("mainnet"));
        assert_eq!(read_link(&main, &relabeled, None).unwrap_err(), LINK_ADDRESS);
        let relabeled = set(&m_code, "network", json!("testnet"));
        assert_eq!(read_link(&test, &relabeled, None).unwrap_err(), LINK_ADDRESS);
        for n in ["Testnet", "testnet ", " testnet", "testnet\u{0}", "", "testnet-10"] {
            let err = read_link(&test, &set(&t_code, "network", json!(n)), None).unwrap_err();
            assert!(err.ends_with("this account is on the testnet network."), "{n:?}");
        }
    }

    #[test]
    fn read_link_refuses_this_accounts_own_keys() {
        let net = Network::testnet();
        let code = code_on("own", &net);
        let p = Keys::fresh(&net).public();
        let mut own = vec![p.spend_x.clone(), p.cancel_x.to_uppercase()];
        own.extend(p.cold_x.iter().cloned());
        for k in &own {
            let got = read_link(&net, &set(&code, "stop_x", json!(k)), Some(&p)).unwrap_err();
            assert_eq!(got, LINK_OWN_KEY, "stop {k}");
            let (_, address) = Keys::heir_of(&net, &k.to_ascii_lowercase()).unwrap();
            let mut c = code.clone();
            c["heir_x"] = json!(k);
            c["heir_address"] = json!(address);
            let got = read_link(&net, &c.to_string(), Some(&p)).unwrap_err();
            assert_eq!(got, LINK_OWN_KEY, "heir {k}");
        }
    }

    #[test]
    fn read_link_drops_unknown_fields_and_never_takes_already() {
        let net = Network::testnet();
        let code = code_on("extra", &net);
        let plain = read_link(&net, &code.to_string(), None).unwrap();
        let mut c = code.clone();
        c["already"] = json!({"guardian": 2, "heir": true});
        c["evil"] = json!("<script>alert(1)</script>");
        c["__proto__"] = json!({"ok": false});
        c["stop_x"] = json!(plain.stop_x.to_uppercase());
        let read = read_link(&net, &c.to_string(), None).unwrap();
        assert_eq!(read, plain, "unknown fields dropped, the key lower-cased");
        assert_eq!(read.answer()["already"], json!({"guardian": null, "heir": false}));
        assert!(!read.answer().to_string().contains("script"));
        // A closed, deeply nested unknown field: skipped without recursion, never a stack overflow.
        let text = code.to_string();
        let open = &text[..text.len() - 1];
        let deep = format!("{open},\"x\":{}{}}}", "[".repeat(100_000), "]".repeat(100_000));
        match read_link(&net, &deep, None) {
            Ok(l) => assert_eq!(l, plain),
            Err(e) => assert_eq!(e, NOT_A_LINK),
        }
    }

    #[test]
    fn read_link_refusal_never_echoes_a_hostile_network() {
        let net = Network::testnet();
        let code = code_on("echo", &net);
        let hostile = format!("<img src=x onerror=alert(1)>{}", "y".repeat(1 << 20));
        let got = read_link(&net, &set(&code, "network", json!(hostile)), None).unwrap_err();
        assert!(got.len() < 1000 && !got.contains('<'), "{}", head(&got));
    }
}
