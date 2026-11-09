//! Runs vault.sil through the real Kaspa script engine.
//! Every reject case is a small change from an accepted case.
//! The engine sees sequence and locktime values but not chain time: this proves
//! the script demands them, while real waiting has to be proven on testnet.
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::hashing::sighash::{SigHashReusedValuesUnsync, calc_schnorr_signature_hash};
use kaspa_consensus_core::hashing::sighash_type::{SIG_HASH_ALL, SIG_HASH_ANY_ONE_CAN_PAY, SIG_HASH_NONE, SigHashType};
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
const TIMESTAMP_LOCK: u64 = 1_700_000_000_000;

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
fn stop_keys() -> [Keypair; 3] {
    [cancel_key(), cancel_key2(), cancel_key3()]
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
fn hash32(b: &[u8]) -> Vec<u8> {
    blake2b_simd::Params::new().hash_length(32).hash(b).as_bytes().to_vec()
}
fn spk_hash(s: &ScriptPublicKey) -> Vec<u8> {
    hash32(&spk_bytes(s))
}

#[derive(Clone)]
struct St {
    stage: i64,
    dest: Vec<u8>,
    amount: i64,
    ws: i64,
    spent: i64,
    /// Deployer dials review 9 varies (windowSize, maxFee, cancelPk2 == cancelPk).
    window: i64,
    max_fee: i64,
    k2_same: bool,
    /// v5 autoHeir dial: 1 lets anyone send inheritOpen after inheritDelay.
    auto_heir: i64,
    /// v6 hardLimit dial: 1 turns `request` off for good, so the
    /// spend key can never move more than the card limit per window.
    hard_limit: i64,
}
fn locked() -> St {
    St {
        stage: 0,
        dest: vec![0; 32],
        amount: 0,
        ws: 0,
        spent: 0,
        window: WINDOW as i64,
        max_fee: MAX_FEE as i64,
        k2_same: false,
        auto_heir: 0,
        hard_limit: 0,
    }
}
// Vault with the autoHeir dial on.
fn auto() -> St {
    St { auto_heir: 1, ..locked() }
}
fn auto_pending(amount: u64) -> St {
    St { auto_heir: 1, ..pending(amount) }
}
// Vault with the hardLimit dial on (v6): no request path at all.
fn hard() -> St {
    St { hard_limit: 1, ..locked() }
}
// Pending with hardLimit 1: unreachable on chain (request refuses), but the
// engine can start a coin there through initStage, so decline/finish get a baseline.
fn hard_pending(amount: u64) -> St {
    St { hard_limit: 1, ..pending(amount) }
}
// Pending reached after a 5 KAS spend in a window that started at 2_000_000.
fn pending_in_window(amount: u64) -> St {
    St { stage: 1, dest: spk_hash(&dest()), amount: amount as i64, ws: 2_000_000, ..spent(5) }
}
// The state decline must write: Locked, no destination, counters untouched.
fn declined(st: &St) -> St {
    St { stage: 0, dest: vec![0; 32], amount: 0, ..st.clone() }
}
fn pending(amount: u64) -> St {
    St { stage: 1, dest: spk_hash(&dest()), amount: amount as i64, ..locked() }
}
fn spent(kas: u64) -> St {
    St { spent: (kas * KAS) as i64, ..locked() }
}

fn compile(s: &St) -> SilAbiArtifact {
    let args = vec![
        V::Bytes(pk(&spend_key())),
        V::Bytes(pk(&cancel_key())),
        V::Bytes(if s.k2_same { pk(&cancel_key()) } else { pk(&cancel_key2()) }),
        V::Bytes(pk(&cancel_key3())),
        V::Bytes(spk_hash(&cold())),
        V::Bytes(pk(&heir_key())),
        V::Bytes(spk_hash(&heir())),
        V::Int(INHERIT as i64),
        V::Int(s.auto_heir),
        V::Int(s.hard_limit),
        V::Int(DELAY as i64),
        V::Int(CAP as i64),
        V::Int(s.window),
        V::Int(s.max_fee),
        V::Int(MIN_REM as i64),
        V::Int(s.stage),
        V::Bytes(s.dest.clone()),
        V::Int(s.amount),
        V::Int(s.ws),
        V::Int(s.spent),
    ];
    compile_to_sil_abi_artifact_with_options(&source(), &args, CompileOptions::default()).expect("vault compiles")
}
fn code(a: &SilAbiArtifact) -> Vec<u8> {
    a.contracts.values().next().unwrap().compiled.bytecode.clone()
}
fn vault_spk(s: &St) -> ScriptPublicKey {
    pay_to_script_hash_script(&code(&compile(s)))
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
    extra: Vec<V>,
    seq: u64,
    lock: u64,
    outputs: Vec<TransactionOutput>,
    cov: Option<Hash>,
    vault_at: usize,
    others: Vec<UtxoEntry>,
}
fn base(st: St, entry: &'static str, signer: Option<Keypair>, outputs: Vec<TransactionOutput>) -> Try {
    Try { st, value: VAL, entry, signer, hash_type: SIG_HASH_ALL, extra: vec![], seq: 0, lock: 0, outputs, cov: Some(COV_A), vault_at: 0, others: vec![] }
}

fn run(t: Try) -> Result<(), TxScriptError> {
    let art = compile(&t.st);
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
        b.push(t.hash_type.to_u8());
        args.push(V::Bytes(b));
    }
    args.extend(t.extra.clone());
    let name = art.contracts.keys().next().unwrap().clone();
    let mut script = encode_contract_entry_sig_script(&art, &name, t.entry, &args).expect("encode entry");
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

// Accepted baselines. Each attack below changes one of these.
fn spend5() -> Try {
    base(locked(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &spent(5))])
}
fn req(amount: u64) -> Try {
    let mut t = base(locked(), "request", Some(spend_key()), vec![cont(VAL - MAX_FEE, &pending(amount))]);
    t.extra = vec![V::Bytes(spk_bytes(&dest())), V::Int(amount as i64)];
    t
}
// Locked state still carrying the finished destination (v2 shape, refused since v3).
fn locked_after() -> St {
    St { dest: spk_hash(&dest()), ..locked() }
}
fn finish_part() -> Try {
    let a = 100 * KAS;
    let mut t = base(pending(a), "finish", None, vec![pay(a, dest()), cont(VAL - a - MAX_FEE, &locked())]);
    t.seq = DELAY;
    t
}
fn finish_all() -> Try {
    let mut t = base(pending(VAL - MAX_FEE), "finish", None, vec![pay(VAL - MAX_FEE, dest())]);
    t.seq = DELAY;
    t
}
fn cancel(st: St) -> Try {
    cancel_with(st, 0)
}
fn cancel_with(st: St, which: usize) -> Try {
    let mut t = base(st, "cancel", Some(stop_keys()[which].clone()), vec![pay(VAL - MAX_FEE, cold())]);
    t.extra = vec![V::Int(which as i64)];
    t
}
fn inherit(st: St) -> Try {
    let mut t = base(st, "inherit", Some(heir_key()), vec![pay(VAL - MAX_FEE, heir())]);
    t.seq = INHERIT;
    t
}
// Inherit from Pending: the committed 100 KAS reach the destination first (M1).
fn inherit_pending() -> Try {
    let a = 100 * KAS;
    let mut t = base(pending(a), "inherit", Some(heir_key()), vec![pay(a, dest()), pay(VAL - a - MAX_FEE, heir())]);
    t.seq = INHERIT;
    t
}
// v5 decline: stop key `which` sends a Pending coin back to Locked, nothing leaves.
fn decline_with(st: St, which: usize) -> Try {
    let out = cont(VAL - MAX_FEE, &declined(&st));
    let mut t = base(st, "decline", Some(stop_keys()[which].clone()), vec![out]);
    t.extra = vec![V::Int(which as i64)];
    t
}
fn decline(st: St) -> Try {
    decline_with(st, 0)
}
// v5 inheritOpen: inherit without a signature, autoHeir must be 1.
fn inherit_open(st: St) -> Try {
    let mut t = base(st, "inheritOpen", None, vec![pay(VAL - MAX_FEE, heir())]);
    t.seq = INHERIT;
    t
}
fn inherit_open_pending() -> Try {
    let a = 100 * KAS;
    let mut t = base(auto_pending(a), "inheritOpen", None, vec![pay(a, dest()), pay(VAL - a - MAX_FEE, heir())]);
    t.seq = INHERIT;
    t
}
// A top-up: one outside coin of 10 KAS pays 9 KAS into the vault and the fee.
fn topup(st: St) -> Try {
    let mut t = base(st.clone(), "topup", Some(spend_key()), vec![cont(VAL + 9 * KAS, &st)]);
    t.others = vec![UtxoEntry::new(10 * KAS, spk(50), 5, false, None)];
    t
}

