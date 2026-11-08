//! Second independent adversarial review: topup, inherit, three stop
//! keys, and their interaction with spend/request/finish/cancel. Helpers copied
//! from vault.rs (plus a few knobs: sig trimming, inheritDelay, default key config).
//! Every attack is a one-line change from an accepted baseline.
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::hashing::sighash::{SigHashReusedValuesUnsync, calc_schnorr_signature_hash};
use kaspa_consensus_core::hashing::sighash_type::{SIG_HASH_ALL, SIG_HASH_ANY_ONE_CAN_PAY, SIG_HASH_NONE, SIG_HASH_SINGLE, SigHashType};
use kaspa_consensus_core::tx::{
    CovenantBinding, MutableTransaction, PopulatedTransaction, ScriptPublicKey, Transaction, TransactionId, TransactionInput,
    TransactionOutpoint, TransactionOutput, UtxoEntry, VerifiableTransaction,
};
use kaspa_txscript::caches::Cache;
use kaspa_txscript::covenants::CovenantsContext;
use kaspa_txscript::script_builder::ScriptBuilder;
use kaspa_txscript::{EngineCtx, EngineFlags, TxScriptEngine, pay_to_script_hash_script};
use kaspa_txscript_errors::TxScriptError;
use secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use silverscript_abi::{ArtifactValue as V, SilAbiArtifact, encode_contract_entry_sig_script};
use silverscript_lang::compiler::{CompileOptions, compile_to_sil_abi_artifact_with_options};

fn source() -> String {
    let path = std::env::var("VAULT_SRC").unwrap_or_else(|_| "../contracts/vault.sil".to_string());
    std::fs::read_to_string(path).expect("read contract")
}
const COV_A: Hash = Hash::from_bytes(*b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
const COV_B: Hash = Hash::from_bytes(*b"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
const KAS: u64 = 100_000_000;
const VAL: u64 = 1_000 * KAS;
const DELAY: u64 = 2_592_000;
const CAP: u64 = 10 * KAS;
const WINDOW: u64 = 864_000;
const MAX_FEE: u64 = KAS / 100;
const MIN_REM: u64 = KAS;
const UTXO_DAA: u64 = 1_000_000;
const INHERIT: u64 = 20_000_000;
const DEFAULT_INHERIT: u64 = 4_000_000_000;

fn key(n: u8) -> Keypair {
    Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[n; 32]).unwrap())
}
fn pk(k: &Keypair) -> Vec<u8> {
    k.x_only_public_key().0.serialize().to_vec()
}
fn spend_key() -> Keypair {
    key(1)
}
fn cancel_key() -> Keypair {
    key(2)
}
fn thief() -> Keypair {
    key(9)
}
fn cancel_key2() -> Keypair {
    key(3)
}
fn cancel_key3() -> Keypair {
    key(4)
}
fn heir_key() -> Keypair {
    key(5)
}
fn spk(tag: u8) -> ScriptPublicKey {
    let mut s = vec![0x20];
    s.extend_from_slice(&[tag; 32]);
    s.push(0xac);
    ScriptPublicKey::new(0, s.into())
}
fn cold() -> ScriptPublicKey {
    spk(40)
}
fn dest() -> ScriptPublicKey {
    spk(41)
}
fn evil() -> ScriptPublicKey {
    spk(66)
}
fn heir() -> ScriptPublicKey {
    spk(42)
}
fn spk_bytes(s: &ScriptPublicKey) -> Vec<u8> {
    let mut b = s.version().to_be_bytes().to_vec();
    b.extend_from_slice(s.script());
    b
}
fn spk_hash(s: &ScriptPublicKey) -> Vec<u8> {
    let b = spk_bytes(s);
    blake2b_simd::Params::new().hash_length(32).hash(&b).as_bytes().to_vec()
}

#[derive(Clone)]
struct St {
    stage: i64,
    dest: Vec<u8>,
    amount: i64,
    ws: i64,
    spent: i64,
}
fn locked() -> St {
    St { stage: 0, dest: vec![0; 32], amount: 0, ws: 0, spent: 0 }
}
fn pending(amount: u64) -> St {
    St { stage: 1, dest: spk_hash(&dest()), amount: amount as i64, ws: 0, spent: 0 }
}
fn spent(kas: u64) -> St {
    St { spent: (kas * KAS) as i64, ..locked() }
}

