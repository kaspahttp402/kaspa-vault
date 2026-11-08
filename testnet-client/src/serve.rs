//! `serve`: a small HTTPS/1.1 service so a phone app on the home wifi can drive the
//! vault (on the network the folder was made for). Money actions run one at a time (the emergency stop never waits);
//! status is cached for a moment. It holds the spend key and the first stop key only.
//! It never loads a safe sheet: the offline keys.json is read through a struct that
//! refuses to start when a cold field is present at all.
//!
//! TLS with a self-signed certificate made on first start and kept next
//! to keys.json; the phone pins the SHA-256 of the DER certificate after the user confirms
//! the check words (BIP-39 words from that hash: three for a glance, eight for the real
//! comparison, and the whole fingerprint in hex; the console prints all three forms and the
//! /pair answer carries them). Only loopback and
//! private peers are answered, nothing else is even handshaken. A phone pairs once with an
//! 8-digit code shown on the console (POST /pair) within ten minutes; MAX_WRONG_CODES_PER_ADDRESS
//! wrong codes shut that one address out for WRONG_CODE_PENALTY, and nothing a stranger types
//! closes pairing for the owner. The owner re-opens pairing at any time by typing REOPEN_WORD
//! and Enter at the console (a new code and a new card). The token (the phone's card)
//! lasts TOKEN_LIFE; then pairing re-opens with a fresh printed code and the old token gets
//! the re-pair words. The online keys.json may not hold the heir's secret (start is refused,
//! like a cold sheet) and the funding secret is dropped at start and never read back: no route
//! needs it (the small-vault lift under GET /api/quote?kind=stop answers with what to do instead). At most 16 connections at once, four per address, one second for the
//! handshake and the request head; bodies over 64 KB
//! and heads over 8 KB drop the connection; a Host header that is not this service is a
//! 400; nothing after startup logs the token or the key.

use crate::core::{Core, SAFE_SHEETS_HERE, guard, no_heir_key, no_safe_sheet, query_pairs, refusal};
use crate::keys::*;
use crate::tx::*;
use serde::Deserialize;
use std::path::PathBuf;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_HEAD: usize = 8 * 1024;
const MAX_BODY: usize = 64 * 1024;
/// Whole-request budget: handshake, head and body together.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Handshake and request head together: an idle or trickling peer frees its slot this fast.
const HEAD_TIMEOUT: Duration = Duration::from_secs(1);
/// Connections served at once; the rest are dropped without a byte read.
const MAX_CONNECTIONS: usize = 16;
/// Connections one address may hold at once, so one neighbour cannot fill the house.
const MAX_PER_ADDRESS: usize = 4;
/// Wrong pairing codes from one address before that address must wait WRONG_CODE_PENALTY.
/// There is no shared cap: no set of strangers can close pairing on the owner.
const MAX_WRONG_CODES_PER_ADDRESS: u32 = 5;
/// How long an address at the cap is refused before its strikes start over (TOO_MANY_TRIES says it).
const WRONG_CODE_PENALTY: Duration = Duration::from_secs(10 * 60);
/// What the owner types at the console (then Enter) to open pairing again with a fresh code.
const REOPEN_WORD: &str = "pair";
/// How long the phone's token (its card) is good; then pairing re-opens with a fresh code.
const TOKEN_LIFE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long after start the pairing code is accepted.
const PAIRING_WINDOW: Duration = Duration::from_secs(10 * 60);
const CERT_FILE: &str = "service-cert.der";
const KEY_FILE: &str = "service-key.der";
/// The BIP-39 English list (keys.rs holds the one copy; the phone's safe sheets use it too).
const WORDS_TEXT: &str = crate::keys::BIP39_WORDS_TEXT;

const PAIRING_CLOSED: &str = "Pairing is closed. On the laptop, type pair and press Enter to open it again.";
const WRONG_CODE: &str = "That code is not right.";
/// Matches WRONG_CODE_PENALTY (a test keeps them in step).
const TOO_MANY_TRIES: &str = "Too many wrong codes from this phone. Wait 10 minutes and try again.";
pub(crate) const CARD_EXPIRED: &str = "This card has expired. Pair the phone again from the code your laptop shows.";

type Tls = StreamOwned<ServerConnection, TcpStream>;

/// The transport around `Core`: pairing, the certificate, the host rule and the refusal counter.
struct Srv {
    core: Arc<Core>,
    /// The token lives with the pairing state, so expiry and re-pair change together.
    pairing: Mutex<Pairing>,
    /// What the pairing banner prints and the /pair answer carries: address, port and the check forms.
    banner: Banner,
    tls: Arc<ServerConfig>,
    /// Host header values that name this service; anything else is a DNS-rebinding attempt.
    hosts: Vec<String>,
    /// (last time a refusal line was printed, refusals since then).
    refusals: Mutex<(Instant, u32)>,
}

// Keys: what serve may hold

/// The only things serve reads from the offline folder. The cold fields are decoded only far
/// enough to know they are there, and their presence refuses the start.
#[derive(Deserialize)]
struct OfflineSheet {
    cancel: Option<[u8; 32]>,
    cold: Option<Value>,
    cold2: Option<Value>,
    cold3: Option<Value>,
}

/// The parser's own error is never shown: it quotes the value it choked on, which for a mistyped
/// secret byte is part of the secret.
pub fn offline_stop_key(text: &str, path: &Path) -> Result<[u8; 32], String> {
    let sheet: OfflineSheet = serde_json::from_str(text)
        .map_err(|_| format!("{} is not an offline keys.json this program can read. Restore the original file.", path.display()))?;
    if sheet.cold.is_some() || sheet.cold2.is_some() || sheet.cold3.is_some() {
        return Err(SAFE_SHEETS_HERE.into());
    }
    sheet.cancel.ok_or_else(|| "The offline keys.json has no stop secret.".into())
}

/// Startup refusal in bank words, no panic trace.
fn refuse_start(why: &str) -> ! {
    println!("{why}");
    std::process::exit(2)
}

// Identity: certificate, check words

fn word_list() -> Vec<&'static str> {
    let words: Vec<&str> = WORDS_TEXT.lines().collect();
    assert_eq!(words.len(), 2048, "bip39-english.txt");
    words
}