#[test]
fn accepted_flows() {
    ok("A1 spend under cap", spend5());
    let reset = St { ws: 2_000_000, ..spent(5) };
    let mut t = base(spent(9), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &reset)]);
    t.lock = 2_000_000;
    ok("A2 window reset", t);
    ok("A3a request", req(100 * KAS));
    ok("A3b partial finish", finish_part());
    ok("A4 full finish", finish_all());
    ok("A5a cancel Locked", cancel(locked()));
    ok("A5b cancel Pending", cancel(pending(100 * KAS)));
    let mut t = base(locked(), "cancel", Some(cancel_key()), vec![pay(VAL, cold()), pay(KAS / 2, dest())]);
    t.extra = vec![V::Int(0)];
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("A6 cancel with outside fee input", t);
    let mut t = cancel(locked());
    t.cov = None;
    ok("A7 cancel plain coin, no covenant id", t);
    let mut t = cancel(locked());
    t.outputs.push(pay(KAS / 2, vault_spk(&locked())));
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("N8 cancel with an extra unbound output to the vault's own address (contract accepts; watcher must key on covenant id, M2)", t);
}

#[test]
fn spend_attacks() {
    no("R1 over cap at once", base(locked(), "spend", Some(spend_key()), vec![pay(11 * KAS, dest()), cont(VAL - 11 * KAS, &spent(11))]));
    no("R2 cumulative over cap", base(spent(9), "spend", Some(spend_key()), vec![pay(2 * KAS, dest()), cont(VAL - 2 * KAS, &spent(11))]));
    let forged = St { ws: TIMESTAMP_LOCK as i64, ..spent(5) };
    let mut t = base(spent(9), "spend", Some(spend_key()), vec![pay(5 * KAS, dest()), cont(VAL - 5 * KAS, &forged)]);
    t.lock = TIMESTAMP_LOCK;
    no("R3 timestamp locktime window reset", t);
    let mut t = spend5();
    t.seq = 1;
    no("R4 nonzero sequence", t);
    no("R5a fee hidden with understated state", base(locked(), "spend", Some(spend_key()), vec![pay(KAS, dest()), cont(VAL - 50 * KAS, &spent(5))]));
    no("R5b fee over cap with honest state", base(locked(), "spend", Some(spend_key()), vec![pay(KAS, dest()), cont(VAL - 50 * KAS, &spent(50))]));
    let mut t = base(pending(100 * KAS), "spend", Some(spend_key()), vec![pay(KAS, dest()), cont(VAL - 2 * KAS, &St { dest: spk_hash(&dest()), ..spent(2) })]);
    t.seq = 0;
    no("R6a spend from Pending, state otherwise valid", t);
    let mut t = req(100 * KAS);
    t.st = pending(100 * KAS);
    no("R6b request from Pending", t);
    let mut t = spend5();
    t.signer = Some(thief());
    no("R7a thief signs spend", t);
    let mut t = spend5();
    t.signer = Some(cancel_key());
    no("R8a cancel key signs spend", t);
    let mut t = spend5();
    t.outputs[1] = cont(VAL - 5 * KAS, &locked());
    no("R21a altered state resets counter", t);
    let mut t = spend5();
    t.outputs[1].script_public_key = evil();
    no("R21b continuation to other script", t);
    let mut t = spend5();
    t.outputs[1].covenant = None;
    no("R22 binding stripped", t);
    let mut t = spend5();
    t.outputs[0].covenant = t.outputs[1].covenant;
    no("R23 two covenant outputs", t);
    let mut t = spend5();
    t.cov = None;
    t.outputs[1].covenant = None;
    no("Rx spend from plain coin", t);
}

#[test]
fn boundaries_and_edge_shapes() {
    ok("A8 spend exactly the cap", base(locked(), "spend", Some(spend_key()), vec![pay(CAP - MAX_FEE, dest()), cont(VAL - CAP, &spent(10))]));
    ok("A9 spend of zero outflow: continuation == input, fee 0 (the owner's renew path; restarts the heir clock)", base(locked(), "spend", Some(spend_key()), vec![pay(0, dest()), cont(VAL, &locked())]));
    no("N2 spend one sompi over the cap", base(locked(), "spend", Some(spend_key()), vec![pay(CAP + 1 - MAX_FEE, dest()), cont(VAL - CAP - 1, &St { spent: (CAP + 1) as i64, ..locked() })]));
    ok("A10 request exact maximum", req(VAL - 2 * MAX_FEE));
    let edge = 500_000_000_000_u64;
    let forged = St { ws: edge as i64, ..spent(5) };
    let mut t = base(spent(9), "spend", Some(spend_key()), vec![pay(5 * KAS, dest()), cont(VAL - 5 * KAS, &forged)]);
    t.lock = edge;
    no("R3b locktime exactly at the timestamp boundary", t);
    let mut t = spend5();
    t.outputs.push(pay(1, evil()));
    no("R25 spend with third output", t);
    let mut t = spend5();
    t.outputs[0].covenant = t.outputs[1].covenant;
    t.outputs[1].covenant = None;
    no("R24 binding moved to payee", t);
    no("R26 negative outflow", base(spent(5), "spend", Some(spend_key()), vec![pay(0, dest()), cont(VAL + KAS, &spent(4))]));
}