/// Contract configuration knobs the review varies.
#[derive(Clone)]
struct Cfg {
    inherit_delay: i64,
    /// Unused slots as the CLI passes them: stop keys 2,3 = cancelPk, heirPk = cancelPk,
    /// heirSpkHash = coldSpkHash, inheritDelay = 4e9.
    defaults: bool,
}
fn cfg() -> Cfg {
    Cfg { inherit_delay: INHERIT as i64, defaults: false }
}
fn compile_cfg(s: &St, c: &Cfg) -> Result<SilAbiArtifact, String> {
    let (k2, k3, hk, hs, id) = if c.defaults {
        (pk(&cancel_key()), pk(&cancel_key()), pk(&cancel_key()), spk_hash(&cold()), DEFAULT_INHERIT as i64)
    } else {
        (pk(&cancel_key2()), pk(&cancel_key3()), pk(&heir_key()), spk_hash(&heir()), c.inherit_delay)
    };
    let args = vec![
        V::Bytes(pk(&spend_key())),
        V::Bytes(pk(&cancel_key())),
        V::Bytes(k2),
        V::Bytes(k3),
        V::Bytes(spk_hash(&cold())),
        V::Bytes(hk),
        V::Bytes(hs),
        V::Int(id),
        V::Int(0), // autoHeir (v5)
        V::Int(0), // hardLimit (v6): off, this suite covers the request path
        V::Int(DELAY as i64),
        V::Int(CAP as i64),
        V::Int(WINDOW as i64),
        V::Int(MAX_FEE as i64),
        V::Int(MIN_REM as i64),
        V::Int(s.stage),
        V::Bytes(s.dest.clone()),
        V::Int(s.amount),
        V::Int(s.ws),
        V::Int(s.spent),
    ];
    compile_to_sil_abi_artifact_with_options(&source(), &args, CompileOptions::default()).map_err(|e| format!("{e:?}"))
}
fn code(a: &SilAbiArtifact) -> Vec<u8> {
    a.contracts.values().next().unwrap().compiled.bytecode.clone()
}
fn vault_spk(s: &St) -> ScriptPublicKey {
    pay_to_script_hash_script(&code(&compile_cfg(s, &cfg()).expect("vault compiles")))
}
fn pay(value: u64, to: ScriptPublicKey) -> TransactionOutput {
    TransactionOutput { value, script_public_key: to, covenant: None }
}
fn cont(value: u64, s: &St) -> TransactionOutput {
    TransactionOutput {
        value,
        script_public_key: vault_spk(s),
        covenant: Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_A }),
    }
}

#[derive(Clone)]
struct Try {
    st: St,
    value: u64,
    entry: &'static str,
    signer: Option<Keypair>,
    hash_type: SigHashType,
    /// Drop the hash-type byte: a 64-byte signature.
    trim_sig: bool,
    extra: Vec<V>,
    seq: u64,
    lock: u64,
    outputs: Vec<TransactionOutput>,
    cov: Option<Hash>,
    vault_at: usize,
    others: Vec<UtxoEntry>,
    cfg: Cfg,
    /// Build the signature script by hand: sig push, then these raw pushes, then the
    /// dispatch tag. Skips the ABI encoder's own type and length checks.
    raw: Option<Vec<Vec<u8>>>,
}
fn base(st: St, entry: &'static str, signer: Option<Keypair>, outputs: Vec<TransactionOutput>) -> Try {
    Try {
        st,
        value: VAL,
        entry,
        signer,
        hash_type: SIG_HASH_ALL,
        trim_sig: false,
        extra: vec![],
        seq: 0,
        lock: 0,
        outputs,
        cov: Some(COV_A),
        vault_at: 0,
        others: vec![],
        cfg: cfg(),
        raw: None,
    }
}