/// The BIP-39 word for the 11 bits of `hash` that start at bit `from` (big-endian bit order).
fn word_at(words: &[&'static str], hash: &[u8], from: usize) -> &'static str {
    words[(0..11).fold(0usize, |acc, i| (acc << 1) | ((hash[(from + i) / 8] >> (7 - (from + i) % 8)) & 1) as usize)]
}

/// The short form: three words from the first 33 bits. A glance, not a proof: a graphics card
/// grinds a certificate with the same three words in minutes, so the phone compares the long form.
fn words_of(hash: &[u8]) -> [&'static str; 3] {
    let words = word_list();
    [word_at(&words, hash, 0), word_at(&words, hash, 11), word_at(&words, hash, 22)]
}

/// The long form: eight words from the first 88 bits; the first three are the short form.
fn long_words_of(hash: &[u8]) -> [&'static str; 8] {
    let words = word_list();
    std::array::from_fn(|i| word_at(&words, hash, 11 * i))
}

/// The whole fingerprint: the certificate's SHA-256 in hex, in groups of four to read aloud.
fn fingerprint_of(hash: &[u8; 32]) -> String {
    let hex = faster_hex::hex_string(hash);
    hex.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(" ")
}

fn cert_sha256(cert_der: &[u8]) -> [u8; 32] {
    Sha256::digest(cert_der).into()
}

/// The certificate and key next to keys.json, made on first start. The phone pins the
/// certificate hash, not the name, so a changed LAN address does not need a new one.
fn load_or_make_identity(dir: &Path, ip: &str) -> (Arc<ServerConfig>, Vec<u8>) {
    let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
    let (cert, key) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(cert), Ok(key)) => (cert, key),
        _ => {
            let made = rcgen::generate_simple_self_signed(vec![ip.to_string()]).unwrap_or_else(|e| refuse_start(&format!("Could not make the service certificate ({e}). Try again.")));
            let (cert, key) = (made.cert.der().to_vec(), made.key_pair.serialize_der());
            crate::keys::write_private(&key_path, &key).unwrap_or_else(|e| refuse_start(&format!("Could not write {}: {e}. Check the folder can be written.", key_path.display())));
            std::fs::write(&cert_path, &cert).unwrap_or_else(|e| refuse_start(&format!("Could not write {}: {e}. Check the folder can be written.", cert_path.display())));
            println!("made a new service certificate: {}", cert_path.display());
            (cert, key)
        }
    };
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert.clone())], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
        .unwrap_or_else(|e| refuse_start(&format!("The service certificate next to keys.json cannot be used ({e}). Delete {} and {} and start again; the phone will need to pair again.", CERT_FILE, KEY_FILE)));
    (Arc::new(config), cert)
}

fn lan_ip() -> String {
    // Connecting a UDP socket sends nothing; it only picks the outgoing interface.
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("8.8.8.8:80").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "this-machine".into())
}

/// Loopback, the RFC 1918 private ranges, fc00::/7 and ::1 (IPv4-mapped v6 is unwrapped first).
fn is_home_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.octets()[0] & 0xfe) == 0xfc,
    }
}

/// The Host values a real client sends to this service (the port is part of the name).
fn allowed_hosts(ip: &str, port: u16) -> Vec<String> {
    let mut hosts = vec![format!("{ip}:{port}"), format!("localhost:{port}"), format!("127.0.0.1:{port}"), format!("[::1]:{port}")];
    if port == 443 {
        hosts.extend([ip.to_string(), "localhost".into(), "127.0.0.1".into(), "[::1]".into()]);
    }
    hosts
}

/// The start-up allow-list plus the address the laptop has right now, so a new wifi lease does not
/// turn every request into a 400 until restart (the phone finds the laptop by address, not by name).
fn hosts_with(frozen: &[String], ip_now: &str, port: u16) -> Vec<String> {
    let mut hosts = frozen.to_vec();
    hosts.extend(allowed_hosts(ip_now, port).into_iter().filter(|h| !frozen.contains(h)));
    hosts
}

fn host_allowed(host: Option<&str>, allowed: &[String]) -> bool {
    host.is_some_and(|h| allowed.iter().any(|a| a.eq_ignore_ascii_case(h.trim())))
}

fn random_token() -> String {
    use secp256k1::rand::RngCore;
    let mut bytes = [0u8; 32];
    secp256k1::rand::thread_rng().fill_bytes(&mut bytes);
    faster_hex::hex_string(&bytes)
}

fn random_code() -> String {
    use secp256k1::rand::Rng;
    format!("{:08}", secp256k1::rand::thread_rng().gen_range(0..100_000_000u32))
}