#[test]
fn pass4_spend_boundaries() {
    // A reset needs lockTime past BOTH windowStart + window and the coin's own DAA + window.
    let both = St { ws: UTXO_DAA as i64, ..spent(9) };
    let edge = UTXO_DAA + WINDOW;
    let reset = |ws: u64| St { ws: ws as i64, ..spent(5) };
    let mut t = base(both.clone(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &reset(edge))]);
    t.lock = edge;
    ok("N3a reset exactly at windowStart + window == inputDaa + window", t);
    let mut t = base(both, "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &reset(edge - 1))]);
    t.lock = edge - 1;
    no("N3b reset one DAA short of both terms", t);
    // Post-topup: the old window has passed but the renewed coin's has not.
    let mut t = base(St { ws: 500_000, ..spent(9) }, "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &reset(1_500_000))]);
    t.lock = 1_500_000;
    no("N3c reset past windowStart's window but short of the coin's window", t);
    let mut t = base(St { ws: 1_200_000, ..spent(9) }, "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &reset(1_900_000))]);
    t.lock = 1_900_000;
    no("N3d reset past the coin's window but short of windowStart's", t);
    // windowStart near i64::MAX (L4): spend errors on the addition, cancel still works, so nothing is frozen.
    let huge = St { ws: i64::MAX - 1, ..locked() };
    no("N4a spend with windowStart = i64::MAX - 1 (overflow error)", base(huge.clone(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &St { spent: (5 * KAS) as i64, ..huge.clone() })]));
    ok("N4b cancel from that state", cancel(huge));
    // Deployer-set counters outside [0, cap]: spend refuses rather than widening the cap.
    no("N12a spend from spentInWindow = -1 taking cap + 1", base(St { spent: -1, ..locked() }, "spend", Some(spend_key()), vec![pay(CAP + 1 - MAX_FEE, dest()), cont(VAL - CAP - 1, &spent(10))]));
    let mut t = base(St { spent: (CAP + 1) as i64, ..locked() }, "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &St { ws: 2_000_000, ..spent(5) })]);
    t.lock = 2_000_000;
    no("N12b window reset from spentInWindow = cap + 1", t);
    // spentInWindow == cap is legal: a zero-outflow renew from a full window is accepted.
    ok("N12c zero-outflow renew from spentInWindow == cap", base(spent(10), "spend", Some(spend_key()), vec![pay(0, dest()), cont(VAL, &spent(10))]));
    // Review 9 finding 2: windowSize <= 0 removes the cap; the client must refuse the dial. These assert
    // today's behaviour. A coin that already spent the cap, created at UTXO_DAA with windowStart = UTXO_DAA:
    // the honest window refuses a second cap spend; windowSize 0 resets once lockTime reaches the coin's
    // own DAA (cap per block); windowSize -1 resets even with lockTime below the coin's DAA (cap per tx).
    let full = |w: i64| St { ws: UTXO_DAA as i64, window: w, ..spent(10) };
    let again = |w: i64, lock: u64| {
        let mut t = base(full(w), "spend", Some(spend_key()), vec![pay(CAP - MAX_FEE, dest()), cont(VAL - CAP, &St { ws: lock as i64, ..full(w) })]);
        t.lock = lock;
        t
    };
    no("N13 honest windowSize: second cap spend one window short", again(WINDOW as i64, UTXO_DAA + WINDOW - 1));
    ok("N13a deployer_foot_gun windowSize 0: second cap spend at lockTime == inputDaa accepted (cap per block)", again(0, UTXO_DAA));
    ok("N13b deployer_foot_gun windowSize -1: second cap spend at lockTime == inputDaa - 1 accepted (cap per tx)", again(-1, UTXO_DAA - 1));
}

#[test]
fn review_findings() {
    // Finding 1 (weaker signature types): review2's S1 matrix covers every entry and type.
    // Finding 2: a request must not leave an unfinishable remainder.
    no("F2a request leaves 2*maxFee: too small to continue, too big to burn", req(VAL - 3 * MAX_FEE));
    no("F2b request leaves minRemainder - 1", req(VAL - 2 * MAX_FEE - MIN_REM + 1));
    ok("F2c request leaves exactly minRemainder", req(VAL - 2 * MAX_FEE - MIN_REM));
    no("F2d spend leaves dust continuation", {
        let mut t = base(locked(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(MIN_REM - 1, &spent(5))]);
        t.value = 5 * KAS + MIN_REM - 1;
        t
    });
    // Boundaries the mutation check said were untested.
    ok("F2e spend leaves exactly minRemainder", {
        let mut t = base(locked(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(MIN_REM, &spent(5))]);
        t.value = 5 * KAS + MIN_REM;
        t
    });
    ok("F2f finish leaves exactly minRemainder", {
        let a = 100 * KAS;
        let mut t = base(pending(a), "finish", None, vec![pay(a, dest()), cont(MIN_REM, &locked())]);
        t.value = a + MIN_REM + MAX_FEE;
        t.seq = DELAY;
        t
    });
    no("F2g finish leaves minRemainder - 1", {
        let a = 100 * KAS;
        let mut t = base(pending(a), "finish", None, vec![pay(a, dest()), cont(MIN_REM - 1, &locked())]);
        t.value = a + MIN_REM - 1 + MAX_FEE;
        t.seq = DELAY;
        t
    });
    // Finding 3: coins without lineage leave only through cancel.
    let mut t = finish_all();
    t.cov = None;
    no("F3a finish on plain coin", t);
    let mut t = finish_part();
    t.cov = None;
    t.outputs[1].covenant = None;
    no("F3b two-output finish on plain coin", t);
}

#[test]
fn request_attacks() {
    let mut t = req(100 * KAS);
    t.outputs[0].value = VAL - 2 * MAX_FEE;
    no("R14 request burns over maxFee", t);
    no("R15 request above balance", req(VAL));
    let mut t = req(100 * KAS);
    t.signer = Some(thief());
    no("R7b thief signs request", t);
    let mut t = req(100 * KAS);
    t.outputs[0] = cont(VAL - MAX_FEE, &St { dest: spk_hash(&evil()), ..pending(100 * KAS) });
    no("Rx request output differs from arguments", t);
    let mut t = req(0);
    t.extra[1] = V::Int(0);
    no("Rx zero amount", t);
    let mut t = req(100 * KAS);
    t.extra[1] = V::Int(-5);
    t.outputs[0] = cont(VAL - MAX_FEE, &St { amount: -5, ..pending(0) });
    no("Rx negative amount", t);
    let mut t = req(100 * KAS);
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("Rx request with outside input", t);
    let mut t = req(100 * KAS);
    t.outputs[0].covenant = None;
    no("Rx request drops covenant binding", t);
    let mut t = req(100 * KAS);
    t.outputs.push(pay(KAS, evil()));
    no("Rx request with extra output", t);
    // Pass 4a: dust amounts and odd destination lengths.
    no("N6a request newAmount = minRemainder - 1 (finish would make an unminable dust output, L3)", req(MIN_REM - 1));
    ok("N6b request newAmount = minRemainder", req(MIN_REM));
    for (name, odd) in [("1-byte", vec![0x20u8]), ("520-byte", vec![0x20u8; 520])] {
        let mut t = req(100 * KAS);
        t.extra[0] = V::Bytes(odd.clone());
        t.outputs[0] = cont(VAL - MAX_FEE, &St { dest: hash32(&odd), ..pending(100 * KAS) });
        no(&format!("N6c request with a {name} newDestSpk (L7)"), t);
    }
    let own = vault_spk(&locked());
    let mut t = req(100 * KAS);
    t.extra[0] = V::Bytes(spk_bytes(&own));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { dest: spk_hash(&own), ..pending(100 * KAS) });
    ok("N6d request to a 37-byte P2SH destination", t);
    // Review 9 finding 6: the destination version byte is not checked. Documents today's behaviour;
    // flips to `no` if the optional version guard lands.
    let v1 = ScriptPublicKey::new(1, dest().script().to_vec().into());
    let mut t = base(locked(), "request", Some(spend_key()), vec![cont(VAL - MAX_FEE, &St { dest: spk_hash(&v1), ..pending(100 * KAS) })]);
    t.extra = vec![V::Bytes(spk_bytes(&v1)), V::Int((100 * KAS) as i64)];
    ok("Q9 request to a 36-byte destination with version 0x0001 accepted (finding 6, no version guard)", t);
}

// Run 4 row H (Engine 17): testnet dials (maxFee 0.05, minRemainder 1), a 2.5 KAS account coin, a ~0.003 fee.
// Requests of 2.49 / 2.48 / 2.45 break `newAmount + maxFee <= outputs[0].value` (the rest after the amount is
// under maxFee), to the account's own vault address and to an ordinary one alike; 1.0 of 2.5 is fine. The two
// boundaries of `leftover <= maxFee || leftover - maxFee >= minRemainder` pin the rule the engine now mirrors.
fn testnet_req(amount: u64, to: &ScriptPublicKey) -> Try {
    let tn = St { max_fee: (5 * KAS / 100) as i64, ..locked() };
    let next = St { stage: 1, dest: spk_hash(to), amount: amount as i64, ..tn.clone() };
    let mut t = base(tn, "request", Some(spend_key()), vec![cont(250 * KAS / 100 - 300_000, &next)]);
    t.value = 250 * KAS / 100;
    t.extra = vec![V::Bytes(spk_bytes(to)), V::Int(amount as i64)];
    t
}

#[test]
fn testnet_request_leftover() {
    let own = vault_spk(&St { max_fee: (5 * KAS / 100) as i64, ..locked() });
    let (out, mf) = (250 * KAS / 100 - 300_000, 5 * KAS / 100);
    for (label, to) in [("own vault", own), ("ordinary", dest())] {
        for hundredths in [249, 248, 245] {
            no(&format!("H {label}: request {hundredths}/100 of 2.5"), testnet_req(hundredths * KAS / 100, &to));
        }
        ok(&format!("H {label}: request 1.0 of 2.5"), testnet_req(KAS, &to));
        ok(&format!("H {label}: rest exactly maxFee"), testnet_req(out - mf, &to));
        no(&format!("H {label}: rest 1 sompi under maxFee"), testnet_req(out - mf + 1, &to));
        ok(&format!("H {label}: rest maxFee + minRemainder"), testnet_req(out - mf - MIN_REM, &to));
        no(&format!("H {label}: rest 1 sompi under maxFee + minRemainder"), testnet_req(out - mf - MIN_REM + 1, &to));
    }
}

#[test]
fn finish_attacks() {
    let mut t = finish_part();
    t.seq = DELAY - 1;
    no("R9 before delay", t);
    let mut t = finish_part();
    t.outputs[0].script_public_key = evil();
    no("R10 wrong destination", t);
    let mut t = finish_part();
    t.outputs[0].value += KAS;
    t.outputs[1].value -= KAS;
    no("R11a more than recorded", t);
    let mut t = finish_part();
    t.outputs[0].value -= KAS;
    no("R11b less than recorded", t);
    let mut t = finish_part();
    t.st = St { stage: 0, ..pending(100 * KAS) };
    no("R12 finish from Locked", t);
    let mut t = finish_part();
    t.outputs.truncate(1);
    no("R13a burn remainder as fee", t);
    let mut t = finish_part();
    t.outputs[1].value -= 1;
    no("R13b fee one over maxFee", t);
    let mut t = finish_part();
    t.outputs[1] = cont(t.outputs[1].value, &pending(100 * KAS));
    no("Rx finish keeps Pending", t);
    let mut t = finish_part();
    t.outputs.push(pay(KAS, evil()));
    no("Rx finish extra output", t);
    let mut t = finish_part();
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("Rx finish with outside input", t);
    let mut t = finish_part();
    t.outputs[1].covenant = None;
    no("Rx finish drops covenant binding", t);
    let mut t = finish_part();
    t.outputs[0].covenant = t.outputs[1].covenant;
    t.outputs[1].covenant = None;
    no("Rx finish moves binding to payee", t);
    let mut t = finish_part();
    t.outputs[1] = cont(t.outputs[1].value, &locked_after());
    no("N10 finish keeping the stale destHash in Locked (L5)", t);
    // Review 9 L1 (v4): the single-output finish ends the lineage; output 0 may not carry the id.
    let a = VAL - MAX_FEE;
    let mut t = base(St { dest: spk_hash(&vault_spk(&locked())), ..pending(a) }, "finish", None, vec![cont(a, &locked())]);
    t.seq = DELAY;
    no("F20 single-output finish to the vault's own Locked spk carrying the covenant id", t);
    let mut t = finish_all();
    t.outputs[0].covenant = Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_A });
    no("F21 single-output finish, destination P2PK (a thief's own key in the attack) carrying the covenant id", t);
    ok("F20c honest single-output finish still accepted", finish_all());
    // F22: a deployer Pending whose remainder sits in the dead zone (maxFee, minRemainder): no finish
    // shape fits with honest values (single: remainder > maxFee; two: remainder < minRemainder). Cancel exits.
    let dead = pending(VAL - KAS / 2);
    let mut t = base(dead.clone(), "finish", None, vec![pay(VAL - KAS / 2, dest())]);
    t.seq = DELAY;
    no("F22a dead-zone remainder, single-output finish", t);
    let mut t = base(dead.clone(), "finish", None, vec![pay(VAL - KAS / 2, dest()), cont(KAS / 2, &locked())]);
    t.seq = DELAY;
    no("F22b dead-zone remainder, two-output finish", t);
    let mut t = cancel(dead);
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("F22c cancel with an outside input exits the dead zone", t);
    // F23: deployer maxFee = -1: no finish shape and no request can satisfy it; cancel with an outside
    // input (cold output one sompi over the coin) exits.
    let neg = |s: St| St { max_fee: -1, ..s };
    let mut t = base(neg(pending(VAL)), "finish", None, vec![pay(VAL, dest())]);
    t.seq = DELAY;
    no("F23a maxFee -1, single-output finish of the whole coin", t);
    let mut t = base(neg(pending(100 * KAS)), "finish", None, vec![pay(100 * KAS, dest()), cont(VAL - 100 * KAS, &neg(locked()))]);
    t.seq = DELAY;
    no("F23b maxFee -1, two-output finish with zero fee", t);
    let mut t = base(neg(locked()), "request", Some(spend_key()), vec![cont(VAL, &neg(pending(100 * KAS)))]);
    t.extra = vec![V::Bytes(spk_bytes(&dest())), V::Int((100 * KAS) as i64)];
    no("F23c maxFee -1, request with zero fee", t);
    let mut t = base(neg(locked()), "cancel", Some(cancel_key()), vec![pay(VAL + 1, cold())]);
    t.extra = vec![V::Int(0)];
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("F23d maxFee -1, cancel with an outside input lifting the cold output by one sompi", t);
}