fn run(t: Try) -> Result<(), TxScriptError> {
    let art = match compile_cfg(&t.st, &t.cfg) {
        Ok(a) => a,
        Err(e) => return Err(TxScriptError::NumberTooBig(format!("compiler refused: {e}"))),
    };
    let bytecode = code(&art);
    let vault = UtxoEntry::new(t.value, pay_to_script_hash_script(&bytecode), UTXO_DAA, false, t.cov);
    let mut entries = t.others.clone();
    entries.insert(t.vault_at, vault);
    let inputs: Vec<_> = (0..entries.len())
        .map(|i| {
            let seq = if i == t.vault_at { t.seq } else { 0 };
            TransactionInput::new(TransactionOutpoint::new(TransactionId::from_bytes([i as u8 + 1; 32]), i as u32), vec![], seq, 4)
        })
        .collect();
    let mut tx = Transaction::new(1, inputs, t.outputs.clone(), t.lock, Default::default(), 0, vec![]);
    let mut args = vec![];
    if let Some(k) = &t.signer {
        let reused = SigHashReusedValuesUnsync::new();
        let mtx = MutableTransaction::with_entries(tx.clone(), entries.clone());
        let hash = calc_schnorr_signature_hash(&mtx.as_verifiable(), t.vault_at, t.hash_type, &reused);
        let sig = k.sign_schnorr(Message::from_digest_slice(hash.as_bytes().as_slice()).unwrap());
        let mut b = sig.as_ref().to_vec();
        if !t.trim_sig {
            b.push(t.hash_type.to_u8());
        }
        args.push(V::Bytes(b));
    }
    args.extend(t.extra.clone());
    let name = art.contracts.keys().next().unwrap().clone();
    let mut script = if let Some(raw) = &t.raw {
        let contract = art.contracts.get(&name).unwrap();
        let tag = contract.entries.get(t.entry).expect("entry").dispatch_tag;
        let mut b = ScriptBuilder::with_flags(EngineFlags::default());
        if let Some(V::Bytes(sig)) = args.first() {
            b.add_data(sig).unwrap();
        }
        for r in raw {
            b.add_data(r).unwrap();
        }
        b.add_data(tag.as_bytes()).unwrap();
        b.drain()
    } else {
        match encode_contract_entry_sig_script(&art, &name, t.entry, &args) {
            Ok(s) => s,
            Err(e) => return Err(TxScriptError::NumberTooBig(format!("ABI encode refused: {e:?}"))),
        }
    };
    script.extend(ScriptBuilder::with_flags(EngineFlags::default()).add_data(&bytecode).unwrap().drain());
    tx.inputs[t.vault_at].signature_script = script;
    let reused = SigHashReusedValuesUnsync::new();
    let cache = Cache::new(1_000);
    let input = tx.inputs[t.vault_at].clone();
    let pop = PopulatedTransaction::new(&tx, entries);
    let cov = CovenantsContext::from_tx(&pop).map_err(TxScriptError::from)?;
    let utxo = pop.utxo(t.vault_at).expect("utxo");
    let mut vm = TxScriptEngine::from_transaction_input(
        &pop,
        &input,
        t.vault_at,
        utxo,
        EngineCtx::new(&cache).with_reused(&reused).with_covenants_ctx(&cov),
        EngineFlags { sigop_script_units: 0.into() },
    );
    vm.execute()
}
fn ok(name: &str, t: Try) {
    match run(t) {
        Ok(()) => println!("PASS accept  {name}"),
        Err(e) => panic!("{name}: should be accepted, got {e:?}"),
    }
}
fn no(name: &str, t: Try) {
    match run(t) {
        Ok(()) => panic!("{name}: ATTACK ACCEPTED"),
        Err(e) => println!("PASS reject  {name}: {e:?}"),
    }
}
/// Documented behaviour we want to see in the log without failing the suite.
fn note(name: &str, t: Try) {
    match run(t) {
        Ok(()) => println!("NOTE accept  {name}"),
        Err(e) => println!("NOTE reject  {name}: {e:?}"),
    }
}

// Accepted baselines (same as vault.rs).
fn spend5() -> Try {
    base(locked(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &spent(5))])
}
fn req(amount: u64) -> Try {
    let mut t = base(locked(), "request", Some(spend_key()), vec![cont(VAL - MAX_FEE, &pending(amount))]);
    t.extra = vec![V::Bytes(spk_bytes(&dest())), V::Int(amount as i64)];
    t
}
fn finish_part() -> Try {
    let a = 100 * KAS;
    let mut t = base(pending(a), "finish", None, vec![pay(a, dest()), cont(VAL - a - MAX_FEE, &locked())]);
    t.seq = DELAY;
    t
}
fn cancel_with(st: St, which: usize) -> Try {
    let keys = [cancel_key(), cancel_key2(), cancel_key3()];
    let mut t = base(st, "cancel", Some(keys[which].clone()), vec![pay(VAL - MAX_FEE, cold())]);
    t.extra = vec![V::Int(which as i64)];
    t
}
fn inherit(st: St) -> Try {
    let mut t = base(st, "inherit", Some(heir_key()), vec![pay(VAL - MAX_FEE, heir())]);
    t.seq = INHERIT;
    t
}
fn topup(st: St) -> Try {
    let mut t = base(st.clone(), "topup", Some(spend_key()), vec![cont(VAL + 9 * KAS, &st)]);
    t.others = vec![UtxoEntry::new(10 * KAS, spk(50), 5, false, None)];
    t
}
fn plain(value: u64) -> UtxoEntry {
    UtxoEntry::new(value, spk(50), 5, false, None)
}