pub async fn run(mut ctx: Ctx, port: u16, offline_dir: PathBuf) {
    if let Err(e) = no_safe_sheet(&ctx.keys).and_then(|()| no_heir_key(&ctx.keys)) {
        refuse_start(&e);
    }
    if ctx.keys.public.is_none() {
        refuse_start("keys.json has no public part. Run `keys --import-public` first.");
    }
    if ctx.keys.spend.is_none() {
        refuse_start("The spend key is not in keys.json. Run this service on the online device that made the vault.");
    }
    let offline_path = offline_dir.join("keys.json");
    let offline_text = std::fs::read_to_string(&offline_path).unwrap_or_else(|e| refuse_start(&format!("Could not read {}: {e}. Point --offline-dir at the folder holding the offline keys.json.", offline_path.display())));
    let stop = offline_stop_key(&offline_text, &offline_path).unwrap_or_else(|e| refuse_start(&e));
    let stop_x = faster_hex::hex_string(&xonly(&keypair(&stop)));
    if stop_x != ctx.keys.public().cancel_x {
        refuse_start("The offline keys.json does not hold this vault's first stop key. Check --offline-dir.");
    }
    // Spend and the first stop key are all the service uses; nothing else stays in memory. The
    // funding secret is dropped here and never read back from disk: no route needs it (see lift_for).
    ctx.keys.cancel = Some(stop);
    ctx.keys.cancel2 = None;
    ctx.keys.cancel3 = None;
    ctx.keys.funding = None;
    let core = Core::open(ctx).unwrap_or_else(|e| refuse_start(&e));

    let listener = TcpListener::bind(("0.0.0.0", port)).await.unwrap_or_else(|e| refuse_start(&format!("Port {port} is taken or not allowed ({e}). Pick another with --port.")));
    let ip = lan_ip();
    let (tls, cert) = load_or_make_identity(&core.ctx.dir, &ip);
    let hash = cert_sha256(&cert);
    let (token, code) = (random_token(), random_code());
    println!("serving the {} vault on https://{ip}:{port} (also https://localhost:{port}); home-network phones only", core.ctx.network.name);
    let banner = Banner { ip: ip.clone(), port, words: words_of(&hash), long_words: long_words_of(&hash), fingerprint: fingerprint_of(&hash) };
    print_pairing(&banner, &code);
    let srv = Arc::new(Srv {
        core: Arc::new(core),
        pairing: Mutex::new(Pairing::new(code, token, Instant::now())),
        banner,
        tls,
        hosts: allowed_hosts(&ip, port),
        refusals: Mutex::new((Instant::now(), 0)),
    });
    // The owner re-opens pairing by typing REOPEN_WORD and Enter at the console. When the console
    // is not a terminal the reader ends quietly and a restart stays the other way.
    let console = srv.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if line.trim().eq_ignore_ascii_case(REOPEN_WORD) {
                console.reopen_pairing();
            }
        }
    });
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let slots = Arc::new(Mutex::new(HashMap::new()));
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => match (limit.clone().try_acquire_owned(), AddressSlot::take(&slots, peer.ip())) {
                (Ok(permit), Some(slot)) => {
                    tokio::spawn(connection(srv.clone(), stream, peer, permit, slot));
                }
                (Err(_), _) => {
                    drop(stream);
                    srv.note_refusal("too many connections");
                }
                (_, None) => {
                    drop(stream);
                    srv.note_refusal("too many connections from one address");
                }
            },
            Err(e) => {
                println!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

// Pairing and the card

/// What the pairing banner prints and the /pair answer carries: address, port, the short check
/// line (three words), the long one (eight words) and the whole fingerprint of the certificate.
struct Banner {
    ip: String,
    port: u16,
    words: [&'static str; 3],
    long_words: [&'static str; 8],
    fingerprint: String,
}

/// The /pair answer: the card, plus every check form so the phone compares the long form of
/// the certificate it pinned, not only the three words. `token` is the field the app has always read.
fn pair_answer(token: &str, b: &Banner) -> Value {
    json!({"ok": true, "token": token, "check_words": b.words, "check_words_long": b.long_words, "fingerprint": b.fingerprint})
}

/// The pairing banner, at start, when the owner re-opens pairing and whenever the card expires.
/// Code and words only: the app has no QR reader, so a QR would only repeat the code.
fn print_pairing(b: &Banner, code: &str) {
    let spaced: String = code.chars().enumerate().map(|(i, c)| if i == 4 { format!(" {c}") } else { c.to_string() }).collect();
    let minutes = WRONG_CODE_PENALTY.as_secs() / 60;
    println!();
    println!("  ==============================================");
    println!("  PAIRING CODE      {spaced}");
    println!("  CHECK WORDS       {}", b.words.join(" "));
    println!("  FULL CHECK        {}", b.long_words.join(" "));
    println!("  FINGERPRINT       {}", b.fingerprint);
    println!("  ==============================================");
    println!("  The phone shows the same words before it trusts https://{}:{}.", b.ip, b.port);
    println!("  The three words are a glance; the full check or the fingerprint is the sure comparison.");
    println!("  Type the code in the app within 10 minutes.");
    println!("  One phone may pair. {MAX_WRONG_CODES_PER_ADDRESS} wrong codes from one address shut that address out for {minutes} minutes;");
    println!("  nothing a stranger types closes pairing. To open pairing again at any time, type {REOPEN_WORD} and press Enter here.");
    println!("  The phone's card lasts 7 days; then this code is printed afresh and the phone asks to pair again.");
    println!();
}

struct Pairing {
    /// `None` once a phone has paired or the window passed.
    code: Option<String>,
    until: Instant,
    /// Wrong codes per address: strikes and the time of the last one. An address at the cap is
    /// refused until WRONG_CODE_PENALTY has passed since its last strike, then starts over.
    /// There is no shared count, so no set of strangers can close pairing on the owner.
    wrong_by: HashMap<IpAddr, (u32, Instant)>,
    /// The card: the token the paired phone sends, and when it stops being accepted.
    token: String,
    token_until: Instant,
    /// The token before the last re-issue, so an expired card gets the re-pair words, not "wrong token".
    old_token: Option<String>,
}

enum TokenCheck {
    Ok,
    Expired,
    Wrong,
}

impl Pairing {
    fn new(code: String, token: String, now: Instant) -> Self {
        Pairing { code: Some(code), until: now + PAIRING_WINDOW, wrong_by: HashMap::new(), token, token_until: now + TOKEN_LIFE, old_token: None }
    }

    /// One attempt from one address. The strike is counted here, under the lock, before any
    /// answer leaves. An address that guessed wrong MAX_WRONG_CODES_PER_ADDRESS times waits out
    /// WRONG_CODE_PENALTY; nothing it does touches any other address or closes pairing.
    fn try_code(&mut self, given: &str, from: IpAddr, now: Instant) -> Result<(), &'static str> {
        let Some(code) = &self.code else { return Err(PAIRING_CLOSED) };
        if now >= self.until {
            self.code = None;
            return Err(PAIRING_CLOSED);
        }
        let mine = self.wrong_by.entry(from).or_insert((0, now));
        if mine.0 >= MAX_WRONG_CODES_PER_ADDRESS {
            if now < mine.1 + WRONG_CODE_PENALTY {
                return Err(TOO_MANY_TRIES);
            }
            *mine = (0, now);
        }
        if !same_secret(given, code) {
            *mine = (mine.0 + 1, now);
            return Err(WRONG_CODE);
        }
        self.code = None;
        Ok(())
    }

    /// Pairing opens afresh: a new token (so only the phone that types the new code holds a card),
    /// a new code (returned for printing), strikes start over, and the old token is kept only to
    /// answer with the re-pair words. Run by the owner's REOPEN_WORD and by card expiry.
    fn reopen(&mut self, now: Instant) -> String {
        let code = random_code();
        self.old_token = Some(std::mem::replace(&mut self.token, random_token()));
        self.token_until = now + TOKEN_LIFE;
        self.code = Some(code.clone());
        self.until = now + PAIRING_WINDOW;
        self.wrong_by.clear();
        code
    }

    /// Past TOKEN_LIFE the card is re-issued; `Some(code)` is the new code for printing.
    fn expire(&mut self, now: Instant) -> Option<String> {
        if now < self.token_until { None } else { Some(self.reopen(now)) }
    }

    /// The card check on every /api request; `Some(code)` means this call re-issued it.
    fn check_token(&mut self, given: Option<&str>, now: Instant) -> (TokenCheck, Option<String>) {
        let new_code = self.expire(now);
        let check = if token_ok(given, &self.token) {
            TokenCheck::Ok
        } else if self.old_token.as_deref().is_some_and(|old| token_ok(given, old)) {
            TokenCheck::Expired
        } else {
            TokenCheck::Wrong
        };
        (check, new_code)
    }
}

/// One address's hold on a connection slot; dropping it frees the slot.
struct AddressSlot {
    slots: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}

impl AddressSlot {
    fn take(slots: &Arc<Mutex<HashMap<IpAddr, usize>>>, ip: IpAddr) -> Option<Self> {
        let mut map = guard(slots);
        let held = map.get(&ip).copied().unwrap_or(0);
        if held >= MAX_PER_ADDRESS {
            return None;
        }
        map.insert(ip, held + 1);
        Some(AddressSlot { slots: slots.clone(), ip })
    }
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        let mut map = guard(&self.slots);
        if let Some(held) = map.get_mut(&self.ip) {
            *held -= 1;
            if *held == 0 {
                map.remove(&self.ip);
            }
        }
    }
}

impl Srv {
    /// POST /pair: the 8-digit code buys the token once, within the pairing window.
    fn pair(&self, body: &[u8], from: IpAddr) -> Result<String, &'static str> {
        let given = serde_json::from_slice::<Value>(body).ok().and_then(|v| v["code"].as_str().map(str::to_string)).unwrap_or_default();
        let (answer, new_code) = {
            let mut p = guard(&self.pairing);
            let now = Instant::now();
            let new_code = p.expire(now);
            (p.try_code(&given, from, now).map(|()| p.token.clone()), new_code)
        };
        self.announce(new_code);
        answer
    }

    /// The card check for /api requests; an expiry re-opens pairing and prints the new code.
    fn check_token(&self, given: Option<&str>) -> TokenCheck {
        let (check, new_code) = guard(&self.pairing).check_token(given, Instant::now());
        self.announce(new_code);
        check
    }

    fn announce(&self, new_code: Option<String>) {
        if let Some(code) = new_code {
            println!("the phone's card has expired after 7 days; pairing is open again with a new code");
            print_pairing(&self.banner, &code);
        }
    }

    /// The owner typed REOPEN_WORD at the console: pairing opens again with a fresh code and card.
    fn reopen_pairing(&self) {
        let code = guard(&self.pairing).reopen(Instant::now());
        println!("pairing is open again for 10 minutes with a new code; a phone paired before must pair again");
        print_pairing(&self.banner, &code);
    }

    /// Refused connections are counted and reported at most once a minute (a flooder
    /// must not be able to fill the console).
    fn note_refusal(&self, what: &str) {
        let mut r = guard(&self.refusals);
        r.1 += 1;
        if r.1 == 1 || r.0.elapsed() >= Duration::from_secs(60) {
            println!("refused {} connection(s), latest: {what}", r.1);
            *r = (Instant::now(), 0);
        }
    }
}

