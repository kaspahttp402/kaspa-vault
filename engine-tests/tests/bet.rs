//! Matched-bet contract through the real Kaspa script engine.
//! Each attack is a one-line change to an accepted baseline.

use kaspa_consensus_core::hashing::sighash::SigHashReusedValuesUnsync;
use kaspa_consensus_core::tx::{
    PopulatedTransaction, ScriptPublicKey, Transaction, TransactionId, TransactionInput, TransactionOutpoint, TransactionOutput,
    UtxoEntry, VerifiableTransaction,
};
use kaspa_txscript::caches::Cache;
use kaspa_txscript::covenants::CovenantsContext;
use kaspa_txscript::script_builder::ScriptBuilder;
use kaspa_txscript::{EngineCtx, EngineFlags, TxScriptEngine, pay_to_script_hash_script};
use kaspa_txscript_errors::TxScriptError;
use secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use silverscript_abi::{ArtifactValue as V, SilAbiArtifact, encode_contract_entry_sig_script};
use silverscript_lang::compiler::{CompileOptions, compile_to_sil_abi_artifact_with_options};

const KAS: u64 = 100_000_000;
const STAKE: u64 = 5 * KAS;
const POT: u64 = 2 * STAKE;
const MAX_FEE: u64 = KAS / 20;
const REFUND_AFTER: u64 = 3_000;
const UTXO_DAA: u64 = 1_000_000;

fn key(n: u8) -> Keypair {
    Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[n; 32]).unwrap())
}
fn pk(k: &Keypair) -> Vec<u8> {
    k.x_only_public_key().0.serialize().to_vec()
}
fn refs() -> [Keypair; 3] {
    [key(1), key(2), key(3)]
}
fn spk(tag: u8) -> ScriptPublicKey {
    let mut s = vec![0x20];
    s.extend_from_slice(&[tag; 32]);
    s.push(0xac);
    ScriptPublicKey::new(0, s.into())
}
fn yes_spk() -> ScriptPublicKey {
    spk(41)
}
fn no_spk() -> ScriptPublicKey {
    spk(42)
}
fn evil() -> ScriptPublicKey {
    spk(66)
}
fn hash(b: &[u8]) -> Vec<u8> {
    blake2b_simd::Params::new().hash_length(32).hash(b).as_bytes().to_vec()
}
fn spk_hash(s: &ScriptPublicKey) -> Vec<u8> {
    let mut b = s.version().to_be_bytes().to_vec();
    b.extend_from_slice(s.script());
    hash(&b)
}
fn digest(market: u8, outcome: u8) -> Vec<u8> {
    hash(&[&[market; 32][..], &[outcome]].concat())
}

fn compile() -> SilAbiArtifact {
    let path = std::env::var("BET_SRC").unwrap_or_else(|_| format!("{}/../contracts/bet.sil", env!("CARGO_MANIFEST_DIR")));
    let src = std::fs::read_to_string(path).unwrap();
    let [a, b, c] = refs();
    let args = vec![
        V::Bytes(pk(&a)),
        V::Bytes(pk(&b)),
        V::Bytes(pk(&c)),
        V::Bytes(digest(7, 1)),
        V::Bytes(digest(7, 0)),
        V::Bytes(spk_hash(&yes_spk())),
        V::Bytes(spk_hash(&no_spk())),
        V::Int(STAKE as i64),
        V::Int(MAX_FEE as i64),
        V::Int(REFUND_AFTER as i64),
    ];
    compile_to_sil_abi_artifact_with_options(&src, &args, CompileOptions::default()).expect("bet compiles")
}

#[derive(Clone)]
struct Try {
    entry: &'static str,
    args: Vec<V>,
    outputs: Vec<TransactionOutput>,
    seq: u64,
    value: u64,
    extra_input: bool,
    bet_at: usize,
}