// Review 9 finding 7: the sighash excludes the signature script, so a relayer may rewrite the
// dispatch tag and the arguments of a signed transaction. Every re-tag of every accepted
// baseline must fail (finish takes no signature, so the sig is dropped for that target).
#[test]
fn review9_retag_matrix() {
    let baselines: Vec<(&str, Try)> = vec![
        ("spend", spend5()),
        ("request", req(100 * KAS)),
        ("topup", topup(locked())),
        ("cancel", cancel(locked())),
        ("inherit", inherit(locked())),
        ("decline", decline(pending(100 * KAS))),
        ("inheritOpen", inherit_open(auto())),
    ];
    let args_for = |entry: &str| -> Vec<V> {
        match entry {
            "request" => vec![V::Bytes(spk_bytes(&dest())), V::Int((100 * KAS) as i64)],
            "cancel" | "decline" => vec![V::Int(0)],
            _ => vec![],
        }
    };
    for (from, t0) in &baselines {
        for to in ["spend", "request", "topup", "cancel", "inherit", "finish", "decline", "inheritOpen"] {
            if *from == to {
                continue;
            }
            let mut t = t0.clone();
            t.entry = to;
            t.extra = args_for(to);
            if to == "finish" || to == "inheritOpen" {
                t.signer = None;
            } else if t.signer.is_none() {
                // The unsigned inheritOpen tx re-tagged to a signed entry: a stranger signs it.
                t.signer = Some(thief());
            }
            no(&format!("M9 {from} tx re-tagged as {to}"), t);
        }
    }
}