#[test]
fn baselines() {
    ok("B spend", spend5());
    ok("B request", req(100 * KAS));
    ok("B finish", finish_part());
    ok("B cancel", cancel_with(locked(), 0));
    ok("B inherit", inherit(locked()));
    ok("B topup", topup(locked()));
}

#[test]
fn sighash_types_everywhere() {
    // Every signed entry, every hash type that is not plain SIGHASH_ALL.
    let types: [(&str, SigHashType); 5] = [
        ("NONE", SIG_HASH_NONE),
        ("SINGLE", SIG_HASH_SINGLE),
        ("ALL|ACP", SIG_HASH_ALL | SIG_HASH_ANY_ONE_CAN_PAY),
        ("NONE|ACP", SIG_HASH_NONE | SIG_HASH_ANY_ONE_CAN_PAY),
        ("SINGLE|ACP", SIG_HASH_SINGLE | SIG_HASH_ANY_ONE_CAN_PAY),
    ];
    let bases: [(&str, fn() -> Try); 5] = [
        ("spend", spend5),
        ("request", || req(100 * KAS)),
        ("topup", || topup(locked())),
        ("cancel", || cancel_with(locked(), 0)),
        ("inherit", || inherit(locked())),
    ];
    for (ename, mk) in bases {
        for (tname, ht) in types {
            let mut t = mk();
            t.hash_type = ht;
            no(&format!("S1 {ename} with SIGHASH_{tname}"), t);
        }
        let mut t = mk();
        t.trim_sig = true;
        t.raw = Some(match ename {
            "request" => vec![spk_bytes(&dest()), (100 * KAS as i64).to_le_bytes()[..5].to_vec()],
            "cancel" => vec![vec![]],
            _ => vec![],
        });
        no(&format!("S2 {ename} with a 64-byte signature (no hash-type byte), raw script"), t);
    }
}