fn run(t: Try) -> Result<(), TxScriptError> {
    let art = compile();
    let bytecode = art.contracts.values().next().unwrap().compiled.bytecode.clone();
    let bet = UtxoEntry::new(t.value, pay_to_script_hash_script(&bytecode), UTXO_DAA, false, None);
    let mut entries = vec![bet];
    if t.extra_input {
        entries.insert(if t.bet_at == 0 { 1 } else { 0 }, UtxoEntry::new(KAS, spk(50), UTXO_DAA, false, None));
    }
    let inputs: Vec<_> = (0..entries.len())
        .map(|i| {
            let seq = if i == t.bet_at { t.seq } else { 0 };
            TransactionInput::new(TransactionOutpoint::new(TransactionId::from_bytes([i as u8 + 1; 32]), i as u32), vec![], seq, 4)
        })
        .collect();
    let mut tx = Transaction::new(1, inputs, t.outputs.clone(), 0, Default::default(), 0, vec![]);
    let name = art.contracts.keys().next().unwrap().clone();
    let mut script = encode_contract_entry_sig_script(&art, &name, t.entry, &t.args).expect("encode entry");
    script.extend(ScriptBuilder::with_flags(EngineFlags::default()).add_data(&bytecode).unwrap().drain());
    tx.inputs[t.bet_at].signature_script = script;
    let reused = SigHashReusedValuesUnsync::new();
    let cache = Cache::new(1_000);
    let input = tx.inputs[t.bet_at].clone();
    let pop = PopulatedTransaction::new(&tx, entries);
    let cov = CovenantsContext::from_tx(&pop).map_err(TxScriptError::from)?;
    let utxo = pop.utxo(t.bet_at).expect("utxo");
    let mut vm = TxScriptEngine::from_transaction_input(
        &pop,
        &input,
        t.bet_at,
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
        Err(_) => println!("PASS reject  {name}"),
        Ok(()) => panic!("{name}: should be rejected"),
    }
}

fn sig(k: &Keypair, d: &[u8]) -> V {
    V::Bytes(k.sign_schnorr(Message::from_digest_slice(d).unwrap()).as_ref().to_vec())
}
fn pay(value: u64, to: ScriptPublicKey) -> TransactionOutput {
    TransactionOutput::new(value, to)
}
/// Referees `a` and `b` (indexes) sign `outcome`; the third is skipped.
fn settle(outcome: i64, a: usize, b: usize) -> Try {
    let r = refs();
    let d = digest(7, outcome as u8);
    let skip = 3 - a - b;
    let to = if outcome == 1 { yes_spk() } else { no_spk() };
    Try {
        entry: "settle",
        args: vec![V::Int(outcome), V::Int(skip as i64), sig(&r[a], &d), sig(&r[b], &d)],
        outputs: vec![pay(POT - MAX_FEE, to)],
        seq: 0,
        value: POT,
        extra_input: false,
        bet_at: 0,
    }
}
fn refund() -> Try {
    Try {
        entry: "refund",
        args: vec![],
        outputs: vec![pay(STAKE - MAX_FEE / 2, yes_spk()), pay(STAKE - MAX_FEE / 2, no_spk())],
        seq: REFUND_AFTER,
        value: POT,
        extra_input: false,
        bet_at: 0,
    }
}

#[test]
fn accepted_flows() {
    ok("A1 YES by referees 1 and 2", settle(1, 0, 1));
    ok("A2 YES by referees 1 and 3", settle(1, 0, 2));
    ok("A3 YES by referees 2 and 3", settle(1, 1, 2));
    ok("A4 NO by referees 1 and 2", settle(0, 0, 1));
    ok("A5 NO by referees 2 and 3", settle(0, 1, 2));
    ok("A6 refund after the wait", refund());
    let mut t = refund();
    t.seq = REFUND_AFTER + 500;
    ok("A7 refund later than the wait", t);
    let mut t = settle(1, 0, 1);
    t.outputs[0].value = POT;
    ok("A8 settle at zero fee in the script", t);
    let mut t = refund();
    t.outputs[0].value = STAKE - MAX_FEE;
    ok("A9 first refund at the exact limit", t);
    let mut t = refund();
    t.outputs[1].value = STAKE - MAX_FEE;
    ok("A10 second refund at the exact limit", t);
}