#[test]
fn cancel_attacks() {
    let mut t = cancel(locked());
    t.signer = Some(thief());
    no("R7c thief signs cancel", t);
    let mut t = cancel(locked());
    t.signer = Some(spend_key());
    no("R8b spend key signs cancel", t);
    let mut t = cancel(locked());
    t.outputs[0].script_public_key = evil();
    no("R16 cancel elsewhere", t);
    let mut t = cancel(locked());
    t.outputs[0].value -= 1;
    no("R17 cold underpaid", t);
    let mut t = cancel(locked());
    t.outputs = vec![pay(VAL - MAX_FEE, evil()), pay(VAL - MAX_FEE, cold())];
    no("Rx cold in second position", t);
    // a coin smaller than maxFee may not vanish as fee on any exit.
    let mut t = cancel(locked());
    t.value = MAX_FEE / 2;
    t.cov = None;
    t.outputs[0].value = 0;
    no("N5a cancel of a plain coin smaller than maxFee burning it all", t);
    let mut t = cancel(locked());
    t.value = MAX_FEE / 2;
    t.cov = None;
    t.outputs[0].value = MIN_REM;
    t.others = vec![UtxoEntry::new(2 * KAS, spk(50), 5, false, None)];
    ok("N5b the same dust coin lifted to minRemainder by an outside input", t);
    let mut t = inherit(locked());
    t.value = MAX_FEE / 2;
    t.cov = None;
    t.outputs[0].value = 0;
    no("N5c inherit of a plain coin smaller than maxFee burning it all", t);
    let mut t = inherit(locked());
    t.value = MAX_FEE / 2;
    t.cov = None;
    t.outputs[0].value = MIN_REM;
    t.others = vec![UtxoEntry::new(2 * KAS, spk(50), 5, false, None)];
    ok("N5d the same dust coin lifted to exactly minRemainder by an outside input, inherit", t);
}

#[test]
fn multi_input_attacks() {
    let twin = |id| UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(id));
    let mut t = spend5();
    t.vault_at = 1;
    t.outputs[1].covenant = Some(CovenantBinding { authorizing_input: 1, covenant_id: COV_A });
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R18 vault at input 1", t);
    let mut t = spend5();
    t.others = vec![twin(COV_A)];
    no("R19a same-id twin, input 0", t);
    let mut t = spend5();
    t.vault_at = 1;
    t.outputs[1].covenant = Some(CovenantBinding { authorizing_input: 1, covenant_id: COV_A });
    t.others = vec![twin(COV_A)];
    no("R19b same-id twin, input 1", t);
    let mut t = cancel(locked());
    t.vault_at = 1;
    t.others = vec![twin(COV_B)];
    no("R20 second vault sharing a cancel output", t);
    let mut t = spend5();
    t.others = vec![twin(COV_B)];
    no("Rx two ids on spend", t);
}

#[test]
fn three_stop_keys() {
    for which in 0..3 {
        ok(&format!("A8 cancel with stop key {which}"), cancel_with(locked(), which));
        ok(&format!("A8 cancel Pending with stop key {which}"), cancel_with(pending(100 * KAS), which));
        let mut t = cancel_with(locked(), which);
        t.signer = Some(thief());
        no(&format!("R21 thief signs cancel, which {which}"), t);
        let mut t = cancel_with(locked(), which);
        t.signer = Some(stop_keys()[(which + 1) % 3].clone());
        no(&format!("R22 wrong stop key for which {which}"), t);
        let mut t = cancel_with(locked(), which);
        t.signer = Some(heir_key());
        no(&format!("R23 heir signs cancel, which {which}"), t);
    }
    for (which, bad) in [(0usize, 3i64), (0, -1), (2, 3), (2, 7)] {
        let mut t = cancel_with(locked(), which);
        t.extra = vec![V::Int(bad)];
        no(&format!("R24 out-of-range which {bad} signed by stop key {which}"), t);
    }
    // Review 9 finding 7 / review 4 L1: when the unused slot repeats cancelPk the same signature is
    // valid under which 0 and which 1. Watcher attribution only; no money moves differently.
    for which in 0..2usize {
        let mut t = cancel_with(St { k2_same: true, ..locked() }, which);
        t.signer = Some(cancel_key());
        ok(&format!("K9 cancelPk2 == cancelPk: the cancel key's signature accepted under which {which}"), t);
    }
    let mut t = cancel_with(St { k2_same: true, ..locked() }, 2);
    t.signer = Some(cancel_key());
    no("K9b cancelPk2 == cancelPk: the cancel key refused under which 2 (cancelPk3 distinct)", t);
}

#[test]
fn inherit_flows() {
    ok("A9a inherit from Locked", inherit(locked()));
    ok("A9b inherit from Pending pays the destination first, the heir second (M1)", inherit_pending());
    let mut t = inherit(locked());
    t.seq = INHERIT + 5;
    ok("A9c inherit later", t);
    let mut t = inherit(locked());
    t.cov = None;
    ok("A9d inherit a plain coin, no covenant id", t);
    let mut t = base(locked(), "inherit", Some(heir_key()), vec![pay(VAL, heir()), pay(KAS / 2, dest())]);
    t.seq = INHERIT;
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("A9e inherit with outside fee input", t);

    let mut t = inherit(locked());
    t.seq = INHERIT - 1;
    no("R25a inherit one DAA early", t);
    let mut t = inherit(locked());
    t.seq = 0;
    no("R25b inherit with sequence 0", t);
    let mut t = inherit(locked());
    t.seq = DELAY;
    no("R25c inherit after only the withdrawal delay", t);
    for (name, k) in [("thief", thief()), ("spend key", spend_key()), ("cancel key", cancel_key()), ("stop key 3", cancel_key3())] {
        let mut t = inherit(locked());
        t.signer = Some(k);
        no(&format!("R26 {name} signs inherit"), t);
    }
    let mut t = inherit(locked());
    t.outputs[0].script_public_key = evil();
    no("R27a inherit elsewhere", t);
    let mut t = inherit(locked());
    t.outputs[0].script_public_key = cold();
    no("R27b inherit to the cold script", t);
    let mut t = inherit(locked());
    t.outputs = vec![pay(VAL - MAX_FEE, evil()), pay(VAL - MAX_FEE, heir())];
    no("R27c heir in second position", t);
    let mut t = inherit(locked());
    t.outputs[0].value -= 1;
    no("R28 heir underpaid", t);
    let mut t = inherit(locked());
    t.vault_at = 1;
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_B))];
    no("R29 second vault sharing an inherit output", t);
    // from Pending the owner's committed payment comes first.
    no("N7a inherit from Pending skipping the destination (the v2 shape)", inherit(pending(100 * KAS)));
    let mut t = inherit_pending();
    t.outputs[0].value -= 1;
    t.outputs[1].value += 1;
    no("N7b heir shorting the beneficiary by one sompi", t);
    let mut t = inherit_pending();
    t.outputs.swap(0, 1);
    no("N7c beneficiary at output 1, heir at output 0", t);
    let mut t = inherit_pending();
    t.outputs[1].value -= 1;
    no("N7d heir output one sompi under remainder - maxFee", t);
    let mut t = inherit_pending();
    t.outputs[1].script_public_key = evil();
    no("N7e heir output elsewhere from Pending", t);
    let mut t = inherit_pending();
    t.outputs[0].script_public_key = evil();
    no("N7f destination swapped from Pending", t);
    let mut t = inherit_pending();
    t.outputs.truncate(1);
    no("N7g inherit from Pending paying only the destination", t);
    // Review 9 I20: a Pending whose leftover is under minRemainder is a dust vault for the heir alone;
    // an outside input lifting the heir output to minRemainder is the exit.
    let dust = pending(VAL - KAS / 2);
    let mut t = base(dust.clone(), "inherit", Some(heir_key()), vec![pay(VAL - KAS / 2, dest()), pay(KAS / 2 - MAX_FEE, heir())]);
    t.seq = INHERIT;
    no("I20a inherit a dust Pending alone: heir output under minRemainder", t);
    let mut t = base(dust, "inherit", Some(heir_key()), vec![pay(VAL - KAS / 2, dest()), pay(MIN_REM, heir())]);
    t.seq = INHERIT;
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("I20b heir lifts the dust Pending with an outside input to minRemainder", t);
}