// HTTP over TLS

struct Request {
    method: String,
    path: String,
    token: Option<String>,
    /// What followed `?` in the target; never logged, read only by /api/quote.
    query: String,
    body: Vec<u8>,
}

enum ReadError {
    /// Head or body over the cap: drop the connection without a word.
    TooLarge,
    TimedOut,
    /// Host header missing or naming something else (DNS rebinding): 400 before the body is read.
    BadHost,
    Bad(String),
}

impl From<&str> for ReadError {
    fn from(e: &str) -> Self {
        ReadError::Bad(e.into())
    }
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match e.kind() {
            TimedOut | WouldBlock => ReadError::TimedOut,
            InvalidData => ReadError::Bad("not TLS, or a bad handshake".into()),
            _ => ReadError::Bad(e.to_string()),
        }
    }
}

/// Reads through the TLS stream against one deadline for the whole request, so a peer
/// trickling one byte at a time gets no more than READ_TIMEOUT in total.
struct Timed<'a> {
    tls: &'a mut Tls,
    deadline: Instant,
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.tls.sock.set_read_timeout(Some(left))?;
        self.tls.read(buf)
    }
}

/// Only printable ASCII reaches the log or the router.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| c.is_ascii_graphic()).take(max).collect()
}

/// Head and body against one reader; the server reads them under two deadlines (see accept_request).
#[cfg(test)]
fn read_request(stream: &mut impl Read, allowed: &[String]) -> Result<Request, ReadError> {
    let (buf, head_end) = read_head(stream)?;
    read_rest(stream, buf, head_end, allowed)
}

/// The request head up to the blank line (the TLS handshake happens inside the first read).
fn read_head(stream: &mut impl Read) -> Result<(Vec<u8>, usize), ReadError> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err("connection closed".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    if head_end > MAX_HEAD {
        return Err(ReadError::TooLarge);
    }
    Ok((buf, head_end))
}

/// Parses the head and reads the body it announces.
fn read_rest(stream: &mut impl Read, buf: Vec<u8>, head_end: usize, allowed: &[String]) -> Result<Request, ReadError> {
    let mut chunk = [0u8; 1024];
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| "request head is not text")?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, target) = (first.next().ok_or("no method")?, first.next().ok_or("no path")?);
    let (mut token, mut host) = (None, None);
    let mut length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.eq_ignore_ascii_case("x-token") {
            token = Some(value.trim().to_string());
        } else if name.eq_ignore_ascii_case("host") {
            host = Some(value.trim().to_string());
        } else if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().map_err(|_| "bad content-length")?;
        }
    }
    if !host_allowed(host.as_deref(), allowed) {
        return Err(ReadError::BadHost);
    }
    if length > MAX_BODY {
        return Err(ReadError::TooLarge);
    }
    // The query string is kept apart from the path; only the path reaches the log.
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let request = Request { method: clean(method, 8), path: clean(path, 64), query: clean(query, 256), token, body: vec![] };
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err("connection closed".into());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Request { body, ..request })
}