#[test]
fn referee_attacks() {
    let r = refs();
    let thief = key(9);
    let d = digest(7, 1);
    let mut t = settle(1, 0, 1);
    t.args[2] = t.args[3].clone();
    t.args[1] = V::Int(2);
    no("R1 one referee signs twice", t);
    let mut t = settle(1, 0, 1);
    t.args[3] = sig(&thief, &d);
    no("R2 outsider replaces a referee", t);
    let mut t = settle(1, 0, 1);
    t.args[2] = sig(&thief, &d);
    t.args[3] = sig(&key(8), &d);
    no("R3 two outsiders", t);
    let mut t = settle(1, 0, 1);
    t.args[2] = sig(&r[0], &digest(7, 0));
    no("R4 one referee signed the other outcome", t);
    let mut t = settle(1, 0, 1);
    t.args[2] = sig(&r[0], &digest(8, 1));
    t.args[3] = sig(&r[1], &digest(8, 1));
    no("R5 signatures from a different market", t);
    let mut t = settle(1, 0, 1);
    t.args[1] = V::Int(0);
    no("R6 skip index does not match the signers", t);
    let mut t = settle(1, 0, 1);
    t.args[1] = V::Int(3);
    no("R7 skip index out of range", t);
    let mut t = settle(1, 0, 1);
    t.args[1] = V::Int(-1);
    no("R8 negative skip index", t);
}

#[test]
fn payout_attacks() {
    let mut t = settle(1, 0, 1);
    t.outputs[0].script_public_key = no_spk();
    no("P1 YES result pays the NO player", t);
    let mut t = settle(0, 0, 1);
    t.outputs[0].script_public_key = yes_spk();
    no("P2 NO result pays the YES player", t);
    let mut t = settle(1, 0, 1);
    t.outputs[0].script_public_key = evil();
    no("P3 referees pay an outsider", t);
    let mut t = settle(1, 0, 1);
    t.outputs[0].value = POT - MAX_FEE - 1;
    no("P4 fee over the limit", t);
    let mut t = settle(1, 0, 1);
    t.outputs = vec![pay(POT - KAS, yes_spk()), pay(KAS - MAX_FEE, evil())];
    no("P5 second output skims the pot", t);
    let mut t = settle(1, 0, 1);
    t.outputs = vec![];
    no("P6 no outputs", t);
    let mut t = settle(1, 0, 1);
    t.args[0] = V::Int(2);
    no("P7 outcome that does not exist", t);
    let mut t = settle(1, 0, 1);
    t.extra_input = true;
    no("P8 outside coin as a second input", t);
    let mut t = settle(1, 0, 1);
    t.extra_input = true;
    t.bet_at = 1;
    no("P9 bet coin not at input 0", t);
    let mut t = settle(1, 0, 1);
    t.outputs = vec![pay(POT - MAX_FEE, yes_spk()), pay(MAX_FEE / 2, evil())];
    no("P10 second output takes the fee allowance", t);
}

#[test]
fn refund_attacks() {
    let mut t = refund();
    t.seq = REFUND_AFTER - 1;
    no("F1 refund before the wait", t);
    let mut t = refund();
    t.seq = 0;
    no("F2 refund with no wait", t);
    let mut t = refund();
    t.outputs.swap(0, 1);
    no("F3 refund to swapped players", t);
    let mut t = refund();
    t.outputs[1].script_public_key = evil();
    no("F4 refund pays an outsider", t);
    let mut t = refund();
    t.outputs[0].script_public_key = evil();
    no("F5 refund's first output pays an outsider", t);
    let mut t = refund();
    t.outputs[0].value = STAKE - MAX_FEE - 1;
    no("F6 first player short-changed", t);
    let mut t = refund();
    t.outputs[1].value = STAKE - MAX_FEE - 1;
    no("F7 second player short-changed", t);
    let mut t = refund();
    t.outputs.push(pay(KAS, evil()));
    no("F8 third output skims", t);
    let mut t = refund();
    t.outputs.pop();
    no("F9 one player left out", t);
    let mut t = refund();
    t.extra_input = true;
    no("F10 outside coin as a second input", t);
    let mut t = refund();
    t.extra_input = true;
    t.bet_at = 1;
    no("F11 bet coin not at input 0", t);
}