#[test]
fn topup_attacks() {
    // Value smuggling: continuation pays exactly the vault input while a second
    // output or the fee takes the outside coin; vault money never shrinks.
    let mut t = topup(locked());
    t.outputs[0].value = VAL;
    t.others = vec![];
    note("T1 topup of nothing with no outside input (fee 0, consensus/mempool matter)", t);
    let mut t = topup(locked());
    t.outputs[0].value = VAL + 9 * KAS;
    t.outputs.push(cont(KAS / 2, &locked()));
    no("T2 topup with a dust second continuation", t);
    let mut t = topup(locked());
    t.outputs[0].covenant = Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_B });
    no("T3 topup continuation bound to a foreign covenant id", t);
    let mut t = topup(locked());
    t.outputs[0].covenant = Some(CovenantBinding { authorizing_input: 1, covenant_id: COV_A });
    no("T4 topup continuation authorized by the plain input", t);
    // A second vault (other id) used as the outside coin: input 0 accepts, but the
    // twin's own script at input 1 must refuse (every entry demands input 0).
    let twin_b = UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_B));
    let mut t = topup(locked());
    t.outputs[0].value = 2 * VAL;
    t.others = vec![twin_b.clone()];
    note("T5a topup absorbing vault B, seen from input 0 (A's script)", t);
    let mut t = topup(locked());
    t.outputs[0].value = 2 * VAL;
    t.vault_at = 1;
    t.cov = Some(COV_B);
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_A))];
    no("T5b topup absorbing vault B, seen from input 1 (B's script)", t);
    // A lineage-less coin at the vault address used as the outside coin.
    let mut t = topup(locked());
    t.outputs[0].value = 2 * VAL;
    t.vault_at = 1;
    t.cov = None;
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_A))];
    no("T6 lineage-less vault-address coin at input 1 feeding a topup", t);
    for entry in ["spend", "cancel", "inherit", "finish"] {
        let mut t = topup(locked());
        t.outputs[0].value = 2 * VAL;
        t.vault_at = 1;
        t.cov = None;
        t.entry = entry;
        t.seq = INHERIT;
        t.signer = match entry {
            "cancel" => Some(cancel_key()),
            "inherit" => Some(heir_key()),
            "finish" => None,
            _ => Some(spend_key()),
        };
        t.extra = if entry == "cancel" { vec![V::Int(0)] } else { vec![] };
        t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_A))];
        no(&format!("T6b lineage-less coin at input 1 leaving through {entry}"), t);
    }
    // Griefing a request: nobody can renew a Pending coin through topup.
    for (name, k) in [("spend key", spend_key()), ("stop key", cancel_key()), ("heir", heir_key()), ("thief", thief())] {
        let mut t = topup(pending(100 * KAS));
        t.signer = Some(k);
        no(&format!("T7 {name} tops up a Pending vault"), t);
    }
    // Topup with sequence/locktime set: not checked by the entry, harmless.
    let mut t = topup(locked());
    t.seq = 5;
    t.lock = 7;
    note("T8 topup with a sequence and locktime (unconstrained, state unchanged)", t);
    // Topup from Locked but writing the destHash of an old request.
    let mut t = topup(locked());
    t.outputs[0] = cont(VAL + 9 * KAS, &St { dest: spk_hash(&dest()), ..locked() });
    no("T9 topup rewriting destHash", t);
    // Two vault coins with the same id at inputs 0 and 1, one fat continuation.
    let mut t = topup(locked());
    t.outputs[0].value = 2 * VAL;
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_A))];
    no("T10 same-id twin merged by topup", t);
    // Topup where the continuation is the Pending script of another state.
    let mut t = topup(spent(7));
    t.outputs[0] = cont(VAL + 9 * KAS, &locked());
    no("T11 topup wiping the spent counter", t);
}