#[test]
fn decline_flows() {
    let a = 100 * KAS;
    for which in 0..3 {
        ok(&format!("A11 decline with stop key {which}"), decline_with(pending(a), which));
    }
    ok("A11d decline keeps a non-zero window start and spent counter", decline(pending_in_window(a)));
    ok("A11e decline with the autoHeir dial on", decline(auto_pending(a)));
    // Review 9 dead-zone coin: amount == value - maxFee, so finish can only end the lineage.
    // decline must still take it back to Locked with the whole coin but the fee.
    ok("A11f decline of a Pending whose amount is the whole coin minus maxFee", decline(pending(VAL - MAX_FEE)));

    for (name, k) in [("spend key", spend_key()), ("thief", thief()), ("heir", heir_key())] {
        let mut t = decline(pending(a));
        t.signer = Some(k);
        no(&format!("R40 {name} signs decline"), t);
    }
    let mut t = decline(pending(a));
    t.extra = vec![V::Int(3)];
    no("R41a decline with which 3", t);
    let mut t = decline_with(pending(a), 1);
    t.signer = Some(cancel_key());
    no("R41b decline which 1 signed by stop key 0", t);
    let mut t = decline(pending(a));
    t.hash_type = SIG_HASH_NONE;
    no("R42a decline signed SIGHASH_NONE", t);
    let mut t = decline(pending(a));
    t.hash_type = SIG_HASH_ALL | SIG_HASH_ANY_ONE_CAN_PAY;
    no("R42b decline signed SIGHASH_ALL|ANYONECANPAY", t);
    // Output state attacks: every field decline writes.
    let mut t = decline(pending(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { stage: 0, amount: 0, ..pending(a) });
    no("R43a decline keeping the old destHash", t);
    let mut t = decline(pending(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &pending(a));
    no("R43b decline leaving the state Pending", t);
    let mut t = decline(pending(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { stage: 1, ..locked() });
    no("R43c decline writing stage=1 with a zero destination", t);
    let mut t = decline(pending_in_window(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { spent: 0, ..declined(&pending_in_window(a)) });
    no("R43d decline lowering spentInWindow to 0", t);
    let mut t = decline(pending_in_window(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { ws: 2_000_001, ..declined(&pending_in_window(a)) });
    no("R43e decline moving the window start", t);
    let mut t = decline(pending_in_window(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { spent: (6 * KAS) as i64, ..declined(&pending_in_window(a)) });
    no("R43f decline raising spentInWindow", t);
    let mut t = decline(pending(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &St { amount: a as i64, ..locked() });
    no("R43g decline keeping the amount", t);
    // Output shape attacks.
    let mut t = decline(pending(a));
    t.outputs = vec![cont(VAL - MAX_FEE - KAS, &locked()), pay(KAS, evil())];
    no("R44a decline with a side payment", t);
    let mut t = decline(pending(a));
    t.outputs = vec![pay(VAL - MAX_FEE, evil())];
    no("R44b decline paying out instead of continuing", t);
    let mut t = decline(pending(a));
    t.outputs[0].covenant = None;
    no("R44c decline continuation without the covenant", t);
    let mut t = decline(pending(a));
    t.outputs = vec![pay(VAL - MAX_FEE, evil()), cont(0, &locked())];
    no("R44d decline with the continuation at index 1", t);
    let mut t = decline(pending(a));
    t.outputs[0].value -= 1;
    no("R45a decline one sompi under input - maxFee", t);
    let mut t = decline(pending(a));
    t.value = MIN_REM;
    t.outputs[0].value = MIN_REM - MAX_FEE;
    no("R45b decline of a coin at minRemainder: continuation under minRemainder", t);
    let mut t = decline(pending(a));
    t.value = MIN_REM;
    t.outputs[0].value = MIN_REM;
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R45c decline cannot be lifted by an outside input (solo vault input)", t);
    let mut t = decline(pending(a));
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R46a decline with an extra plain input", t);
    let mut t = decline(pending(a));
    t.vault_at = 1;
    t.outputs[0].covenant = Some(CovenantBinding { authorizing_input: 1, covenant_id: COV_A });
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R46b decline with the vault at input 1", t);
    no("R47a decline replayed on a Locked coin", decline(locked()));
    no("R47b decline replayed on a Locked coin with counters", decline(St { ws: 2_000_000, ..spent(5) }));
    let mut t = decline(pending(a));
    t.cov = None;
    no("R47c decline of a plain coin, no covenant lineage", t);
    let mut t = decline(pending(a));
    t.outputs[0] = cont(VAL - MAX_FEE, &auto());
    no("R47d decline switching the autoHeir dial on", t);
    // Mutation v5, line 237 `require(tx.outputs.length == 1)`: output 0 is the proper continuation at
    // exactly input - maxFee (so both value rules hold) and output 1 leaks half the fee allowance to a
    // stranger. Only the output-count rule refuses it; without it a stop key could leak up to maxFee.
    let mut t = decline(pending(a));
    t.outputs = vec![cont(VAL - MAX_FEE, &declined(&pending(a))), pay(MAX_FEE / 2, evil())];
    no("R44e decline with a second output leaking part of the fee allowance (L237 outputs.length == 1)", t);
    // Mutation v5, line 240 `require(tx.outputs[0].value >= minRemainder)`: exactly minRemainder is
    // allowed (kills the >= to > mutant), one sompi under is refused with the fee rule still satisfied.
    let mut t = decline(pending(a));
    t.value = MIN_REM + MAX_FEE;
    t.outputs[0].value = MIN_REM;
    ok("A11g decline leaving exactly minRemainder (L240 >= minRemainder)", t);
    let mut t = decline(pending(a));
    t.value = MIN_REM - 1 + MAX_FEE;
    t.outputs[0].value = MIN_REM - 1;
    no("R45d decline leaving minRemainder - 1 (L240 >= minRemainder)", t);
}

#[test]
fn inherit_open_flows() {
    ok("A12a inheritOpen from Locked, autoHeir 1", inherit_open(auto()));
    ok("A12b inheritOpen from Pending pays the destination first", inherit_open_pending());
    let mut t = inherit_open(auto());
    t.seq = INHERIT + 5;
    ok("A12c inheritOpen later", t);
    let mut t = inherit_open(auto());
    t.cov = None;
    ok("A12d inheritOpen of a plain coin, no covenant id", t);
    let mut t = base(auto(), "inheritOpen", None, vec![pay(VAL, heir())]);
    t.seq = INHERIT;
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("A12e inheritOpen with an outside fee input", t);
    ok("A12f signed inherit still works with autoHeir 1", inherit(auto()));
    ok("A12g signed inherit still works with autoHeir 0", inherit(locked()));
    ok("A12h signed inherit from Pending with autoHeir 1", {
        let mut t = inherit_pending();
        t.st = auto_pending(100 * KAS);
        t
    });

    let mut t = inherit_open(auto());
    t.seq = INHERIT - 1;
    no("R50a inheritOpen one DAA early", t);
    let mut t = inherit_open(auto());
    t.seq = 0;
    no("R50b inheritOpen with sequence 0", t);
    let mut t = inherit_open(auto());
    t.seq = DELAY;
    no("R50c inheritOpen after only the withdrawal delay", t);
    no("R51a inheritOpen with autoHeir 0", inherit_open(locked()));
    no("R51b inheritOpen with autoHeir 2", inherit_open(St { auto_heir: 2, ..locked() }));
    no("R51c inheritOpen with autoHeir -1", inherit_open(St { auto_heir: -1, ..locked() }));
    let mut t = inherit_open(auto());
    t.outputs[0].script_public_key = evil();
    no("R52a inheritOpen elsewhere", t);
    let mut t = inherit_open(auto());
    t.outputs[0].script_public_key = cold();
    no("R52b inheritOpen to the cold script", t);
    let mut t = inherit_open(auto());
    t.outputs = vec![pay(VAL - MAX_FEE, evil()), pay(VAL - MAX_FEE, heir())];
    no("R52c inheritOpen heir in second position", t);
    let mut t = inherit_open(auto());
    t.outputs[0].value -= 1;
    no("R53a inheritOpen heir underpaid by one sompi", t);
    let mut t = inherit_open(auto());
    t.outputs[0].value = VAL - 2 * MAX_FEE;
    no("R53b inheritOpen heir short by two fees", t);
    let mut t = inherit_open(auto());
    t.outputs = vec![pay(VAL - MAX_FEE - KAS, heir()), cont(KAS, &auto())];
    no("R54a inheritOpen leaving a covenant continuation (lineage not ended)", t);
    let mut t = inherit_open(auto());
    t.outputs = vec![pay(VAL - MAX_FEE - KAS, heir()), cont(KAS, &locked())];
    no("R54b inheritOpen leaving a continuation with autoHeir 0", t);
    // Mutation v5, line 113 `require(stage == 0)` in payHeirAll's else-branch. No entry writes a stage
    // other than 0 or 1, but the constructor takes initStage, so a deployer could mint a stage-2 coin;
    // without the rule inherit/inheritOpen would pay the heir the whole coin, skipping the Pending branch.
    no("R60a inherit from a deployer-minted stage-2 coin (L113 stage == 0)", inherit(St { stage: 2, ..locked() }));
    no("R60b inheritOpen from a deployer-minted stage-2 coin, autoHeir 1 (L113 stage == 0)", inherit_open(St { stage: 2, ..auto() }));
    no("R55a inheritOpen from Pending skipping the destination", inherit_open(auto_pending(100 * KAS)));
    let mut t = inherit_open_pending();
    t.outputs[0].value -= 1;
    t.outputs[1].value += 1;
    no("R55b inheritOpen from Pending shorting the beneficiary by one sompi", t);
    let mut t = inherit_open_pending();
    t.outputs[0].value += 1;
    t.outputs[1].value -= 1;
    no("R55c inheritOpen from Pending overpaying the beneficiary", t);
    let mut t = inherit_open_pending();
    t.outputs.swap(0, 1);
    no("R55d inheritOpen beneficiary at output 1, heir at output 0", t);
    let mut t = inherit_open_pending();
    t.outputs[1].script_public_key = evil();
    no("R55e inheritOpen heir output elsewhere from Pending", t);
    let mut t = inherit_open_pending();
    t.outputs.truncate(1);
    no("R55f inheritOpen from Pending paying only the destination", t);
    let mut t = inherit_open_pending();
    t.st = pending(100 * KAS);
    no("R55g inheritOpen from Pending with autoHeir 0", t);
    let mut t = inherit_open(auto());
    t.value = MAX_FEE / 2;
    t.cov = None;
    t.outputs[0].value = 0;
    no("R56a inheritOpen of a plain coin smaller than maxFee burning it all", t);
    let mut t = inherit_open(auto());
    t.value = MAX_FEE / 2;
    t.cov = None;
    t.outputs[0].value = MIN_REM;
    t.others = vec![UtxoEntry::new(2 * KAS, spk(50), 5, false, None)];
    ok("R56b the same dust coin lifted to minRemainder by an outside input, inheritOpen", t);
    let mut t = inherit_open(auto());
    t.vault_at = 1;
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&auto()), UTXO_DAA, false, Some(COV_B))];
    no("R57 second vault sharing an inheritOpen output", t);
    // Review v5 finding 1: no signer stands behind inheritOpen, so its output count is pinned.
    // A stranger adding an input of his own and a second output to himself worth the vault's
    // maxFee (the heir output still meets owed - maxFee) must be refused.
    let mut t = inherit_open(auto());
    t.outputs = vec![pay(VAL - MAX_FEE, heir()), pay(MAX_FEE + KAS, evil())];
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R58a inheritOpen from Locked with a stranger's input and a second output to himself", t);
    let mut t = inherit_open(auto());
    t.outputs = vec![pay(VAL - MAX_FEE, heir()), pay(MAX_FEE, evil())];
    no("R58b inheritOpen from Locked with a second output skimming maxFee", t);
    let mut t = inherit_open_pending();
    t.outputs.push(pay(MAX_FEE + KAS, evil()));
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R58c inheritOpen from Pending with three outputs", t);
    let mut t = inherit_open(auto());
    t.outputs = vec![pay((VAL - MAX_FEE) / 2, heir()), pay((VAL - MAX_FEE) / 2, heir())];
    no("R58d inheritOpen splitting owed - maxFee over two heir outputs", t);
    let mut t = inherit_open_pending();
    t.outputs = vec![pay(100 * KAS, dest()), pay((VAL - 100 * KAS - MAX_FEE) / 2, heir()), pay((VAL - 100 * KAS - MAX_FEE) / 2, heir())];
    no("R58e inheritOpen from Pending splitting the heir's share over two outputs", t);
    // An outside lift still works: owed - maxFee under minRemainder is refused alone and
    // accepted when an outside coin lifts the single heir output to minRemainder.
    let tiny = MIN_REM + MAX_FEE / 2;
    let mut t = inherit_open(auto());
    t.value = tiny;
    t.cov = None;
    t.outputs[0].value = tiny - MAX_FEE;
    no("R59a inheritOpen of a tiny coin alone: heir output under minRemainder", t);
    let mut t = inherit_open(auto());
    t.value = tiny;
    t.cov = None;
    t.outputs[0].value = MIN_REM;
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    ok("R59b the same tiny coin lifted to minRemainder by an outside input, single heir output", t);
    let mut t = inherit_open(auto());
    t.value = tiny;
    t.cov = None;
    t.outputs = vec![pay(MIN_REM, heir()), pay(KAS - MAX_FEE, evil())];
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R59c the lift with the lifter's change as a second output", t);
    // A lineage-less vault-script coin behind another input: not input 0, refused.
    let mut t = inherit_open(auto());
    t.cov = None;
    t.vault_at = 1;
    t.others = vec![UtxoEntry::new(KAS, spk(50), 5, false, None)];
    no("R60 inheritOpen of a plain vault-script coin at input 1", t);
}

// v6 hardLimit: with the dial on, `request` must fail outright while
// every other entry behaves exactly as on a hardLimit 0 vault.
#[test]
fn hard_limit_flows() {
    // The honest request on a hard vault: same signer, args and continuation shape as A3a,
    // only the dial differs. Must fail on the dial alone (vault.sil `require(hardLimit == 0)`).
    let req_hard = |dial: i64| -> Try {
        let a = 100 * KAS;
        let st = St { hard_limit: dial, ..locked() };
        let next = St { hard_limit: dial, ..pending(a) };
        let mut t = base(st, "request", Some(spend_key()), vec![cont(VAL - MAX_FEE, &next)]);
        t.extra = vec![V::Bytes(spk_bytes(&dest())), V::Int(a as i64)];
        t
    };
    ok("A13 the same request accepted with hardLimit 0", req_hard(0));
    no("R70a request with hardLimit 1", req_hard(1));
    no("R70b request with hardLimit 2", req_hard(2));
    no("R70c request with hardLimit -1", req_hard(-1));
    // Within-limit spend, stop, top-up and inheritance are untouched by the dial.
    let next = St { hard_limit: 1, ..spent(5) };
    let t = base(hard(), "spend", Some(spend_key()), vec![pay(5 * KAS - MAX_FEE, dest()), cont(VAL - 5 * KAS, &next)]);
    ok("A14a spend under cap with hardLimit 1", t);
    ok("A14b cancel Locked with hardLimit 1", cancel(hard()));
    ok("A14c topup with hardLimit 1", topup(hard()));
    ok("A14d inherit after inheritDelay with hardLimit 1", inherit(hard()));
    ok("A14e inheritOpen with hardLimit 1 and autoHeir 1", inherit_open(St { hard_limit: 1, ..auto() }));
    // decline/finish on a hard Pending (engine-only state, see hard_pending): the contract
    // did not change there, so they still pass; on chain no such coin can exist.
    ok("A14f decline on an engine-made hard Pending", decline(hard_pending(100 * KAS)));
    let a = 100 * KAS;
    let mut t = base(hard_pending(a), "finish", None, vec![pay(a, dest()), cont(VAL - a - MAX_FEE, &hard())]);
    t.seq = DELAY;
    ok("A14g finish on an engine-made hard Pending", t);
    // Re-tag matrix (review 9 finding 7 shape): the refused hard request, re-tagged by a
    // relayer to every other entry, must still fail.
    let t0 = req_hard(1);
    for to in ["spend", "topup", "cancel", "inherit", "finish", "decline", "inheritOpen"] {
        let mut t = t0.clone();
        t.entry = to;
        t.extra = match to {
            "cancel" | "decline" => vec![V::Int(0)],
            _ => vec![],
        };
        if to == "finish" || to == "inheritOpen" {
            t.signer = None;
        }
        no(&format!("R71 hardLimit 1 request tx re-tagged as {to}"), t);
    }
}

#[test]
fn topup_flows() {
    ok("A10a topup Locked", topup(locked()));
    ok("A10b topup keeps spent counter", topup(spent(7)));
    let st = St { ws: 2_000_000, ..spent(3) };
    ok("A10c topup keeps window start", topup(st));
    let mut t = topup(locked());
    t.outputs[0].value = VAL;
    ok("A10d topup of nothing, fee paid outside", t);
    let mut t = topup(locked());
    t.others = vec![UtxoEntry::new(5 * KAS, spk(50), 5, false, None), UtxoEntry::new(5 * KAS, spk(51), 6, false, None)];
    ok("A10e topup from two outside coins", t);

    let mut t = topup(locked());
    t.outputs[0].value = VAL - 1;
    no("R30a topup with one sompi leaving", t);
    let mut t = topup(locked());
    t.outputs[0].value = VAL - 5 * KAS;
    no("R30b topup draining 5 KAS", t);
    let mut t = topup(locked());
    t.outputs.push(pay(KAS, dest()));
    no("R30c topup with a side payment", t);
    no("R31 topup in Pending", topup(pending(100 * KAS)));
    // The continuation carries exactly the state topup would write, so only the
    // stage check stands between the spend key and quietly dropping a request.
    let mut t = topup(pending(100 * KAS));
    t.outputs[0] = cont(VAL + 9 * KAS, &St { stage: 0, amount: 0, ..pending(100 * KAS) });
    no("R31b topup in Pending, continuation reset to Locked", t);
    for (name, k) in [("thief", thief()), ("cancel key", cancel_key()), ("heir", heir_key())] {
        let mut t = topup(locked());
        t.signer = Some(k);
        no(&format!("R32 {name} signs topup"), t);
    }
    // R32b, R33, R34a, R34b, R35a dropped: review2's S1, T2, T10, T5b, T11 are the same attacks.
    // Vault A at input 1 tops up vault B (input 0): one output bound to B's
    // lineage holds both balances and A's lineage dies. Only the input-0 rule
    // stops A's script from accepting this.
    let mut t = topup(locked());
    t.vault_at = 1;
    t.value = 5 * KAS;
    t.outputs[0] = TransactionOutput {
        value: VAL + 5 * KAS,
        script_public_key: vault_spk(&locked()),
        covenant: Some(CovenantBinding { authorizing_input: 0, covenant_id: COV_B }),
    };
    t.others = vec![UtxoEntry::new(VAL, vault_spk(&locked()), UTXO_DAA, false, Some(COV_B))];
    no("R34d topup merging vault A into vault B's coin", t);
    let mut t = topup(locked());
    t.cov = None;
    no("R34c topup a plain coin, no covenant id", t);
    let mut t = topup(locked());
    t.outputs[0] = cont(VAL + 9 * KAS, &St { ws: 5, ..locked() });
    no("R35b topup changing the window start", t);
    let mut t = topup(locked());
    t.outputs[0] = cont(VAL + 9 * KAS, &pending(VAL));
    no("R35c topup into Pending", t);
    let mut t = topup(locked());
    t.outputs[0].covenant = None;
    no("R35d topup output without the covenant", t);
    // Review 9 finding 9 (by design): free renewals keep the heir waiting. Three topups of nothing,
    // each fee paid from an outside coin, each restarting the silence clock.
    for n in 1..=3u64 {
        let mut t = topup(locked());
        t.outputs[0].value = VAL;
        t.others = vec![UtxoEntry::new(n * KAS, spk(50 + n as u8), 5 + n, false, None)];
        ok(&format!("T20 free renewal {n} of 3: topup with out0 == in0, outside fee only (heir clock restarts, by design)"), t);
    }
}