/// Blocking side of a connection: handshake and read one request under the deadline.
fn accept_request(config: Arc<ServerConfig>, sock: TcpStream, allowed: &[String]) -> Result<(Tls, Result<Request, ReadError>), String> {
    sock.set_nonblocking(false).map_err(|e| e.to_string())?;
    sock.set_write_timeout(Some(READ_TIMEOUT)).map_err(|e| e.to_string())?;
    let conn = ServerConnection::new(config).map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, sock);
    // Handshake and head within HEAD_TIMEOUT, the body within READ_TIMEOUT, both from the same start.
    let start = Instant::now();
    let head = read_head(&mut Timed { tls: &mut tls, deadline: start + HEAD_TIMEOUT });
    let request = head.and_then(|(buf, head_end)| read_rest(&mut Timed { tls: &mut tls, deadline: start + READ_TIMEOUT }, buf, head_end, allowed));
    Ok((tls, request))
}

/// Constant-time compare: XOR fold over equal-length bytes; unequal lengths fail.
fn same_secret(given: &str, want: &str) -> bool {
    given.len() == want.len() && given.bytes().zip(want.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

fn token_ok(given: Option<&str>, want: &str) -> bool {
    given.is_some_and(|g| same_secret(g, want))
}

fn response_head(code: u16, body_len: usize) -> String {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n")
}

/// Blocking side of the answer: write, close_notify, shut the socket.
fn finish(mut tls: Tls, code: u16, body: &str) {
    let _ = tls.sock.set_write_timeout(Some(READ_TIMEOUT));
    let _ = tls.write_all(response_head(code, body.len()).as_bytes());
    let _ = tls.write_all(body.as_bytes());
    tls.conn.send_close_notify();
    let _ = tls.flush();
    let _ = tls.sock.shutdown(Shutdown::Both);
}

async fn answer(tls: Tls, code: u16, body: String) {
    let _ = tokio::task::spawn_blocking(move || finish(tls, code, &body)).await;
}

async fn connection(srv: Arc<Srv>, stream: tokio::net::TcpStream, peer: SocketAddr, _permit: OwnedSemaphorePermit, _slot: AddressSlot) {
    // A peer outside the home network gets no handshake, no byte: the connection just closes.
    if !is_home_network(peer.ip()) {
        srv.note_refusal("not on the home network");
        return;
    }
    let Ok(sock) = stream.into_std() else { return };
    let (config, hosts) = (srv.tls.clone(), hosts_with(&srv.hosts, &lan_ip(), srv.banner.port));
    let (tls, request) = match tokio::task::spawn_blocking(move || accept_request(config, sock, &hosts)).await {
        Ok(Ok(accepted)) => accepted,
        Ok(Err(e)) => {
            println!("{peer} dropped: {e}");
            return;
        }
        Err(e) => {
            println!("{peer} dropped: {e}");
            return;
        }
    };
    let request = match request {
        Ok(r) => r,
        Err(ReadError::TooLarge) => {
            println!("{peer} dropped: request too large");
            return;
        }
        Err(ReadError::TimedOut) => {
            println!("{peer} timed out");
            return;
        }
        Err(ReadError::BadHost) => {
            println!("{peer} refused: Host header is not this service");
            answer(tls, 400, refusal("Bad request").to_string()).await;
            return;
        }
        Err(ReadError::Bad(e)) => {
            println!("{peer} bad request: {e}");
            answer(tls, 400, refusal("Bad request").to_string()).await;
            return;
        }
    };
    let (method, path) = (request.method.clone(), request.path.clone());
    let (code, body) = if method == "POST" && path == "/pair" {
        match srv.pair(&request.body, peer.ip()) {
            Ok(token) => (200, pair_answer(&token, &srv.banner)),
            Err(WRONG_CODE) => (401, refusal(WRONG_CODE)),
            Err(TOO_MANY_TRIES) => (429, refusal(TOO_MANY_TRIES)),
            Err(e) => (403, refusal(e)),
        }
    } else if !path.starts_with("/api/") {
        (404, refusal("Not found"))
    } else {
        match srv.check_token(request.token.as_deref()) {
            // The card ran out: the phone forgets it and the laptop has printed a fresh code.
            TokenCheck::Expired => (401, refusal(CARD_EXPIRED)),
            // A 256-bit token cannot be guessed, so a wrong one is refused and nothing more:
            // no strike, no lockout that a spoofed address could turn against the phone.
            TokenCheck::Wrong => (401, refusal("Wrong or missing token")),
            // In its own task: should anything still panic, this request fails and the server lives.
            TokenCheck::Ok => match tokio::spawn(route(srv.clone(), request)).await {
                Ok(answer) => answer,
                Err(e) => {
                    println!("{peer} {method} {path}: the handler failed: {e}");
                    (500, json!({"ok": false, "tx": null, "error": "Something went wrong inside the service. Check status before trying again."}))
                }
            },
        }
    };
    answer(tls, code, body.to_string()).await;
    // The /pair answer holds the token; it is never part of the log line.
    let outcome = match (&body["ok"], &body["tx"], &body["error"]) {
        (Value::Bool(true), Value::String(tx), _) => format!(" {} tx {tx}", body["kind"].as_str().unwrap_or("ok")),
        (Value::Bool(false), _, Value::String(e)) => format!(" refused: {e}"),
        _ => String::new(),
    };
    println!("{peer} {method} {path} -> {code}{outcome}");
}

/// Each /api route is one `Core::call` method; the body (or the query) becomes its JSON object.
/// A body that is not a JSON object reads as an empty one, as `Value::Null` did before: the method
/// then refuses in its own words. Anything else under /api/ is not a method, and `call` says so.
async fn route(srv: Arc<Srv>, request: Request) -> (u16, Value) {
    let body = || serde_json::from_slice::<Value>(&request.body).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
    let (method, args) = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/status") => ("status", json!({})),
        ("GET", "/api/quote") => ("quote", json!(query_pairs(&request.query))),
        ("POST", "/api/send") => ("send", body()),
        ("POST", "/api/finish") => ("finish", json!({})),
        ("POST", "/api/stop") => ("stop", json!({})),
        ("POST", "/api/decline") => ("decline", body()),
        _ => ("", json!({})),
    };
    srv.core.call(method, &args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Dials;
    use crate::core::{HEIR_KEY_HERE, NETWORK_DOWN};
    use crate::network::Network;
    use kaspa_wrpc_client::prelude::*;

    #[test]
    fn home_network_rule() {
        let home = ["127.0.0.1", "10.1.2.3", "172.16.0.9", "172.31.255.1", "10.0.1.20", "::1", "fc00::1", "fdab::7", "::ffff:10.0.0.5"];
        let away = ["8.8.8.8", "172.32.0.1", "100.64.0.1", "169.254.1.1", "2001:db8::1", "fe80::1", "::ffff:8.8.8.8"];
        for ip in home {
            assert!(is_home_network(ip.parse().unwrap()), "{ip} should be home");
        }
        for ip in away {
            assert!(!is_home_network(ip.parse().unwrap()), "{ip} should be refused");
        }
    }

    #[test]
    fn secret_compare() {
        assert!(same_secret("12345678", "12345678"));
        assert!(!same_secret("12345679", "12345678"));
        assert!(!same_secret("1234567", "12345678"));
        assert!(!same_secret("", "12345678"));
        assert!(!token_ok(None, "abc"));
        assert_eq!(random_code().len(), 8);
        assert_eq!(random_token().len(), 64);
    }

    /// Fixed vectors: the word list is the canonical BIP-39 English list and the bit
    /// slicing is big-endian 11-bit groups from the first 33 bits.
    #[test]
    fn check_words_derivation() {
        assert_eq!(WORDS_TEXT.lines().count(), 2048);
        assert_eq!(words_of(&[0u8; 32]), ["abandon", "abandon", "abandon"]);
        assert_eq!(words_of(&[0xffu8; 32]), ["zoo", "zoo", "zoo"]);
        assert_eq!(words_of(&[0x00, 0x20, 0x00, 0x40, 0x00]), ["ability", "abandon", "avoid"]);
        // sha256("kaspa") = e0d9d0a56149f986...
        let hash = cert_sha256(b"kaspa");
        assert_eq!(faster_hex::hex_string(&hash[..8]), "e0d9d0a56149f986");
        assert_eq!(words_of(&hash), ["this", "soldier", "citizen"]);
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, last])
    }

    #[test]
    fn pairing_strikes_are_per_address_and_lift_after_the_penalty() {
        let t0 = Instant::now();
        let mut p = Pairing::new("12345678".into(), "tok".into(), t0);
        for i in 1..=MAX_WRONG_CODES_PER_ADDRESS {
            assert_eq!(p.try_code("00000000", ip(3), t0), Err(WRONG_CODE), "wrong code {i}");
        }
        assert!(p.code.is_some(), "one address alone cannot close pairing");
        assert_eq!(p.try_code("12345678", ip(3), t0), Err(TOO_MANY_TRIES), "that address waits, right code or not");
        assert_eq!(p.try_code("12345678", ip(3), t0 + WRONG_CODE_PENALTY - Duration::from_secs(1)), Err(TOO_MANY_TRIES));
        assert_eq!(p.try_code("12345679", ip(4), t0), Err(WRONG_CODE), "another address is not touched");

        // Twenty, forty wrong codes over many addresses: pairing stays open for the owner.
        for last in 10..40 {
            for _ in 0..MAX_WRONG_CODES_PER_ADDRESS {
                assert_eq!(p.try_code("00000000", ip(last), t0), Err(WRONG_CODE));
            }
        }
        assert!(p.code.is_some(), "no number of strangers closes pairing");
        assert_eq!(p.try_code("12345678", ip(4), t0), Ok(()), "the owner still pairs");
        assert_eq!(p.try_code("12345678", ip(4), t0), Err(PAIRING_CLOSED), "one phone only");

        // After the penalty the struck address starts over with a clean count.
        let mut p = Pairing::new("12345678".into(), "tok".into(), t0);
        for _ in 0..MAX_WRONG_CODES_PER_ADDRESS {
            assert_eq!(p.try_code("00000000", ip(3), t0), Err(WRONG_CODE));
        }
        // The penalty dial equals the window here, so the test holds the window open (reopen does the same in life).
        p.until = t0 + 2 * WRONG_CODE_PENALTY;
        let later = t0 + WRONG_CODE_PENALTY;
        assert_eq!(p.try_code("00000000", ip(3), later), Err(WRONG_CODE), "the penalty is over: a strike again, not a refusal");
        assert_eq!(p.wrong_by[&ip(3)].0, 1, "the count started over");
        assert_eq!(p.try_code("12345678", ip(3), later), Ok(()), "and the right code pairs");

        let mut p = Pairing::new("12345678".into(), "tok".into(), t0);
        assert_eq!(p.try_code("12345678", ip(3), t0 + PAIRING_WINDOW), Err(PAIRING_CLOSED), "window over");
        assert!(PAIRING_CLOSED.contains(REOPEN_WORD), "the refusal says how to open pairing again");
    }

    #[test]
    fn too_many_tries_words_match_the_penalty_dial() {
        let minutes = WRONG_CODE_PENALTY.as_secs() / 60;
        assert!(TOO_MANY_TRIES.contains(&format!("{minutes} minutes")), "{TOO_MANY_TRIES}");
    }

    #[test]
    fn reopen_prints_a_new_code_and_a_new_card() {
        let t0 = Instant::now();
        let mut p = Pairing::new("12345678".into(), "tok-1".into(), t0);
        assert_eq!(p.try_code("12345678", ip(3), t0), Ok(()));
        assert_eq!(p.try_code("12345678", ip(5), t0), Err(PAIRING_CLOSED), "closed after the first phone");
        for _ in 0..MAX_WRONG_CODES_PER_ADDRESS {
            let _ = p.try_code("00000000", ip(6), t0);
        }
        let t1 = t0 + Duration::from_secs(60);
        let code = p.reopen(t1);
        assert_eq!(code.len(), 8);
        assert_ne!(code, "12345678");
        assert!(p.code.is_some(), "pairing is open again");
        assert_ne!(p.token, "tok-1", "a new card, so the old phone cannot ride along");
        assert!(matches!(p.check_token(Some("tok-1"), t1), (TokenCheck::Expired, None)), "the old phone gets the re-pair words");
        assert_eq!(p.try_code("00000000", ip(6), t1), Err(WRONG_CODE), "strikes started over");
        assert_eq!(p.try_code(&code, ip(5), t1), Ok(()), "the new code pairs the next phone");
        let new = p.token.clone();
        assert!(matches!(p.check_token(Some(new.as_str()), t1), (TokenCheck::Ok, None)));
        assert_eq!(p.try_code(&code, ip(5), t1 + PAIRING_WINDOW), Err(PAIRING_CLOSED), "the new window closes too");
    }

    #[test]
    fn long_check_words_and_fingerprint_derivation() {
        assert_eq!(long_words_of(&[0u8; 32]), ["abandon"; 8]);
        assert_eq!(long_words_of(&[0xffu8; 32]), ["zoo"; 8]);
        let hash: [u8; 32] = Sha256::digest(b"certificate").into();
        let long = long_words_of(&hash);
        assert_eq!(&long[..3], &words_of(&hash), "the three words are the start of the eight");
        assert!(long.iter().collect::<std::collections::HashSet<_>>().len() > 1);
        // 88 bits: bits 77..88 land in bytes 9 and 10.
        let mut h = [0u8; 32];
        h[9] = 0x00;
        h[10] = 0x01 << 7; // bit 80 set -> word index 2^7 = 128 for the eighth word (bits 77..88)
        assert_eq!(long_words_of(&h)[7], word_list()[128]);
        let fp = fingerprint_of(&[0xabu8; 32]);
        assert_eq!(fp.len(), 64 + 15, "64 hex digits in 16 groups of four");
        assert!(fp.starts_with("abab abab "));
        assert_eq!(fp.split(' ').count(), 16);
    }

    #[test]
    fn pair_answer_carries_short_and_long_check_forms() {
        let hash: [u8; 32] = Sha256::digest(b"certificate").into();
        let b = Banner { ip: "10.0.1.2".into(), port: 8787, words: words_of(&hash), long_words: long_words_of(&hash), fingerprint: fingerprint_of(&hash) };
        let a = pair_answer("tok", &b);
        assert_eq!(a["ok"], true);
        assert_eq!(a["token"], "tok", "the field the app has always read");
        assert_eq!(a["check_words"].as_array().unwrap().len(), 3);
        assert_eq!(a["check_words_long"].as_array().unwrap().len(), 8);
        assert_eq!(a["check_words_long"][0], a["check_words"][0]);
        assert_eq!(a["fingerprint"], fingerprint_of(&hash));
        assert_eq!(a["fingerprint"].as_str().unwrap().replace(' ', ""), faster_hex::hex_string(&hash));
    }

    #[test]
    fn heir_key_in_keys_json_refuses_start() {
        assert_eq!(no_heir_key(&Keys { heir: Some([7; 32]), ..Default::default() }), Err(HEIR_KEY_HERE.to_string()));
        assert_eq!(no_heir_key(&Keys::default()), Ok(()));
        let mut k = Keys { heir: Some([7; 32]), ..Default::default() };
        k.cold = Some([1; 32]);
        assert_eq!(no_safe_sheet(&k).and_then(|()| no_heir_key(&k)), Err(SAFE_SHEETS_HERE.to_string()), "the safe sheets are named first");
        assert!(!HEIR_KEY_HERE.contains("heir secret"), "bank words");
    }

    #[test]
    fn expired_card_answers_401_words_and_reopens_pairing() {
        let t0 = Instant::now();
        let mut p = Pairing::new("12345678".into(), "tok-1".into(), t0);
        assert_eq!(p.try_code("12345678", ip(3), t0), Ok(()));
        assert!(matches!(p.check_token(Some("tok-1"), t0 + TOKEN_LIFE - Duration::from_secs(1)), (TokenCheck::Ok, None)));
        assert!(matches!(p.check_token(None, t0), (TokenCheck::Wrong, None)));
        assert!(matches!(p.check_token(Some("nonsense"), t0), (TokenCheck::Wrong, None)));
        let (check, code) = p.check_token(Some("tok-1"), t0 + TOKEN_LIFE);
        assert!(matches!(check, TokenCheck::Expired), "the old card gets the re-pair words");
        let code = code.expect("a fresh code is printed at expiry");
        assert_eq!(code.len(), 8);
        assert_ne!(p.token, "tok-1");
        assert!(matches!(p.check_token(Some("tok-1"), t0 + TOKEN_LIFE), (TokenCheck::Expired, None)), "printed once");
        assert!(matches!(p.check_token(Some("nonsense"), t0 + TOKEN_LIFE), (TokenCheck::Wrong, None)));
        let new = p.token.clone();
        assert_eq!(p.try_code("00000000", ip(3), t0 + TOKEN_LIFE), Err(WRONG_CODE), "counters started over");
        assert_eq!(p.try_code(&code, ip(3), t0 + TOKEN_LIFE), Ok(()), "the new code pairs again");
        assert!(matches!(p.check_token(Some(new.as_str()), t0 + TOKEN_LIFE), (TokenCheck::Ok, None)));
        assert_eq!(p.try_code(&code, ip(3), t0 + TOKEN_LIFE + PAIRING_WINDOW), Err(PAIRING_CLOSED), "the new window closes too");
        // The body the phone reads on that 401.
        assert_eq!(refusal(CARD_EXPIRED)["error"], "This card has expired. Pair the phone again from the code your laptop shows.");
    }

    #[test]
    fn address_slots_cap_at_four() {
        let slots = Arc::new(Mutex::new(HashMap::new()));
        let held: Vec<_> = (0..MAX_PER_ADDRESS).map(|_| AddressSlot::take(&slots, ip(3)).expect("under the cap")).collect();
        assert!(AddressSlot::take(&slots, ip(3)).is_none(), "the fifth connection from one address is refused");
        assert!(AddressSlot::take(&slots, ip(4)).is_some(), "another address is still served");
        drop(held);
        assert!(AddressSlot::take(&slots, ip(3)).is_some(), "freed slots serve again");
        assert!(slots.lock().unwrap().is_empty(), "freed slots leave no entry behind");
    }

    #[test]
    fn host_header_must_name_this_service() {
        let allowed = allowed_hosts("10.0.1.7", 8787);
        assert!(host_allowed(Some("10.0.1.7:8787"), &allowed));
        assert!(host_allowed(Some("LOCALHOST:8787"), &allowed));
        assert!(host_allowed(Some(" 127.0.0.1:8787 "), &allowed));
        assert!(!host_allowed(None, &allowed));
        assert!(!host_allowed(Some("10.0.1.7"), &allowed), "port is part of the name");
        assert!(!host_allowed(Some("10.0.1.7:8788"), &allowed));
        assert!(!host_allowed(Some("evil.example:8787"), &allowed), "DNS rebinding");
        assert!(host_allowed(Some("10.0.1.7"), &allowed_hosts("10.0.1.7", 443)));

        let good = b"GET /api/status?x=1 HTTP/1.1\r\nHost: 10.0.1.7:8787\r\nX-Token: abc\r\nContent-Length: 2\r\n\r\n{}".to_vec();
        let r = read_request(&mut good.as_slice(), &allowed).ok().expect("good request");
        assert_eq!((r.method.as_str(), r.path.as_str(), r.token.as_deref(), r.body.as_slice()), ("GET", "/api/status", Some("abc"), &b"{}"[..]));
        let rebind = b"POST /api/stop HTTP/1.1\r\nHost: evil.example:8787\r\nContent-Length: 2\r\n\r\n{}".to_vec();
        assert!(matches!(read_request(&mut rebind.as_slice(), &allowed), Err(ReadError::BadHost)));
        let no_host = b"GET /api/status HTTP/1.1\r\n\r\n".to_vec();
        assert!(matches!(read_request(&mut no_host.as_slice(), &allowed), Err(ReadError::BadHost)));
        let escape = b"GET /api/\x1b[2Jstatus HTTP/1.1\r\nHost: localhost:8787\r\n\r\n".to_vec();
        assert_eq!(read_request(&mut escape.as_slice(), &allowed).ok().unwrap().path, "/api/[2Jstatus");
    }

    #[test]
    fn safe_sheets_refuse_the_start() {
        let stop = [7u8; 32];
        let (other, sheet) = ([1u8; 32], [9u8; 32]);
        let only_stop = json!({"cancel": stop, "cancel2": other}).to_string();
        assert_eq!(offline_stop_key(&only_stop, Path::new("k.json")), Ok(stop));
        for field in ["cold", "cold2", "cold3"] {
            let with_sheet = json!({"cancel": stop, field: sheet}).to_string();
            assert_eq!(offline_stop_key(&with_sheet, Path::new("k.json")), Err(SAFE_SHEETS_HERE.to_string()), "{field}");
        }
        assert!(offline_stop_key(&json!({"cold": null}).to_string(), Path::new("k.json")).unwrap_err().contains("no stop secret"));
        assert!(no_safe_sheet(&Keys::default()).is_ok());
        assert_eq!(no_safe_sheet(&Keys { cold3: Some([1; 32]), ..Default::default() }), Err(SAFE_SHEETS_HERE.to_string()));
    }

    #[test]
    fn mistyped_offline_sheet_is_not_echoed() {
        // One byte of the stop secret out of range, and one that is a word: the parser's own text
        // would quote the bad value, so the message is fixed and names only the file.
        let mut bytes: Vec<Value> = (0..32).map(|_| json!(7)).collect();
        bytes[5] = json!(300);
        let path = Path::new("C:/vault/offline/keys.json");
        let want = format!("{} is not an offline keys.json this program can read. Restore the original file.", path.display());
        assert_eq!(offline_stop_key(&json!({"cancel": bytes}).to_string(), path), Err(want.clone()));
        assert_eq!(offline_stop_key(&json!({"cancel": "seven"}).to_string(), path), Err(want.clone()));
        assert_eq!(offline_stop_key("{not json", path), Err(want));
        let err = offline_stop_key(&json!({"cancel": [300]}).to_string(), path).unwrap_err();
        assert!(!err.contains("300") && !err.contains("u8") && !err.contains("invalid"), "{err}");
    }

    #[test]
    fn host_list_follows_a_new_lease() {
        let frozen = allowed_hosts("10.0.1.7", 8787);
        assert!(!host_allowed(Some("10.0.0.5:8787"), &frozen));
        let now = hosts_with(&frozen, "10.0.0.5", 8787);
        assert!(host_allowed(Some("10.0.0.5:8787"), &now));
        assert!(host_allowed(Some("10.0.1.7:8787"), &now), "the old address still works");
        assert_eq!(hosts_with(&frozen, "10.0.1.7", 8787), frozen, "same address adds nothing");
        assert!(!host_allowed(Some("10.0.0.5:8788"), &now));
    }


    /// The phone library: the https route and a direct `Core::call` are one body. No node
    /// in the test, so both answer the same 503 with the network-down words (the route's old shape).
    #[tokio::test]
    async fn call_status_and_route_status_give_one_body() {
        let dir = std::env::temp_dir().join(format!("kv-serve-call-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let net = Network::testnet();
        let rpc = KaspaRpcClient::new(WrpcEncoding::Borsh, Some("ws://127.0.0.1:9"), None, Some(net.resolver_id()), None).unwrap();
        let mut keys = Keys::fresh(&net);
        (keys.heir, keys.cold, keys.cold2, keys.cold3) = (None, None, None, None);
        let ctx = Ctx { dir: dir.clone(), keys, rpc: Some(rpc), force: false, network: net.clone(), dials: Dials::testnet(), account: 1 };
        let core = Arc::new(Core::open(ctx).unwrap());
        let (tls, cert) = load_or_make_identity(&dir, "127.0.0.1");
        let hash = cert_sha256(&cert);
        let banner = Banner { ip: "127.0.0.1".into(), port: 8787, words: words_of(&hash), long_words: long_words_of(&hash), fingerprint: fingerprint_of(&hash) };
        let srv = Arc::new(Srv {
            core: core.clone(),
            pairing: Mutex::new(Pairing::new("12345678".into(), "tok".into(), Instant::now())),
            banner,
            tls,
            hosts: allowed_hosts("127.0.0.1", 8787),
            refusals: Mutex::new((Instant::now(), 0)),
        });
        let request = Request { method: "GET".into(), path: "/api/status".into(), token: Some("tok".into()), query: String::new(), body: vec![] };
        let routed = route(srv, request).await;
        let direct = core.call("status", &json!({})).await;
        assert_eq!(routed, direct);
        assert_eq!(routed.0, 503, "no node in the test: the network-down answer");
        assert_eq!(routed.1["error"], NETWORK_DOWN);
        assert_eq!(core.call("nothing", &json!({})).await, routed, "an unknown method is refused after the node check, as the route always was");
        assert_eq!(core.call("status", &json!([1])).await, (400, refusal("Bad request")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