#[test]
fn inherit_attacks() {
    let mut t = inherit(locked());
    t.seq = INHERIT | (1 << 63);
    no("I1 inherit with the sequence disable flag set", t);
    let mut t = inherit(locked());
    t.seq = INHERIT | (1 << 40);
    note("I2 inherit with non-consensus high bits (masked by engine and consensus alike)", t);
    let mut t = inherit(locked());
    t.seq = u64::MAX >> 1;
    note("I3 inherit with sequence 2^63-1 (masked to 2^32-1 >= delay)", t);
    let mut t = inherit(locked());
    t.seq = (1 << 32) | 5;
    no("I4 inherit where only bits above 32 are large and the low word is 5", t);
    // inheritDelay misconfigured at or above 2^32: inherit must never pass.
    let mut t = inherit(locked());
    t.cfg.inherit_delay = 1 << 32;
    t.seq = u64::MAX >> 1;
    no("I5 inheritDelay = 2^32 (out of CSV domain) with a maximal sequence", t);
    let mut t = inherit(locked());
    t.cfg.inherit_delay = -1;
    t.seq = 0;
    no("I6 inheritDelay = -1 with sequence 0", t);
    let mut t = inherit(locked());
    t.cfg.inherit_delay = 0;
    t.seq = 0;
    note("I7 inheritDelay = 0: heir can take at once (configuration, not a flaw)", t);
    // Default configuration: heir slot = cancel key, heir script = cold, 4e9 delay.
    let mut t = inherit(locked());
    t.cfg.defaults = true;
    t.signer = Some(cancel_key());
    t.outputs[0].script_public_key = cold();
    t.seq = DEFAULT_INHERIT;
    note("I8a default config: stop key 'inherits' to cold after 4e9 DAA", t);
    let mut t = inherit(locked());
    t.cfg.defaults = true;
    t.signer = Some(cancel_key());
    t.outputs[0].script_public_key = cold();
    t.seq = DEFAULT_INHERIT - 1;
    no("I8b default config: one DAA early", t);
    let mut t = inherit(locked());
    t.cfg.defaults = true;
    t.signer = Some(heir_key());
    t.outputs[0].script_public_key = cold();
    t.seq = DEFAULT_INHERIT;
    no("I8c default config: real heir key is not configured", t);
    // Inherit from Pending must pay the committed destination first (fixed in contract v3).
    let t = inherit(pending(100 * KAS));
    no("I9 (fixed: destination first) inherit from Pending swallowing the pending withdrawal", t);
    // Heir with a bound continuation output funded from outside: lineage survives.
    let mut t = inherit(locked());
    t.outputs.push(cont(2 * KAS, &locked()));
    t.others = vec![plain(3 * KAS)];
    no("I10 (fixed: lineage ends) inherit leaving a COV_A-bound 'vault' output funded from outside", t);
    // Heir paying the heir script from the outside coin only, vault to evil.
    let mut t = inherit(locked());
    t.outputs = vec![pay(VAL - MAX_FEE, heir()), pay(VAL, evil())];
    t.others = vec![plain(VAL)];
    note("I11 inherit: output 0 heir, output 1 anywhere (heir's own surplus)", t);
    let mut t = inherit(locked());
    t.outputs = vec![pay(VAL - MAX_FEE - 1, heir()), pay(VAL + 1, evil())];
    t.others = vec![plain(VAL)];
    no("I12 inherit shorting the heir script by one sompi with an outside coin", t);
    // Two coins: vault at 0, same-id twin at 1 (twin's script would refuse at index 1).
    let mut t = inherit(locked());
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_A))];
    t.outputs[0].value = 2 * VAL - MAX_FEE;
    note("I13 inherit with a same-id twin at input 1, seen from input 0", t);
    let mut t = inherit(locked());
    t.vault_at = 1;
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_A))];
    t.outputs[0].value = 2 * VAL - MAX_FEE;
    no("I13b inherit with a same-id twin, seen from input 1", t);
    // Heir signs with the vault coin's own value but the tx has an input 0 that is plain.
    let mut t = inherit(locked());
    t.vault_at = 1;
    t.others = vec![plain(KAS)];
    no("I14 vault at input 1 behind a plain coin", t);
    // Heir tries the other entries.
    for entry in ["spend", "request", "topup"] {
        let mut t = inherit(locked());
        t.entry = entry;
        t.outputs = vec![cont(VAL, &locked())];
        if entry == "request" {
            t.extra = vec![V::Bytes(spk_bytes(&heir())), V::Int((100 * KAS) as i64)];
            t.outputs = vec![cont(VAL - MAX_FEE, &St { stage: 1, dest: spk_hash(&heir()), amount: (100 * KAS) as i64, ws: 0, spent: 0 })];
        }
        if entry == "spend" {
            t.outputs = vec![pay(5 * KAS - MAX_FEE, heir()), cont(VAL - 5 * KAS, &spent(5))];
        }
        no(&format!("I15 heir key uses {entry}"), t);
    }
}

#[test]
fn cancel_which_attacks() {
    for bad in [i64::MAX, i64::MIN, 1 << 40, -2] {
        let mut t = cancel_with(locked(), 0);
        t.extra = vec![V::Int(bad)];
        no(&format!("C1 which = {bad}"), t);
    }
    // Non-minimal and odd encodings of which.
    let mut t = cancel_with(locked(), 0);
    t.raw = Some(vec![vec![]]);
    note("C2a which pushed as empty bytes (numeric 0), raw script", t);
    let mut t = cancel_with(locked(), 1);
    t.raw = Some(vec![vec![1, 0]]);
    note("C2b which = 1 pushed non-minimally [01 00], raw script", t);
    let mut t = cancel_with(locked(), 0);
    t.raw = Some(vec![vec![0x80]]);
    note("C2c which pushed as negative zero [80], raw script", t);
    let mut t = cancel_with(locked(), 2);
    t.raw = Some(vec![vec![2, 0, 0, 0, 0, 0, 0, 0, 0]]);
    no("C2d which = 2 pushed as 9 bytes, raw script", t);
    let mut t = cancel_with(locked(), 0);
    t.raw = Some(vec![vec![], vec![]]);
    no("C3 cancel with an extra argument, raw script", t);
    let mut t = cancel_with(locked(), 0);
    t.raw = Some(vec![]);
    no("C4 cancel without which, raw script", t);
    // Default config: which 1/2 resolve to cancelPk; a thief still fails.
    for which in 1..3usize {
        let mut t = cancel_with(locked(), which);
        t.cfg.defaults = true;
        t.signer = Some(cancel_key());
        note(&format!("C5 default config, cancel key with which {which}"), t);
        let mut t = cancel_with(locked(), which);
        t.cfg.defaults = true;
        no(&format!("C5b default config, unconfigured key {which} with which {which}"), t);
    }
    // Stop key continues the lineage from outside funds after sending the vault cold.
    let mut t = cancel_with(locked(), 0);
    t.outputs.push(cont(2 * KAS, &locked()));
    t.others = vec![plain(3 * KAS)];
    no("C6 (fixed: lineage ends) cancel leaving a COV_A-bound 'vault' output funded from outside", t);
    let mut t = cancel_with(locked(), 0);
    t.outputs.push(TransactionOutput {
        value: 2 * KAS,
        script_public_key: evil(),
        covenant: Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_A }),
    });
    t.others = vec![plain(3 * KAS)];
    no("C6b (fixed: lineage ends) cancel binding COV_A to an arbitrary script", t);
    // Cold output carrying the covenant binding.
    let mut t = cancel_with(locked(), 0);
    t.outputs[0].covenant = Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_A });
    no("C7 (fixed: lineage ends) cancel whose cold output carries the covenant id", t);
}

#[test]
fn arithmetic_and_order_attacks() {
    // newAmount near i64::MAX: newAmount + maxFee must not wrap.
    let mut t = req(100 * KAS);
    t.extra = vec![V::Bytes(spk_bytes(&dest())), V::Int(i64::MAX - 1)];
    t.outputs = vec![cont(VAL - MAX_FEE, &St { stage: 1, dest: spk_hash(&dest()), amount: i64::MAX - 1, ws: 0, spent: 0 })];
    no("A1 request with newAmount = i64::MAX - 1", t);
    let mut t = req(100 * KAS);
    t.extra = vec![V::Bytes(spk_bytes(&dest())), V::Int(i64::MIN + 5)];
    t.outputs = vec![cont(VAL - MAX_FEE, &St { stage: 1, dest: spk_hash(&dest()), amount: i64::MIN + 5, ws: 0, spent: 0 })];
    no("A2 request with a hugely negative newAmount", t);
    // Spend window arithmetic with a window start near the DAA ceiling.
    let st = St { ws: 499_999_999_999, ..locked() };
    let mut t = base(st.clone(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &St { spent: (5 * KAS) as i64, ..st })]);
    t.lock = 499_999_999_998;
    note("A3 spend with windowStart near the DAA ceiling, no reset", t);
    // Spent counter near i64::MAX in the state: spent + outflow must not wrap.
    let st = St { spent: i64::MAX - 1, ..locked() };
    let mut t = base(st.clone(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &St { spent: i64::MIN + (5 * KAS) as i64 - 2, ..locked() })]);
    t.lock = 0;
    no("A4 spend with spentInWindow = i64::MAX - 1 (wrap would pass the cap)", t);
    // Continuations at the wrong index.
    let mut t = spend5();
    t.outputs.swap(0, 1);
    no("O1 spend with continuation at output 0", t);
    let mut t = finish_part();
    t.outputs.swap(0, 1);
    no("O2 finish with continuation at output 0", t);
    let mut t = finish_part();
    t.outputs[1].covenant = Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_B });
    no("O3 finish continuation under a foreign id", t);
    // Finish by a mempool watcher burning the whole maxFee from the remainder.
    let mut t = finish_part();
    t.outputs[1].value -= 0; // baseline already burns MAX_FEE
    note("O4 finish remainder minus the full maxFee (keyless finisher may burn up to maxFee)", t);
    let mut t = finish_part();
    t.outputs[1].value -= 1;
    no("O5 finish remainder minus maxFee and one sompi", t);
    // Request by spend key committing to the vault's own Locked address (self-harm only).
    let own = vault_spk(&locked());
    let mut t = req(100 * KAS);
    t.extra = vec![V::Bytes(spk_bytes(&own)), V::Int((100 * KAS) as i64)];
    t.outputs = vec![cont(VAL - MAX_FEE, &St { stage: 1, dest: spk_hash(&own), amount: (100 * KAS) as i64, ws: 0, spent: 0 })];
    note("O6 request whose destination is the vault's own address (self-harm, lineage-less result)", t);
    let mut t = spend5();
    t.outputs[0].script_public_key = vault_spk(&locked());
    note("N9 spend whose payee is the vault's own Locked address (lineage-less twin of O6)", t);
}
