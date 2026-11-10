//! Air-gapped signing. The online device builds an unsigned transaction and hands it over
//! as a file (plus a QR code when it fits on a screen); the offline device, which alone
//! holds the cancel and cold keys (the safe is a P2SH 2-of-3 over three cold sheets; any
//! two present offline sign), shows the human what the request does in bank words with the
//! exact figures, refuses anything odd, waits for `y`, and only then signs, sets the compute
//! budget and hands back the signature the same way. The online device re-checks everything
//! in the script engine before the node ever sees it.
//!
//! The request is a small Borsh struct as hex (only what the offline side cannot derive:
//! the coin, the vault state, the amounts); both sides rebuild the identical transaction
//! from it. Its fingerprint (first 16 hex of SHA-256 over the request bytes) is printed on
//! both screens and travels back inside the response, so an answer can never be matched to
//! a different request. The response is a small JSON envelope.

use crate::contract::{self, *};
use crate::keys::*;
use crate::tx::*;
use crate::network::Network;
use kaspa_addresses::Address;
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::hashing::sighash::SigHashReusedValuesUnsync;
#[cfg(feature = "laptop")]
use kaspa_consensus_core::mass::units::ComputeBudget;
use kaspa_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
#[cfg(feature = "laptop")]
use kaspa_consensus_core::tx::ComputeCommit;
use kaspa_consensus_core::tx::{CovenantBinding, PopulatedTransaction, ScriptPublicKey, Transaction, TransactionInput, TransactionOutpoint, TransactionOutput, UtxoEntry, VerifiableTransaction};
use kaspa_txscript::caches::Cache;
use kaspa_txscript::covenants::CovenantsContext;
use kaspa_txscript::script_builder::ScriptBuilder;
use kaspa_txscript::{EngineCtx, EngineFlags, TxScriptEngine};
use secp256k1::Keypair;
use serde::{Deserialize, Serialize};
use silverscript_abi::ArtifactValue as V;
#[cfg(feature = "laptop")]
use std::io::BufRead;
#[cfg(feature = "laptop")]
use std::path::{Path, PathBuf};

const CANCEL: u8 = 0;
const COLD_SEND: u8 = 1;
/// v5: a stop key sends a waiting withdrawal back to Locked; output 0 is the vault's own continuation.
const DECLINE: u8 = 2;
/// The offline device never signs a fee above this, whatever the request says.
pub const FEE_CEILING: u64 = KAS / 10;
/// ...nor one above this many times what the engine itself measures as needed.
pub const FEE_MULTIPLE: u64 = 2;
pub const DIFFERENT_REQUEST: &str = "This answer belongs to a different request.";
pub const NOTHING_WAITING: &str = "Nothing is waiting: the vault is Locked.";
const VAULT_STATE_MISSING: &str = "a cancel or decline request must carry the vault state";
const NO_LINEAGE: &str = "the coin carries no account lineage (a coin sent straight to the vault address); this device refuses it, use the online cancel";
const ZERO_COIN: &str = "the request spends a coin of 0 KAS";
const COINS_OVERFLOW: &str = "the request's coins add up to more than any amount can hold; refused";
/// A signature as both devices write it: 64 bytes of Schnorr signature and the sighash type.
#[cfg(feature = "laptop")]
const SIG_LEN: usize = 65;
#[cfg(feature = "laptop")]
const BAD_SIGNATURE: &str =
    "the answer carries a signature that is not 65 bytes of hex; make the answer again offline";

/// Input 0 is the coin; a cold-send may spend more cold coins through `extra`. Output 0 pays the
/// cold address (the whole coin for cancel, the change for cold-send) unless a cold-send sweeps
/// everything into the payment (`output0 == 0`, no change output); for a decline (v5) output 0 is
/// the vault's own Locked continuation (`after_decline` of `state`, bound to `covenant_id`), the
/// whole coin but the fee. The first output is the only one the offline side may trim for fee.
#[derive(Clone, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct SignRequest {
    pub kind: u8,
    pub outpoint: TransactionOutpoint,
    pub amount: u64,
    pub daa: u64,
    /// The coin's covenant lineage (cancel: the vault's covenant id).
    pub covenant_id: Option<Hash>,
    /// Vault state, cancel and decline: the offline side derives the vault script from it.
    pub state: Option<State>,
    pub output0: u64,
    /// cold-send only: the payment.
    pub pay: Option<(u64, ScriptPublicKey)>,
    /// cancel and decline: the stop slot (0-based, as the contract counts) whose key signs offline.
    pub which: u8,
    /// cold-send only: further cold coins spent beside the first (smallest-first selection), each
    /// (outpoint, amount, daa). Older request layouts end at `which`; `read_request` still reads them.
    pub extra: Vec<(TransactionOutpoint, u64, u64)>,
    /// The network the online device made this for ("testnet" or "mainnet"); the offline device
    /// refuses a request for another network than its own folder. Requests from before the tag read as empty.
    pub network: String,
    /// The vault's dials (state.json at `fund`): the offline device derives the vault script and judges
    /// the amounts with them. Requests from before the field read as the testnet preset.
    pub dials: Dials,
    /// The fee floor the online device measured (sompi per gram): the offline device prices the move
    /// with it instead of its own guess, never below its own floor. Requests from before the field read as the testnet floor.
    pub fee_rate: u64,
}

/// Request shape before the dials and the fee floor (mainnet wave): still read, with the testnet preset.
#[derive(borsh::BorshDeserialize)]
#[cfg(feature = "laptop")]
struct SignRequestV3 {
    kind: u8,
    outpoint: TransactionOutpoint,
    amount: u64,
    daa: u64,
    covenant_id: Option<Hash>,
    state: Option<State>,
    output0: u64,
    pay: Option<(u64, ScriptPublicKey)>,
    which: u8,
    extra: Vec<(TransactionOutpoint, u64, u64)>,
    network: String,
}

/// The request layout of pass 10 to the network tag (no `network`), read as itself.
#[derive(borsh::BorshDeserialize)]
#[cfg(feature = "laptop")]
struct SignRequestV2 {
    kind: u8,
    outpoint: TransactionOutpoint,
    amount: u64,
    daa: u64,
    covenant_id: Option<Hash>,
    state: Option<State>,
    output0: u64,
    pay: Option<(u64, ScriptPublicKey)>,
    which: u8,
    extra: Vec<(TransactionOutpoint, u64, u64)>,
}

/// Small enough for one QR code: the online side already has the transaction, so only the
/// signature(s), the compute budget and the (possibly fee-trimmed) first output value come back,
/// bound to the request by its fingerprint. `signatures` holds one entry for a cancel or a legacy
/// safe, two (in redeem-key order) for a 2-of-3 cold-send.
#[derive(Serialize, Deserialize)]
pub struct SignResponse {
    pub kind: String,
    pub txid: String,
    /// Fingerprint of the request this answers (first 16 hex of SHA-256 over the request bytes).
    pub request: String,
    pub signatures: Vec<String>,
    pub budget: u16,
    pub output0_value: u64,
    /// One signature set and budget per further input (a cold-send of several coins); absent in the older layout.
    #[serde(default)]
    pub more: Vec<InputSigs>,
}

/// The signature(s) and compute budget of one further input.
#[derive(Serialize, Deserialize)]
pub struct InputSigs {
    pub signatures: Vec<String>,
    pub budget: u16,
}

/// First 16 hex of SHA-256 over the request bytes (64 bits). It ties the answer to the request and
/// is compared by eye across two screens; 8 hex (32 bits) could be ground to a match in minutes.
/// The human check that matters is the address and amount the offline screen prints, not this.
pub fn fingerprint(request_bytes: &[u8]) -> String {
    sha256_hex(request_bytes)[..16].to_string()
}

/// The first `n` data pushes of a script: the signature(s) at the front of a signature script we built ourselves.
#[cfg(feature = "laptop")]
fn first_pushes(script: &[u8], n: usize) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let mut i = 0;
    for _ in 0..n {
        let len = script[i] as usize;
        assert!((1..=0x4b).contains(&len), "unexpected push at {i}");
        out.push(script[i + 1..i + 1 + len].to_vec());
        i += 1 + len;
    }
    out
}
#[cfg(feature = "laptop")]
fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not what this command reads: {e}", path.display()))
}
/// The request and the exact bytes it was fingerprinted over.
#[cfg(feature = "laptop")]
fn read_request(path: &Path) -> Result<(SignRequest, Vec<u8>), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let hex = text.trim();
    if hex.is_empty() || hex.len() % 2 != 0 {
        return Err(format!("{} does not hold a request (odd or empty hex)", path.display()));
    }
    let mut bytes = vec![0u8; hex.len() / 2];
    faster_hex::hex_decode(hex.as_bytes(), &mut bytes).map_err(|_| format!("{} does not hold a request (not hex)", path.display()))?;
    let req: SignRequest = match borsh::from_slice::<SignRequest>(&bytes) {
        Ok(r) => r,
        Err(e) => match borsh::from_slice::<SignRequestV3>(&bytes) {
            Ok(o) => {
                println!("old request file ({}, written before the dials); read with the testnet dials and fee floor", path.display());
                SignRequest { kind: o.kind, outpoint: o.outpoint, amount: o.amount, daa: o.daa, covenant_id: o.covenant_id, state: o.state, output0: o.output0, pay: o.pay, which: o.which, extra: o.extra, network: o.network, dials: Dials::testnet(), fee_rate: crate::network::TESTNET_FEE_FLOOR }
            }
            Err(_) => match borsh::from_slice::<SignRequestV2>(&bytes) {
            Ok(o) => {
                println!("old request file ({}, written before the network tag); read as such", path.display());
                SignRequest { kind: o.kind, outpoint: o.outpoint, amount: o.amount, daa: o.daa, covenant_id: o.covenant_id, state: o.state, output0: o.output0, pay: o.pay, which: o.which, extra: o.extra, network: String::new(), dials: Dials::testnet(), fee_rate: crate::network::TESTNET_FEE_FLOOR }
            }
            // The older one-coin layout is no longer read: a request that old is stale anyway.
            Err(_) => return Err(format!("{} does not hold a request this program understands ({e}). Make a new one with `cancel --airgap`, `decline --airgap` or `cold-send --airgap`.", path.display())),
            },
        },
    };
    req.name()?;
    Ok((req, bytes))
}
#[cfg(feature = "laptop")]
fn airgap_dir(ctx: &Ctx) -> PathBuf {
    ctx.dir.join("airgap")
}
/// A stale request or answer must never be signed or sent by mistake: both go before anything new is written.
#[cfg(feature = "laptop")]
fn clear_airgap(ctx: &Ctx) {
    let dir = airgap_dir(ctx);
    for name in ["request.hex", "response.json"] {
        if std::fs::remove_file(dir.join(name)).is_ok() {
            println!("removed the old airgap/{name}");
        }
    }
}

/// Prints the payload as a QR code when it would still be readable on a screen.
#[cfg(feature = "laptop")]
pub fn show_qr(label: &str, payload: &str) {
    match qrcode::QrCode::new(payload.as_bytes()) {
        Ok(code) if code.width() <= 125 => {
            let art = code.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build();
            println!("{label}: {} bytes, QR {} modules wide\n{art}", payload.len(), code.width());
        }
        Ok(code) => println!("{label}: {} bytes would need a {}-module QR, too dense for a screen; move the file instead", payload.len(), code.width()),
        Err(e) => println!("{label}: too big for one QR code ({e}); move the file instead"),
    }
}

/// The address a script pays, or a refusal: the offline device signs only to addresses a person can read.
fn readable_address(net: &Network, spk: &ScriptPublicKey) -> Result<Address, String> {
    net.address_of(spk)
        .ok_or_else(|| format!("the destination is not an address a person can read (script {}); refused", faster_hex::hex_string(spk.script())))
}

impl SignRequest {
    fn name(&self) -> Result<&'static str, String> {
        match self.kind {
            CANCEL => Ok("cancel"),
            COLD_SEND => Ok("cold-send"),
            DECLINE => Ok("decline"),
            k => Err(format!("unknown request kind {k}")),
        }
    }
    /// The unsigned transaction and its coin, rebuilt the same way on both devices.
    fn build(&self, ctx: &Ctx) -> Result<(Transaction, Vec<UtxoEntry>), String> {
        let spk = match self.kind {
            CANCEL | DECLINE => contract::spk_of(&ctx.try_compile_with(&self.dials, self.state.as_ref().ok_or(VAULT_STATE_MISSING)?)?),
            _ => ctx.cold_spk(),
        };
        let mut entries = vec![UtxoEntry::new(self.amount, spk.clone(), self.daa, false, self.covenant_id)];
        let mut inputs = vec![TransactionInput::new_with_compute_budget(self.outpoint, vec![], 0, 0)];
        for (op, amount, daa) in &self.extra {
            if self.kind != COLD_SEND {
                return Err("only a cold-send may spend several coins".into());
            }
            entries.push(UtxoEntry::new(*amount, spk.clone(), *daa, false, None));
            inputs.push(TransactionInput::new_with_compute_budget(*op, vec![], 0, 0));
        }
        // Output 0 is the change back to the cold safe; a cold-send sweep (output0 == 0) has none.
        // A decline's output 0 is the vault's Locked continuation on the same lineage.
        let mut outputs = vec![];
        if self.kind == DECLINE {
            let next = after_decline(self.state.as_ref().ok_or(VAULT_STATE_MISSING)?);
            let id = self.covenant_id.ok_or(NO_LINEAGE)?;
            outputs.push(TransactionOutput::with_covenant(self.output0, ctx.try_vault_spk(&next)?, Some(CovenantBinding::new(0, id))));
        } else if !self.is_sweep() {
            outputs.push(TransactionOutput::new(self.output0, ctx.cold_spk()));
        }
        if let Some((value, to)) = &self.pay {
            outputs.push(TransactionOutput::new(*value, to.clone()));
        }
        let mut tx = Transaction::new(1, inputs, outputs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
        tx.finalize();
        Ok((tx, entries))
    }
    /// A cold-send that spends its coins in full: the change was too small to keep and went with the payment.
    fn is_sweep(&self) -> bool {
        self.kind == COLD_SEND && self.output0 == 0 && self.pay.is_some()
    }
    /// What the inputs hold together. Every coin must hold something (the mass rules divide by each
    /// amount) and the sum must fit: a malformed request is refused in words, never a panic.
    fn in_total(&self) -> Result<u64, String> {
        let amounts = std::iter::once(self.amount).chain(self.extra.iter().map(|(_, a, _)| *a));
        let mut total = 0u64;
        for a in amounts {
            if a == 0 {
                return Err(ZERO_COIN.into());
            }
            total = total.checked_add(a).ok_or(COINS_OVERFLOW)?;
        }
        Ok(total)
    }
    /// How many signatures each input needs: two for a 2-of-3 cold-send, else one.
    fn sig_count(&self, ctx: &Ctx) -> usize {
        if self.kind == COLD_SEND && !ctx.keys.legacy_safe() { 2 } else { 1 }
    }
    /// Signature script for input 0 around the signature(s). `cancel` takes the signature and
    /// the request's stop slot `which`; a 2-of-3 cold-send takes two signatures in key order, then
    /// the redeem script; a legacy single-key safe takes one signature.
    fn sigscript(&self, ctx: &Ctx, sigs: &[Vec<u8>]) -> Vec<u8> {
        assert_eq!(sigs.len(), self.sig_count(ctx), "wrong number of signatures");
        match self.kind {
            CANCEL | DECLINE => ctx.vault_sigscript(&ctx.compile_with(&self.dials, self.state.as_ref().unwrap()), self.name().unwrap(), &[V::Bytes(sigs[0].clone()), V::Int(self.which as i64)]),
            _ => match ctx.keys.public().cold_redeem() {
                Some(redeem) => multisig_sigscript(sigs, &redeem),
                None => ScriptBuilder::new().add_data(&sigs[0]).unwrap().drain(),
            },
        }
    }
    /// Sizes the fee with stand-in signatures of the right length (the real ones come from offline).
    /// `total` is what the sized output gets before fee: output 0 (the change), or with `sweep` the
    /// payment itself (then there is no change output). Returns the sized request and its fee. The
    /// ceiling is the contract's maxFee for a cancel, else what the offline device ever signs.
    fn sized(mut self, ctx: &Ctx, total: u64, sweep: bool) -> Result<(SignRequest, u64), String> {
        let after_fee = |fee: u64| {
            total.checked_sub(fee).ok_or_else(|| {
                format!("fee {fee} sompi is more than the {total} sompi left after the payment: the cold coin is too small (storage mass); fund the safe with a bigger coin or send less")
            })
        };
        let place = |r: &mut SignRequest, value: u64| {
            if sweep {
                r.pay.as_mut().expect("a sweep is a payment").0 = value;
                r.output0 = 0;
            } else {
                r.output0 = value;
            }
        };
        let entries = self.build(ctx)?.1;
        let build = |fee: u64| -> Result<Transaction, String> {
            let mut r = self.clone();
            place(&mut r, after_fee(fee)?);
            let (mut tx, _) = r.build(ctx)?;
            let stand_in = r.sigscript(ctx, &vec![vec![0u8; 65]; r.sig_count(ctx)]);
            for input in &mut tx.inputs {
                input.signature_script = stand_in.clone();
            }
            tx.finalize();
            Ok(tx)
        };
        let ceiling = if matches!(self.kind, CANCEL | DECLINE) { self.dials.max_fee as u64 } else { FEE_CEILING };
        let (_, fee) = ctx.try_with_fee(&entries, Some(ceiling), &build).map_err(|e| match e {
            FeeError::OverCeiling { need, ceiling } => format!(
                "the network fee for this would be {:.5} KAS, more than the {:.2} KAS this move allows. Small coins cost more to move than they hold (storage mass): add a bigger coin to the safe so this one can ride along with it, or wait for a quieter network.",
                kas(need),
                kas(ceiling)
            ),
            e => e.to_string(),
        })?;
        place(&mut self, after_fee(fee)?);
        Ok((self, fee))
    }
    /// Writes airgap/request.hex and its QR, and prints the fingerprint the response must carry.
    #[cfg(feature = "laptop")]
    fn write(&self, ctx: &Ctx, note: &str) -> Result<String, String> {
        let dir = airgap_dir(ctx);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join("request.hex");
        let bytes = borsh::to_vec(self).unwrap();
        let fp = fingerprint(&bytes);
        // Upper-case hex fits the QR alphanumeric mode.
        let payload = faster_hex::hex_string(&bytes).to_ascii_uppercase();
        std::fs::write(&path, &payload).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{note}\nunsigned request written to {} ({} bytes)", path.display(), payload.len());
        println!("Request fingerprint: {fp}  (the offline screen must show the same, and the answer will carry it; what you must read there is the address and the amount)");
        show_qr("Signing request", &payload);
        println!("Next, on the offline device: run `sign <request.hex>` with the same --network");
        Ok(fp)
    }
}

impl Ctx {
    /// Runs every input through the script engine under its committed budget, as the node will.
    fn verify(&self, tx: &Transaction, entries: &[UtxoEntry]) -> Result<(), String> {
        let cache = Cache::new(1_000);
        let reused = SigHashReusedValuesUnsync::new();
        let pop = PopulatedTransaction::new(tx, entries.to_vec());
        let cov = CovenantsContext::from_tx(&pop).map_err(|e| format!("covenant context: {e:?}"))?;
        for (i, input) in tx.inputs.iter().enumerate() {
            let mut vm = TxScriptEngine::from_transaction_input_with_script_units_limit(
                &pop,
                input,
                i,
                pop.utxo(i).unwrap(),
                EngineCtx::new(&cache).with_reused(&reused).with_covenants_ctx(&cov),
                EngineFlags::default(),
                input.compute_commit.allowed_script_units(),
            );
            vm.execute().map_err(|e| format!("signed transaction fails the engine on input {i}: {e:?}"))?;
        }
        println!("engine check passed for all {} inputs under their committed budgets", tx.inputs.len());
        Ok(())
    }
}

/// Online: build the unsigned cancel with stop key `which` (1, 2 or 3) and write airgap/request.hex for the offline device.
#[cfg(feature = "laptop")]
pub async fn prepare_cancel(ctx: &Ctx, which: u8) -> Result<(), String> {
    clear_airgap(ctx);
    let slot = stop_slot(which)?;
    if ctx.keys.stop_secret(slot).is_some() {
        return Err(format!("stop key {which} is on this device; air-gapped signing is pointless until you run split-keys"));
    }
    let v = ctx.try_load_vault()?.ok_or("no vault; run fund")?;
    let coin = ctx.try_vault_coin(&v).await?;
    if coin.1.is_coinbase {
        return Err("the vault coin is a coinbase; wait for it to mature".into());
    }
    let req = SignRequest {
        kind: CANCEL,
        outpoint: coin.0,
        amount: coin.1.amount,
        daa: coin.1.block_daa_score,
        covenant_id: coin.1.covenant_id,
        state: Some(v.state.clone()),
        output0: 0,
        pay: None,
        which: slot as u8,
        extra: vec![],
        network: ctx.network.name.into(),
        dials: ctx.dials()?,
        fee_rate: fee_rate(),
    };
    let (req, fee) = req.sized(ctx, coin.1.amount, false)?;
    let note = format!(
        "cancel (stop key {which}): {:.4} KAS from vault {} to cold {}, fee {:.5} KAS",
        kas(coin.1.amount - fee),
        ctx.vault_address(&v.state),
        ctx.keys.public().cold_address,
        kas(fee)
    );
    req.write(ctx, &note)?;
    Ok(())
}

/// Online: build the unsigned decline (v5) with stop key `which` (1, 2 or 3) and write airgap/request.hex
/// for the offline device. One output: the vault's own Locked continuation, the whole coin but the fee.
#[cfg(feature = "laptop")]
pub async fn prepare_decline(ctx: &Ctx, which: u8) -> Result<(), String> {
    clear_airgap(ctx);
    let slot = stop_slot(which)?;
    if ctx.keys.stop_secret(slot).is_some() {
        return Err(format!("stop key {which} is on this device; air-gapped signing is pointless until you run split-keys"));
    }
    let v = ctx.try_load_vault()?.ok_or("no vault; run fund")?;
    if v.state.stage != 1 {
        return Err(NOTHING_WAITING.into());
    }
    let coin = ctx.try_vault_coin(&v).await?;
    let req = SignRequest {
        kind: DECLINE,
        outpoint: coin.0,
        amount: coin.1.amount,
        daa: coin.1.block_daa_score,
        covenant_id: Some(coin.1.covenant_id.ok_or(NO_LINEAGE)?),
        state: Some(v.state.clone()),
        output0: 0,
        pay: None,
        which: slot as u8,
        extra: vec![],
        network: ctx.network.name.into(),
        dials: ctx.dials()?,
        fee_rate: fee_rate(),
    };
    let (req, fee) = req.sized(ctx, coin.1.amount, false)?;
    if req.output0 < req.dials.min_remainder as u64 {
        return Err(format!(
            "declining now would leave {:.5} KAS in the vault, less than the {} KAS it must keep. Cancel to move everything to your safe.",
            kas(req.output0),
            kas(req.dials.min_remainder as u64)
        ));
    }
    let note = format!(
        "decline (stop key {which}): vault {} back to Locked, the request for {:.4} KAS to {} is withdrawn, {:.4} KAS stays, fee {:.5} KAS",
        ctx.vault_address(&v.state),
        kas(v.state.amount as u64),
        v.pending_to.as_deref().unwrap_or("an unrecorded address"),
        kas(req.output0),
        kas(fee)
    );
    req.write(ctx, &note)?;
    Ok(())
}

/// Online: pay `amount_kas` from the cold safe (P2SH 2-of-3, or a legacy pay-to-pubkey, keys
/// offline) to `to`, change back to cold, as an unsigned request for the offline device.
#[cfg(feature = "laptop")]
pub async fn prepare_cold_send(ctx: &Ctx, amount_kas: f64, to: &str) -> Result<(), String> {
    clear_airgap(ctx);
    if ctx.keys.public().cold_external {
        return Err(COLD_EXTERNAL.into());
    }
    if !ctx.keys.cold_keys().is_empty() {
        return Err("a cold key is on this device; air-gapped signing is pointless until you run split-keys".into());
    }
    let amount = sompi(amount_kas);
    let to_spk = ctx.network.try_to_spk(to)?;
    let cold = ctx.network.address(&ctx.keys.public().cold_address)?;
    let coins = cold_coins(ctx).await?;
    let (req, fee) = cold_send_request(ctx, coins, Some(amount), to_spk)?;
    let ins: Vec<String> = std::iter::once((req.outpoint, req.amount))
        .chain(req.extra.iter().map(|(op, a, _)| (*op, *a)))
        .map(|(op, a)| format!("{}:{} ({:.5} KAS)", op.transaction_id, op.index, kas(a)))
        .collect();
    let change = if req.is_sweep() { "no change (the coins are spent in full)".to_string() } else { format!("change {:.5} KAS back to cold", kas(req.output0)) };
    let note = format!(
        "cold-send: {:.5} KAS from cold {cold} to {to}, {change}, fee {:.5} KAS\ninputs ({}): {}",
        kas(req.pay.as_ref().map_or(0, |p| p.0)),
        kas(fee),
        ins.len(),
        ins.join(", ")
    );
    req.write(ctx, &note)?;
    Ok(())
}

/// The spendable coins at the cold safe (no coinbase coins), for a cold-send or a take-out.
async fn cold_coins(ctx: &Ctx) -> Result<Vec<Coin>, String> {
    let cold = ctx.network.address(&ctx.keys.public().cold_address)?;
    Ok(ctx.try_utxos(&cold).await?.into_iter().filter(|(_, u)| !u.is_coinbase).collect())
}

/// The one builder of a spend from the safe, for the laptop's `cold-send --airgap` and the phone's `takeout`
/// alike: `Some(amount)` picks coins smallest first with change back to the safe (`choose_cold_coins`);
/// `None` is everything the safe holds in one payment less the fee (every coin, no change: a sweep).
pub(crate) fn cold_send_request(ctx: &Ctx, coins: Vec<Coin>, amount: Option<u64>, to_spk: ScriptPublicKey) -> Result<(SignRequest, u64), String> {
    match amount {
        Some(amount) => choose_cold_coins(ctx, coins, amount, to_spk),
        None => sweep_cold_coins(ctx, coins, to_spk),
    }
}

/// Every cold coin into one payment, fee taken from it (`SignRequest::sized` as a sweep). Smallest first
/// keeps the order the airgap path shows.
fn sweep_cold_coins(ctx: &Ctx, mut coins: Vec<Coin>, to_spk: ScriptPublicKey) -> Result<(SignRequest, u64), String> {
    if coins.is_empty() {
        return Err("no mature coins at the cold address".into());
    }
    coins.sort_by_key(|(_, u)| u.amount);
    let total: u64 = coins.iter().map(|(_, u)| u.amount).sum();
    let (first, rest) = coins.split_first().expect("at least one coin");
    let req = SignRequest {
        kind: COLD_SEND,
        which: 0,
        outpoint: first.0,
        amount: first.1.amount,
        daa: first.1.block_daa_score,
        covenant_id: first.1.covenant_id,
        state: None,
        output0: 0,
        pay: Some((total, to_spk)),
        extra: rest.iter().map(|(op, u)| (*op, u.amount, u.block_daa_score)).collect(),
        network: ctx.network.name.into(),
        dials: ctx.dials()?,
        fee_rate: fee_rate(),
    };
    req.sized(ctx, total, true).map_err(|e| format!("the safe holds {:.5} KAS; {e}", kas(total)))
}

/// "Update now": the account's stop has not shown up in the safe yet (the page waits a moment and asks again).
pub const STOP_NOT_IN_SAFE_YET: &str = "The account's funds have not reached your vault yet. Trying again in a moment.";

/// The phone's take-out (mainnet): spend from the safe on this device, with two of its three
/// cold sheets held in `ctx.keys` (cold/cold2/cold3 at the slots the rule block's `cold_x` names; the caller
/// matched them). Builds the request the way `cold-send --airgap` does (`cold_send_request`), signs and
/// budgets it through the same `check` the offline device runs (two signatures in redeem-key order, then the
/// redeem script), proves it in the engine and submits it. `amount` None = everything less the fee; returns
/// (tx id, what reached `to` in sompi, `to`). Nothing is written to the airgap folder. `only_tx` limits the
/// coins to the outputs of that one transaction (the account's own stop, for "Update now"): the rest of the safe stays.
pub async fn takeout(ctx: &Ctx, amount: Option<u64>, to: &str, only_tx: Option<&str>) -> Result<(String, u64, String), String> {
    let present = ctx.keys.cold_keys();
    if present.len() != 2 {
        return Err(format!("a take-out needs exactly two cold sheets here; found {}", present.len()));
    }
    let to_spk = ctx.network.try_to_spk(to)?;
    let mut coins = cold_coins(ctx).await?;
    if let Some(tx) = only_tx {
        coins.retain(|(o, _)| o.transaction_id.to_string() == tx);
        if coins.is_empty() {
            return Err(STOP_NOT_IN_SAFE_YET.into());
        }
    }
    let (req, _) = cold_send_request(ctx, coins, amount, to_spk)?;
    let c = sign_here(ctx, &req)?;
    ctx.verify(&c.tx, &c.entries)?;
    let paid = c.tx.outputs.last().map_or(0, |o| o.value);
    let id = ctx.try_submit(&c.tx, "takeout (two sheets on this device)").await?;
    Ok((id, paid, to.to_string()))
}

/// Signing Keypair copies, erased when this drops: on a refusal inside `check`, on "Not signed", and
/// after the answer is written.
struct Erased(Vec<Keypair>);

impl Erased {
    fn wipe(&mut self) {
        for k in &mut self.0 {
            k.non_secure_erase();
        }
    }
}

impl Drop for Erased {
    fn drop(&mut self) {
        self.wipe();
    }
}

/// The two cold keys that sign and their sheet numbers: the first two present, or the two `sheets` names.
/// Every Keypair in `present` is erased here; only the two copies that sign leave, inside `Erased`.
fn two_cold(mut present: Vec<(usize, Keypair)>, sheets: &Option<Vec<usize>>) -> Result<(Vec<String>, Erased), String> {
    let missing = sheets.iter().flatten().find(|s| !present.iter().any(|(n, _)| n == *s)).copied();
    let mut keep = Erased(vec![]);
    let mut names = vec![];
    for (n, k) in present.iter_mut() {
        let wanted = sheets.as_ref().is_none_or(|w| w.contains(n));
        if missing.is_none() && wanted && keep.0.len() < 2 {
            names.push(n.to_string());
            keep.0.push(*k);
        }
        k.non_secure_erase();
    }
    if let Some(s) = missing {
        return Err(format!("cold sheet {s} is not on this device"));
    }
    if keep.0.len() < 2 {
        return Err(format!("Taking money out of the vault needs two of its three seeds on this device; found {}.", keep.0.len()));
    }
    Ok((names, keep))
}

/// `check` for a request made on this same device (no file, no fingerprint to compare by eye): the signed,
/// budgeted transaction and its coins; the signing keys erase themselves when the answer drops.
fn sign_here(ctx: &Ctx, req: &SignRequest) -> Result<Checked, String> {
    let fp = fingerprint(&borsh::to_vec(req).unwrap());
    let c = check(ctx, req, &fp, None)?;
    for l in &c.lines {
        println!("{l}");
    }
    Ok(c)
}

/// Picks the cold coins for a payment of `amount`: smallest first, so a coin too small to move alone
/// rides along with a bigger one, adding coins until the payment, the measured fee and a change of at
/// least CHANGE_FLOOR fit. When the last coin still leaves change under the floor: change of at most
/// maxFee is folded into the payment (a sweep, said out loud); more than that is refused with the
/// amount that would empty the safe. Returns the sized request and its fee.
fn choose_cold_coins(ctx: &Ctx, mut coins: Vec<Coin>, amount: u64, to_spk: ScriptPublicKey) -> Result<(SignRequest, u64), String> {
    if amount == 0 {
        return Err("a cold-send needs an amount above zero".into());
    }
    if coins.is_empty() {
        return Err("no mature coins at the cold address".into());
    }
    coins.sort_by_key(|(_, u)| u.amount);
    let all: u64 = coins.iter().map(|(_, u)| u.amount).sum();
    let mut total = 0u64;
    for taken in 1..=coins.len() {
        total += coins[taken - 1].1.amount;
        let more = taken < coins.len();
        if total <= amount {
            continue;
        }
        let (first, rest) = coins[..taken].split_first().expect("at least one coin");
        let req = SignRequest {
            kind: COLD_SEND,
            which: 0,
            outpoint: first.0,
            amount: first.1.amount,
            daa: first.1.block_daa_score,
            covenant_id: first.1.covenant_id,
            state: None,
            output0: 0,
            pay: Some((amount, to_spk.clone())),
            extra: rest.iter().map(|(op, u)| (*op, u.amount, u.block_daa_score)).collect(),
            network: ctx.network.name.into(),
            dials: ctx.dials()?,
            fee_rate: fee_rate(),
        };
        // Change (before fee) of at most maxFee is a crumb: too small to keep as a coin, and a change output
        // that small has a storage mass no ceiling allows, so it goes with the payment (a sweep) instead.
        let crumb = total - amount <= ctx.dials()?.max_fee as u64;
        let sweep = |req: SignRequest| -> Result<(SignRequest, u64), String> {
            let (req, fee) = req.sized(ctx, total, true)?;
            let paying = req.pay.as_ref().map_or(0, |p| p.0);
            if paying < amount {
                return Err(format!("the cold safe holds {:.5} KAS, too little to send {:.5} KAS and pay the {:.5} KAS network fee", kas(total), kas(amount), kas(fee)));
            }
            println!("Change of {:.5} KAS is too small to keep as a coin; it was added to the payment, which is now {:.5} KAS.", kas(paying - amount), kas(paying));
            Ok((req, fee))
        };
        let (req, fee) = match req.clone().sized(ctx, total - amount, false) {
            Ok(x) => x,
            Err(_) if more => continue, // these coins cannot pay the fee yet; add the next
            Err(_) if crumb => return sweep(req),
            Err(e) => return Err(e),
        };
        let change = total - amount - fee;
        if change >= CHANGE_FLOOR {
            if taken > 1 {
                println!("{} cold coins go together: the smaller one(s) could not move on their own", taken);
            }
            return Ok((req, fee));
        }
        if more {
            continue; // the next coin may lift the change over the floor
        }
        if crumb {
            return sweep(req);
        }
        return Err(format!(
            "after this payment only {:.5} KAS would come back to the safe, too little to keep as a coin (the safe keeps coins of at least {} KAS). Send about {:.5} KAS to empty the safe, or send less so more comes back.",
            kas(change),
            kas(CHANGE_FLOOR),
            kas(total - fee)
        ));
    }
    Err(format!("the cold safe holds {:.5} KAS in {} coin(s), too little to send {:.5} KAS and pay the network fee", kas(all), coins.len(), kas(amount)))
}

/// Everything the offline screen shows, and the transaction (signed in memory, written nowhere yet) behind it.
struct Checked {
    tx: Transaction,
    entries: Vec<UtxoEntry>,
    /// Held so the copies are erased when this drops; only the laptop's `sign` also counts them.
    #[cfg_attr(not(feature = "laptop"), allow(dead_code))]
    keys: Erased,
    #[cfg_attr(not(feature = "laptop"), allow(dead_code))]
    name: &'static str,
    lines: Vec<String>,
}

/// cold-send on an account whose safe is an outside address (`set-cold-to`): nothing here signs it.
pub const COLD_EXTERNAL: &str = concat!(
    "This account's safe is an outside address you chose (your own wallet); ",
    "there are no cold sheets here. Pay from that wallet's own app.",
);

/// Offline: pick the keys, rebuild the transaction from this device's own scripts, sign it in memory,
/// measure the fee in the engine, and refuse anything the human should never be asked to approve.
fn check(ctx: &Ctx, req: &SignRequest, fp: &str, sheets: Option<Vec<usize>>) -> Result<Checked, String> {
    let name = req.name()?;
    let in_total = req.in_total()?;
    let mut lines = vec![];
    let short = |h: &Hash| h.to_string()[..8].to_string();
    // 0. This device's own keys.json: the public block must be the one its secrets make (and hash to the code written in it).
    ctx.keys.check_self()?;
    lines.push(format!("Rule block:   code {} (the online fund printed the same)", ctx.keys.public().sha256()));
    // 1. What kind of action, with which key(s). Card payments, withdrawals and the heir never come here.
    let keys = match req.kind {
        CANCEL | DECLINE => {
            // The request's dials price this move (fee allowance, the amount that must stay), so a set
            // that cannot be a real account's is refused in words. The heir rule is a funding rule and is
            // not judged here: an emergency stop must never be refused over it.
            req.dials.check(false, ctx.network.is_mainnet())?;
            let slot = req.which as usize;
            if slot >= 3 {
                return Err(format!("the request names stop slot {}; there are three (1, 2, 3)", req.which));
            }
            let key = ctx.keys.try_stop_key(slot)?;
            lines.push(if req.kind == CANCEL {
                format!("Action:       EMERGENCY STOP by stop key {} (everything in the account moves to the cold safe; the account closes)", slot + 1)
            } else {
                format!("Action:       DECLINE by stop key {} (the waiting withdrawal is withdrawn; the account stays open, Locked again; nothing leaves but the fee)", slot + 1)
            });
            Erased(vec![key])
        }
        _ if ctx.keys.public().cold_external => return Err(COLD_EXTERNAL.into()),
        _ if ctx.keys.legacy_safe() => {
            lines.push("Action:       SAFE WITHDRAWAL with the one cold key (legacy single-key safe)".to_string());
            Erased(vec![ctx.keys.cold_key()])
        }
        _ => {
            let (names, two) = two_cold(ctx.keys.cold_keys(), &sheets)?;
            lines.push(format!("Action:       SAFE WITHDRAWAL with sheets {} (2-of-3 safe)", names.join(",")));
            two
        }
    };
    // 2. The account, and that the coin really belongs to it. The input script is derived here from this
    //    device's own keys, so a request can only ever spend a coin at this vault or this safe.
    let outpoint = format!("{}:{}", req.outpoint.transaction_id, req.outpoint.index);
    match req.kind {
        CANCEL | DECLINE => {
            let state = req.state.as_ref().ok_or(VAULT_STATE_MISSING)?;
            if req.kind == DECLINE && state.stage != 1 {
                return Err(NOTHING_WAITING.into());
            }
            let id = req.covenant_id.ok_or(NO_LINEAGE)?;
            let local = ctx.try_load_vault()?;
            if let Some(v) = &local
                && v.covenant_id != id {
                    return Err(format!("the request spends a different account ({}) than this device's state.json ({})", short(&id), short(&v.covenant_id)));
                }
            let art = ctx.try_compile_with(&req.dials, state)?;
            let addr = ctx.network.address_of(&contract::spk_of(&art)).map(|a| a.to_string()).unwrap_or_else(|| "a script that is no address".into());
            lines.push(format!("Account:      vault {} (covenant id), stage {}", short(&id), state.stage));
            lines.push(format!("Coin:         {outpoint} holding {:.4} KAS at {addr}", kas(req.amount)));
            if req.kind == DECLINE {
                if req.output0 < req.dials.min_remainder as u64 {
                    return Err(format!("declining would leave {:.4} KAS in the account, under the {} KAS it must keep; refused", kas(req.output0), kas(req.dials.min_remainder as u64)));
                }
                lines.push(format!(
                    "Decline:      decline (stop key {}): vault {addr} back to Locked, the waiting withdrawal of {:.4} KAS is withdrawn, fee {:.5} KAS",
                    req.which + 1,
                    kas(state.amount as u64),
                    kas(req.amount.saturating_sub(req.output0))
                ));
            }
            let hash = local.and_then(|v| v.contract_sha256);
            let source = contract::load_source(&ctx.dir, &ctx.state_path(), hash.as_deref())?;
            let pinned = ctx.dir.join(SOURCE_FILE).exists();
            lines.push(format!(
                "Rules file:   sha256 {} ({})",
                sha256_hex(source.as_bytes()),
                if pinned { "pinned copy in this folder" } else { "repository copy; copy the online device's vault.sil here to pin it" }
            ));
        }
        _ => {
            if req.pay.is_none() {
                return Err("a cold-send request must carry the payment".into());
            }
            lines.push(format!("Account:      cold safe {}", ctx.keys.public().cold_address));
            if req.extra.is_empty() {
                lines.push(format!("Coin:         {outpoint} holding {:.4} KAS", kas(req.amount)));
            } else {
                let n = req.extra.len() + 1;
                lines.push(format!("Coin 1 of {n}:  {outpoint} holding {:.4} KAS", kas(req.amount)));
                for (i, (op, amount, _)) in req.extra.iter().enumerate() {
                    lines.push(format!("Coin {} of {n}:  {}:{} holding {:.4} KAS", i + 2, op.transaction_id, op.index, kas(*amount)));
                }
                lines.push(format!(
                    "Coins:        {:.4} KAS together (the smaller could not move alone)",
                    kas(in_total)
                ));
            }
        }
    }
    // 3. Where the money goes: every output must decode to an address a person can read, and none may be empty
    //    (the mass rules divide by output values; a zero output is a malformed request, not a fee).
    let sweep = req.is_sweep();
    if req.output0 == 0 && !sweep {
        return Err("output 0 (what goes to the cold safe) is 0 KAS; refused".into());
    }
    let (mut tx, entries) = req.build(ctx)?;
    let out_total = tx.outputs.iter().try_fold(0u64, |a, o| a.checked_add(o.value)).ok_or("the outputs overflow")?;
    let fee_before = in_total.checked_sub(out_total).ok_or_else(|| format!("the outputs ({:.4} KAS) add up to more than the coin(s) ({:.4} KAS)", kas(out_total), kas(in_total)))?;
    let change = if sweep { None } else { Some(readable_address(&ctx.network, &tx.outputs[0].script_public_key)?) };
    let pay = match &req.pay {
        None => None,
        Some((0, _)) => return Err("the payment is 0 KAS".into()),
        Some((value, spk)) => Some((*value, readable_address(&ctx.network, spk)?)),
    };
    // 4. Sign in memory and measure what the engine itself needs; trim output 0 only within the contract's allowance.
    let do_sign = |tx: &mut Transaction| -> Result<(), String> {
        ctx.try_sign_and_budget(tx, &entries, &|i, t| {
            let sigs: Vec<Vec<u8>> = keys.0.iter().map(|k| ctx.sign_input(t, &entries, i, k)).collect();
            req.sigscript(ctx, &sigs)
        })
    };
    do_sign(&mut tx)?;
    let need = ctx.required_fee(&tx, &entries);
    let mut fee = fee_before;
    let mut trimmed = None;
    if fee < need {
        let trim = need - fee;
        let allowance = if req.kind == COLD_SEND { own_fee_allowance(ctx)? } else { req.dials.max_fee as u64 };
        if trim > allowance {
            return Err(format!("the online side sized the fee {:.5} KAS short; more than the fee allowance of {:.2} KAS, refused. Make the request again on the online device.", kas(trim), kas(allowance)));
        }
        let left = tx.outputs[0].value.checked_sub(trim).ok_or("output 0 is too small to carry the fee")?;
        println!("fee {fee} sompi is under the node's minimum {need} once the budget is set; lowering output 0 by {trim} sompi");
        tx.outputs[0].value = left;
        do_sign(&mut tx)?;
        fee = need;
        trimmed = Some(trim);
    }
    let first = tx.outputs[0].value;
    match (&pay, &change) {
        (None, Some(vault)) if req.kind == DECLINE => {
            lines.push("Leaving:      nothing (the waiting withdrawal is withdrawn)".to_string());
            lines.push(format!("Staying:      {:.4} KAS in the account at {vault}, Locked again", kas(first)));
        }
        (None, Some(cold)) => {
            lines.push(format!("Leaving:      everything, {:.4} KAS, to the cold safe {cold}", kas(first)));
            lines.push("Staying:      nothing (the account closes)".to_string());
        }
        (None, None) => return Err("a cancel must pay the cold safe".into()),
        (Some((value, to)), Some(cold)) => {
            lines.push(format!("Leaving:      {:.4} KAS to {to}", kas(*value)));
            lines.push(format!("Staying:      {:.4} KAS back to the cold safe {cold}", kas(first)));
        }
        (Some((_, to)), None) => {
            lines.push(format!("Leaving:      {:.4} KAS to {to} (the whole coin{} less the fee; change too small to keep went with the payment)", kas(first), if req.extra.is_empty() { "" } else { "s" }));
            lines.push("Staying:      nothing (the safe coin(s) are spent in full)".to_string());
        }
    }
    if let Some(trim) = trimmed {
        lines.push(format!(
            "Trimmed:      the online side's fee guess fell {:.5} KAS short of the engine's need; taken from {}",
            kas(trim),
            if sweep { "the payment" } else { "the amount staying" }
        ));
    }
    // 5. The fee, capped here whatever the request says.
    if fee > FEE_CEILING {
        return Err(format!("fee {:.5} KAS is more than the {:.2} KAS this device ever signs; refused", kas(fee), kas(FEE_CEILING)));
    }
    if fee > FEE_MULTIPLE * need {
        return Err(format!("fee {:.5} KAS is {:.1} times the engine's own need of {:.5} KAS (limit {FEE_MULTIPLE}x); refused", kas(fee), fee as f64 / need as f64, kas(need)));
    }
    lines.push(format!(
        "Fee:          {:.5} KAS = {:.2}x the engine's own need of {:.5} KAS, {:.3}% of the coin",
        kas(fee),
        fee as f64 / need as f64,
        kas(need),
        fee as f64 * 100.0 / in_total as f64
    ));
    lines.push(format!("Transaction:  {}", tx.id()));
    lines.push(format!("Fingerprint:  {fp}  (must match the online screen)"));
    Ok(Checked { tx, entries, keys, name, lines })
}

/// Offline: show the human what the request does, wait for `y`, then write airgap/response.json (+ QR).
/// `sheets` picks which two cold sheets sign a 2-of-3 cold-send (default: the first two present here).
/// Ok(true) = signed and written; Ok(false) = the human said no, nothing written; Err = refused before asking.
/// The request must be for this folder's network; a request from before the tag counts as testnet.
#[cfg(feature = "laptop")]
fn check_request_network(ctx: &Ctx, req: &SignRequest) -> Result<(), String> {
    let made_for = if req.network.is_empty() { crate::network::TESTNET } else { req.network.as_str() };
    if made_for == ctx.network.name {
        return Ok(());
    }
    Err(format!("this request was made for the {made_for} network; this device's folder is on {}. Make the request again on the online device with --network {}, or sign it in the folder made for {made_for}.", ctx.network.name, ctx.network.name))
}

/// The request's fee floor must sit within FEE_RATE_MULTIPLE times this device's own network floor;
/// above that the online device (or whoever edited the request) is pricing the move, not the network.
#[cfg(feature = "laptop")]
fn check_fee_rate(ctx: &Ctx, req: &SignRequest) -> Result<(), String> {
    let ceiling = FEE_RATE_MULTIPLE * ctx.network.fee_floor;
    if req.fee_rate > ceiling {
        return Err("the request asks for a network fee far above what this network charges; make the request again on the online device".into());
    }
    Ok(())
}
/// What this device itself lets a cold-send fee trim take: its network preset's allowance. Cold coins
/// carry no covenant, so nothing on chain pins the request's dials; cancel and decline use the request's
/// (they are constructor arguments, a mismatch makes the transaction invalid on chain).
fn own_fee_allowance(ctx: &Ctx) -> Result<u64, String> {
    Ok(Dials::preset(ctx.network.dials, ctx.network.daa_per_sec)?.max_fee as u64)
}

#[cfg(feature = "laptop")]
pub fn sign(ctx: &Ctx, request: &Path, sheets: Option<Vec<usize>>, answer: &mut dyn BufRead) -> Result<bool, String> {
    assert!(ctx.rpc.is_none());
    let (req, bytes) = read_request(request)?;
    // Item 5: the online device measured the network's fee floor; price the move with it (never below this
    // device's own floor, never above FEE_RATE_MULTIPLE times it).
    check_fee_rate(ctx, &req)?;
    check_request_network(ctx, &req)?;
    set_fee_rate(req.fee_rate, &ctx.network);
    let fp = fingerprint(&bytes);
    let c = check(ctx, &req, &fp, sheets)?;
    println!("\n=== Check this before signing ===");
    for l in &c.lines {
        println!("{l}");
    }
    print!("Sign? [y/N] ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut line = String::new();
    answer.read_line(&mut line).map_err(|e| format!("reading the answer: {e}"))?;
    if line.trim() != "y" {
        println!("Not signed.");
        return Ok(false);
    }
    let tx = c.tx;
    let sigs = first_pushes(&tx.inputs[0].signature_script, c.keys.0.len());
    let resp = SignResponse {
        kind: c.name.into(),
        txid: tx.id().to_string(),
        request: fp,
        signatures: sigs.iter().map(|s| faster_hex::hex_string(s)).collect(),
        budget: tx.inputs[0].compute_commit.compute_budget().expect("v1 input has a budget"),
        output0_value: tx.outputs[0].value,
        more: tx.inputs[1..]
            .iter()
            .map(|input| InputSigs {
                signatures: first_pushes(&input.signature_script, c.keys.0.len()).iter().map(|s| faster_hex::hex_string(s)).collect(),
                budget: input.compute_commit.compute_budget().expect("v1 input has a budget"),
            })
            .collect(),
    };
    let dir = airgap_dir(ctx);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("response.json");
    let json = serde_json::to_string(&resp).unwrap();
    std::fs::write(&path, &json).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("signed {} tx {}\nsigned response written to {}", resp.kind, resp.txid, path.display());
    show_qr("Signed response", &json);
    println!("Next, on the online device: run `submit <response.json>`");
    Ok(true)
}

/// The answer's signatures as bytes: each must be SIG_LEN bytes of hex, else the answer is refused.
#[cfg(feature = "laptop")]
fn answer_sigs(hexes: &[String]) -> Result<Vec<Vec<u8>>, String> {
    let decode = |s: &String| {
        let mut b = vec![0u8; SIG_LEN];
        let ok = s.len() == 2 * SIG_LEN && faster_hex::hex_decode(s.as_bytes(), &mut b).is_ok();
        if ok { Ok(b) } else { Err(BAD_SIGNATURE.to_string()) }
    };
    hexes.iter().map(decode).collect()
}

/// Online: the transaction the offline device signed, rebuilt from our own request and checked against
/// the answer: same request (fingerprint), same kind, output 0 moved only within the fee allowance, same txid.
#[cfg(feature = "laptop")]
fn rebuild(ctx: &Ctx, req: &SignRequest, fp: &str, resp: &SignResponse) -> Result<(Transaction, Vec<UtxoEntry>), String> {
    if resp.request != fp {
        return Err(DIFFERENT_REQUEST.into());
    }
    if req.name()? != resp.kind {
        return Err(format!("the answer is for a {} but the request is a {}", resp.kind, req.name()?));
    }
    if resp.signatures.len() != req.sig_count(ctx) {
        return Err(format!("the answer carries {} signature(s); this request needs {}", resp.signatures.len(), req.sig_count(ctx)));
    }
    if resp.more.len() != req.extra.len() || resp.more.iter().any(|m| m.signatures.len() != req.sig_count(ctx)) {
        return Err(format!("the answer signs {} further input(s); this request has {}", resp.more.len(), req.extra.len()));
    }
    let (mut tx, entries) = req.build(ctx)?;
    let asked = tx.outputs[0].value;
    let reach = resp.output0_value.saturating_add(req.dials.max_fee as u64);
    if resp.output0_value > asked || reach < asked {
        return Err("output 0 moved by more than the fee allowance".into());
    }
    tx.outputs[0].value = resp.output0_value;
    tx.inputs[0].compute_commit = ComputeCommit::from(ComputeBudget(resp.budget));
    let sigs = answer_sigs(&resp.signatures)?;
    tx.inputs[0].signature_script = req.sigscript(ctx, &sigs);
    for (i, m) in resp.more.iter().enumerate() {
        tx.inputs[i + 1].compute_commit = ComputeCommit::from(ComputeBudget(m.budget));
        let sigs = answer_sigs(&m.signatures)?;
        tx.inputs[i + 1].signature_script = req.sigscript(ctx, &sigs);
    }
    tx.finalize();
    if tx.id().to_string() != resp.txid {
        return Err("rebuilt transaction differs from what the offline device signed".into());
    }
    Ok((tx, entries))
}

/// Online: make sure the offline device signed what was asked (and nothing else), prove it in the engine, send it.
#[cfg(feature = "laptop")]
pub async fn submit(ctx: &Ctx, response: &Path) -> Result<(), String> {
    let req_path = airgap_dir(ctx).join("request.hex");
    let (req, bytes) = read_request(&req_path)?;
    let fp = fingerprint(&bytes);
    let resp: SignResponse = read_json(response)?;
    let (tx, entries) = rebuild(ctx, &req, &fp, &resp)?;
    ctx.verify(&tx, &entries)?;
    let id = ctx.try_submit(&tx, &format!("{} (signed offline)", resp.kind)).await?;
    // Sent: neither file may be signed or sent a second time.
    clear_airgap(ctx);
    if req.kind == CANCEL {
        if let Some(mut v) = ctx.try_load_vault()? {
            v.history.push(format!("cancel signed offline: tx {id}"));
            ctx.save_anchored(&v)?;
        }
        println!("vault emptied to cold; remove state.json before funding again");
    }
    if req.kind == DECLINE {
        if let Some(mut v) = ctx.try_load_vault()? {
            let dest = v.pending_to.take().unwrap_or_else(|| "an unrecorded address".to_string());
            let amount = v.state.amount as u64;
            v.state = after_decline(&v.state);
            v.history.push(format!(
                "decline (stop key {}) signed offline: the request for {} KAS to {dest} is withdrawn, the vault is Locked again, fee {:.5} KAS, tx {id}",
                req.which + 1,
                kas(amount),
                kas(req.amount.saturating_sub(tx.outputs[0].value))
            ));
            ctx.save_anchored(&v)?;
        }
        println!("withdrawal declined; the vault is Locked again");
    }
    Ok(())
}

/// Moves the three stop secrets (cancel, cancel2, cancel3) and all cold secrets (the three sheets of
/// the safe) to the offline device's folder and leaves public keys here. No full copy is kept anywhere:
/// the safe sheets are to be written on paper from the offline file. The heir secret stays in the
/// online file for this testnet run only, so `inherit` can be driven from here; a real heir keeps
/// their own key on their own device and never hands it to the vault's owner.
#[cfg(feature = "laptop")]
pub fn split_keys(online: &Path, offline: &Path) -> Result<(), String> {
    let path = std::path::absolute(online.join("keys.json")).map_err(|e| format!("{}: {e}", online.display()))?;
    let keys = Keys::load(&path)?;
    let plain = !Keys::is_locked(&path);
    println!("note: split-keys makes every secret on this machine first and leaves the old file's bytes on this disk. For real money make the stop keys and cold sheets with `keys --offline` on the offline device and bring offline-public.json here with `keys --import-public`.");
    if keys.cancel.is_none() && keys.cold_keys().is_empty() {
        return Err("no cancel or cold secret here to move".into());
    }
    keys.check_self()?;
    let public = keys.public();
    let code = public.sha256();
    std::fs::create_dir_all(offline).map_err(|e| format!("{}: {e}", offline.display()))?;
    // Full paths in the printout, so the operator sees exactly which folder now holds the secrets.
    let offline = std::path::absolute(offline).map_err(|e| format!("{}: {e}", offline.display()))?;
    let off_path = offline.join("keys.json");
    if off_path.exists() {
        return Err(format!("{} already exists", off_path.display()));
    }
    let off = Keys {
        cancel: keys.cancel,
        cancel2: keys.cancel2,
        cancel3: keys.cancel3,
        cold: keys.cold,
        cold2: keys.cold2,
        cold3: keys.cold3,
        public: Some(public.clone()),
        public_sha256: Some(code.clone()),
        ..Default::default()
    };
    let on = Keys { spend: keys.spend, funding: keys.funding, heir: keys.heir, public: Some(public), public_sha256: Some(code.clone()), ..Default::default() };
    // Both halves keep the form of the file they came from (a locked file: the same passphrase for both).
    off.save(&off_path, plain)?;
    on.save(&path, plain)?;
    println!("offline keys (stop keys, cold sheets + public): {}\nonline keys (spend, funding, heir + public): {}", off_path.display(), path.display());
    println!("rule block code: {code} (written in both files; fund must print the same, and the offline device shows it before it signs)");
    println!("Write the safe sheets on paper now; this file holds them: {}", off_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline_ctx(tag: &str) -> Ctx {
        let dir = std::env::temp_dir().join(format!("kv-airgap-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        contract::pin_source(&dir).unwrap();
        Ctx { dir, keys: Keys::fresh(&Network::testnet()), rpc: None, force: false, network: Network::testnet(), dials: Dials::testnet(), account: 1 }
    }
    fn pay_to() -> ScriptPublicKey {
        Network::testnet().to_spk(&Network::testnet().p2pk(&xonly(&keypair(&[7u8; 32]))).to_string())
    }
    /// A cold-send of 40 KAS from a 100 KAS safe coin, sized the way the online device sizes it.
    fn cold_send(ctx: &Ctx, pay: ScriptPublicKey) -> (SignRequest, u64) {
        let req = SignRequest {
            kind: COLD_SEND,
            which: 0,
            outpoint: TransactionOutpoint::new(Hash::from_bytes([9u8; 32]), 1),
            amount: 100 * KAS,
            daa: 5_000,
            covenant_id: None,
            state: None,
            output0: 0,
            pay: Some((40 * KAS, pay)),
            extra: vec![],
            network: ctx.network.name.into(),
            dials: Dials::testnet(),
            fee_rate: 100,
        };
        req.sized(ctx, 60 * KAS, false).unwrap()
    }
    /// A cold coin of `amount` KAS at this device's safe.
    fn cold_coin(ctx: &Ctx, tag: u8, amount: f64) -> Coin {
        (TransactionOutpoint::new(Hash::from_bytes([tag; 32]), 0), UtxoEntry::new(sompi(amount), ctx.cold_spk(), 5_000, false, None))
    }
    fn write_request(ctx: &Ctx, req: &SignRequest) -> (PathBuf, String) {
        let dir = airgap_dir(ctx);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = borsh::to_vec(req).unwrap();
        let path = dir.join("request.hex");
        std::fs::write(&path, faster_hex::hex_string(&bytes).to_ascii_uppercase()).unwrap();
        (path, fingerprint(&bytes))
    }
    fn run(ctx: &Ctx, req: &SignRequest, answer: &str) -> Result<bool, String> {
        let (path, _) = write_request(ctx, req);
        std::fs::remove_file(airgap_dir(ctx).join("response.json")).ok();
        sign(ctx, &path, None, &mut answer.as_bytes())
    }

    #[test]
    fn y_signs_and_the_answer_is_bound_to_the_request() {
        let ctx = offline_ctx("y");
        let (req, _) = cold_send(&ctx, pay_to());
        assert_eq!(run(&ctx, &req, "y\n"), Ok(true));
        let json = std::fs::read_to_string(airgap_dir(&ctx).join("response.json")).unwrap();
        assert!(!json.contains("\"signature\":"), "the single signature field is gone: {json}");
        let resp: SignResponse = serde_json::from_str(&json).unwrap();
        let (_, fp) = write_request(&ctx, &req);
        assert_eq!((resp.request.as_str(), resp.signatures.len(), resp.kind.as_str()), (fp.as_str(), 2, "cold-send"));
        // the budget set after the engine run adds mass, so the offline side may trim output 0, but only within the allowance
        assert!(resp.output0_value <= req.output0 && resp.output0_value + MAX_FEE as u64 >= req.output0, "{} vs {}", resp.output0_value, req.output0);
        // the online side accepts it, and refuses the same answer for any other request
        let (tx, entries) = rebuild(&ctx, &req, &fp, &resp).unwrap();
        ctx.verify(&tx, &entries).unwrap();
        assert_eq!(rebuild(&ctx, &req, "00000000", &resp).unwrap_err(), DIFFERENT_REQUEST);
        let mut other = req.clone();
        other.output0 -= 1;
        let (_, other_fp) = write_request(&ctx, &other);
        assert_eq!(rebuild(&ctx, &other, &other_fp, &resp).unwrap_err(), DIFFERENT_REQUEST);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn anything_but_y_signs_nothing() {
        let ctx = offline_ctx("n");
        let (req, _) = cold_send(&ctx, pay_to());
        for answer in ["n\n", "\n", "", "yes\n", "Y\n"] {
            assert_eq!(run(&ctx, &req, answer), Ok(false), "answer {answer:?}");
            assert!(!airgap_dir(&ctx).join("response.json").exists(), "answer {answer:?} wrote a response");
        }
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn sign_refuses_keys_whose_public_block_was_edited() {
        let mut ctx = offline_ctx("edited-block");
        let (good, _) = cold_send(&ctx, pay_to());
        assert_eq!(run(&ctx, &good, "n\n"), Ok(false), "the untouched block is fine");
        // the safe swapped in the block: the cold sheets here do not make that address, so no signature
        let honest = ctx.keys.public();
        ctx.keys.public.as_mut().unwrap().cold_address = ctx.network.p2pk(&xonly(&keypair(&[9u8; 32]))).to_string();
        let err = run(&ctx, &good, "y\n").unwrap_err();
        assert!(err.contains("do not match the rule block"), "{err}");
        assert!(!airgap_dir(&ctx).join("response.json").exists(), "a refused request wrote a response");
        // the block as split-keys wrote it, with its code: an edit the secrets cannot see (the dial) is caught by the code
        ctx.keys.public = Some(honest.clone());
        ctx.keys.public_sha256 = Some(honest.sha256());
        assert_eq!(run(&ctx, &good, "n\n"), Ok(false));
        ctx.keys.public.as_mut().unwrap().auto_heir = !honest.auto_heir;
        let err = run(&ctx, &good, "y\n").unwrap_err();
        assert!(err.contains("code written in it"), "{err}");
        // split-keys refuses to split a file whose block its secrets disagree with
        let dir = ctx.dir.join("split-src");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keys.json"), serde_json::to_string_pretty(&ctx.keys).unwrap()).unwrap();
        let err = split_keys(&dir, &dir.join("offline")).unwrap_err();
        assert!(err.contains("code written in it"), "{err}");
        // and writes the code into both halves when the block is honest
        ctx.keys.public = Some(honest.clone());
        ctx.keys.public_sha256 = None;
        std::fs::write(dir.join("keys.json"), serde_json::to_string_pretty(&ctx.keys).unwrap()).unwrap();
        split_keys(&dir, &dir.join("offline")).unwrap();
        let off: Keys = read_json(&dir.join("offline").join("keys.json")).unwrap();
        let on: Keys = read_json(&dir.join("keys.json")).unwrap();
        assert_eq!((off.public_sha256.as_deref(), on.public_sha256.as_deref()), (Some(honest.sha256().as_str()), Some(honest.sha256().as_str())));
        assert_eq!(off.check_self(), Ok(()));
    }

    #[test]
    fn refusals_happen_before_the_question() {
        let ctx = offline_ctx("refuse");
        let (good, fee) = cold_send(&ctx, pay_to());
        let refused = |req: &SignRequest, what: &str| {
            let err = run(&ctx, req, "y\n").unwrap_err();
            assert!(err.contains(what), "{what}: {err}");
            assert!(!airgap_dir(&ctx).join("response.json").exists(), "{what}: a refused request wrote a response");
        };
        // fee above the ceiling (output 0 trimmed by 0.2 KAS)
        let mut r = good.clone();
        r.output0 -= KAS / 5;
        refused(&r, "0.10 KAS this device ever signs");
        // fee above 2x the engine's need
        let mut r = good.clone();
        r.output0 -= 3 * fee;
        refused(&r, "times the engine's own need");
        // output 0 at nothing: a cold-send becomes a sweep whose 60 KAS fee is refused; a cancel is refused as such
        let mut r = good.clone();
        r.output0 = 0;
        refused(&r, "this device ever signs");
        let empty_cancel = SignRequest { kind: CANCEL, which: 0, state: Some(locked()), covenant_id: Some(Hash::from_bytes([3u8; 32])), pay: None, output0: 0, ..good.clone() };
        refused(&empty_cancel, "is 0 KAS");
        // outputs above the coin (A4: no wrap, a refusal)
        let mut r = good.clone();
        r.output0 = good.amount;
        refused(&r, "more than the coin");
        // a destination with no address form
        let mut r = good.clone();
        r.pay = Some((40 * KAS, ScriptPublicKey::from_vec(0, vec![0x51])));
        refused(&r, "not an address a person can read");
        // unknown kind, stop slot out of range, cancel without lineage
        let mut r = good.clone();
        r.kind = 9;
        refused(&r, "unknown request kind");
        let cancel = SignRequest { kind: CANCEL, which: 7, state: Some(locked()), covenant_id: Some(Hash::from_bytes([3u8; 32])), pay: None, ..good.clone() };
        refused(&cancel, "stop slot 7");
        let stray = SignRequest { which: 1, covenant_id: None, ..cancel.clone() };
        refused(&stray, "no account lineage");
        // a cancel whose account differs from this device's state.json
        let sha = sha256_hex(&std::fs::read(ctx.dir.join(SOURCE_FILE)).unwrap());
        let v = Vault { covenant_id: Hash::from_bytes([4u8; 32]), state: locked(), pending_to: None, history: vec![], contract_sha256: Some(sha), network: String::new(), dials: Dials::testnet() };
        ctx.try_save_vault(&v).unwrap();
        let other_account = SignRequest { which: 1, ..cancel.clone() };
        refused(&other_account, "a different account");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn cold_send_picks_coins_smallest_first_and_signs_every_input() {
        let ctx = offline_ctx("pick");
        // the stuck 0.2715 coin rides along with the 0.5 coin: two inputs, change over the floor
        let coins = vec![cold_coin(&ctx, 1, 0.5), cold_coin(&ctx, 2, 0.2715), cold_coin(&ctx, 3, 20.0)];
        let (req, fee) = choose_cold_coins(&ctx, coins.clone(), sompi(0.1), pay_to()).unwrap();
        assert_eq!((req.amount, req.extra.len(), req.extra[0].1), (sompi(0.2715), 1, sompi(0.5)));
        assert!(req.output0 >= CHANGE_FLOOR && req.output0 + sompi(0.1) + fee == sompi(0.7715), "change {} fee {fee}", req.output0);
        // the whole round trip: offline signs both inputs, online rebuilds and proves it in the engine
        assert_eq!(run(&ctx, &req, "y\n"), Ok(true));
        let resp: SignResponse = serde_json::from_str(&std::fs::read_to_string(airgap_dir(&ctx).join("response.json")).unwrap()).unwrap();
        assert_eq!((resp.signatures.len(), resp.more.len(), resp.more[0].signatures.len()), (2, 1, 2));
        let (_, fp) = write_request(&ctx, &req);
        let (tx, entries) = rebuild(&ctx, &req, &fp, &resp).unwrap();
        assert_eq!(tx.inputs.len(), 2);
        ctx.verify(&tx, &entries).unwrap();
        // a lone small coin is refused with the fix named; a bigger send carries both small coins along
        let err = choose_cold_coins(&ctx, vec![cold_coin(&ctx, 2, 0.2715)], sompi(0.1), pay_to()).err().unwrap();
        assert!(err.contains("add a bigger coin to the safe"), "{err}");
        let (req, _) = choose_cold_coins(&ctx, coins, sompi(3.0), pay_to()).unwrap();
        assert_eq!((req.amount, req.extra.len(), req.extra[1].1), (sompi(0.2715), 2, sompi(20.0)));
        // nearly the whole coin: the crumb of change goes with the payment (a sweep), and offline shows it
        let (req, fee) = choose_cold_coins(&ctx, vec![cold_coin(&ctx, 4, 1.0)], sompi(0.98), pay_to()).unwrap();
        assert!(req.is_sweep() && req.pay.as_ref().unwrap().0 + fee == KAS, "{} {fee}", req.pay.as_ref().unwrap().0);
        assert_eq!(run(&ctx, &req, "y\n"), Ok(true));
        // change between maxFee and the floor is refused, naming the sweep amount
        let err = choose_cold_coins(&ctx, vec![cold_coin(&ctx, 4, 1.0)], sompi(0.7), pay_to()).err().unwrap();
        assert!(err.contains("too little to keep as a coin") && err.contains("to empty the safe"), "{err}");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// The phone's take-out on the host: everything in the safe to one address is a sweep over every coin,
    /// smallest first; a device holding two sheets signs it through the same `check` the offline device runs,
    /// two 65-byte signatures in redeem-key order then the redeem script on every input, and the engine proves
    /// it. Swapped signatures fail the engine (OP_CHECKMULTISIG walks the keys forward); one sheet is refused.
    #[test]
    fn takeout_sweeps_every_cold_coin_and_two_sheets_sign_in_redeem_order() {
        let ctx = offline_ctx("takeout");
        let coins = vec![cold_coin(&ctx, 1, 20.0), cold_coin(&ctx, 2, 0.5), cold_coin(&ctx, 3, 3.0)];
        let (req, fee) = cold_send_request(&ctx, coins.clone(), None, pay_to()).unwrap();
        assert!(req.is_sweep() && req.output0 == 0);
        assert_eq!((req.amount, req.extra.len(), req.extra[0].1, req.extra[1].1), (sompi(0.5), 2, sompi(3.0), sompi(20.0)), "smallest first");
        assert_eq!(req.pay.as_ref().unwrap().0 + fee, sompi(23.5), "everything less the fee");
        // an amount goes the airgap way (change back to the safe)
        let (some, _) = cold_send_request(&ctx, coins, Some(sompi(1.0)), pay_to()).unwrap();
        assert!(!some.is_sweep() && some.output0 >= CHANGE_FLOOR);
        // the phone with sheets 1 and 3 typed: cold2 absent
        let mut two = ctx.keys.clone();
        two.cold2 = None;
        let phone = Ctx { keys: two, ..ctx.clone() };
        let c = sign_here(&phone, &req).unwrap();
        assert_eq!((c.tx.inputs.len(), c.tx.outputs.len(), c.entries.len()), (3, 1, 3));
        let redeem = ctx.keys.public().cold_redeem().unwrap();
        for (i, input) in c.tx.inputs.iter().enumerate() {
            let pushes = first_pushes(&input.signature_script, 2);
            assert_eq!((pushes[0].len(), pushes[1].len()), (65, 65), "input {i}: two 65-byte signatures first");
            assert_eq!(input.signature_script, multisig_sigscript(&pushes, &redeem), "input {i}: then the redeem script as one push");
        }
        ctx.verify(&c.tx, &c.entries).unwrap();
        // the same two signatures the other way round fail the engine: redeem-key order is load-bearing
        let mut swapped = c.tx.clone();
        let pushes = first_pushes(&swapped.inputs[0].signature_script, 2);
        swapped.inputs[0].signature_script = multisig_sigscript(&[pushes[1].clone(), pushes[0].clone()], &redeem);
        assert!(ctx.verify(&swapped, &c.entries).is_err());
        // one sheet cannot
        let mut one = ctx.keys.clone();
        (one.cold2, one.cold3) = (None, None);
        assert!(sign_here(&Ctx { keys: one, ..ctx.clone() }, &req).err().unwrap().contains("two of its three seeds"), "one sheet is refused");
        // an empty safe
        assert!(cold_send_request(&ctx, vec![], None, pay_to()).err().unwrap().contains("no mature coins"));
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn decline_round_trip_and_locked_refusal() {
        let ctx = offline_ctx("decline");
        let pending = State { stage: 1, dest_hash: spk_hash(&pay_to()), amount: 3 * KAS as i64, window_start: 0, spent: 0 };
        let req = SignRequest {
            kind: DECLINE,
            which: 1,
            outpoint: TransactionOutpoint::new(Hash::from_bytes([9u8; 32]), 0),
            amount: 10 * KAS,
            daa: 5_000,
            covenant_id: Some(Hash::from_bytes([5u8; 32])),
            state: Some(pending.clone()),
            output0: 0,
            pay: None,
            extra: vec![],
            network: ctx.network.name.into(),
            dials: Dials::testnet(),
            fee_rate: 100,
        };
        let (req, fee) = req.sized(&ctx, 10 * KAS, false).unwrap();
        assert!(fee > 0 && fee <= MAX_FEE as u64 && req.output0 + fee == 10 * KAS, "output0 {} fee {fee}", req.output0);
        // offline: stop key 2 signs it; the answer says what it is
        assert_eq!(run(&ctx, &req, "y\n"), Ok(true));
        let resp: SignResponse = serde_json::from_str(&std::fs::read_to_string(airgap_dir(&ctx).join("response.json")).unwrap()).unwrap();
        assert_eq!((resp.kind.as_str(), resp.signatures.len(), resp.more.len()), ("decline", 1, 0));
        // online: rebuilt, proven in the engine; the one output is the Locked continuation on the same lineage
        let (_, fp) = write_request(&ctx, &req);
        let (tx, entries) = rebuild(&ctx, &req, &fp, &resp).unwrap();
        ctx.verify(&tx, &entries).unwrap();
        assert_eq!(tx.outputs.len(), 1);
        assert_eq!(tx.outputs[0].script_public_key, ctx.vault_spk(&after_decline(&pending)));
        assert_eq!(tx.outputs[0].covenant.as_ref().map(|c| c.covenant_id), Some(Hash::from_bytes([5u8; 32])));
        assert!(tx.outputs[0].value >= MIN_REMAINDER as u64 && tx.outputs[0].value + MAX_FEE as u64 >= 10 * KAS);
        // a Locked vault has nothing to decline; a decline without lineage is refused like a cancel
        let err = run(&ctx, &SignRequest { state: Some(locked()), ..req.clone() }, "y\n").unwrap_err();
        assert_eq!(err, NOTHING_WAITING);
        let err = run(&ctx, &SignRequest { covenant_id: None, ..req.clone() }, "y\n").unwrap_err();
        assert!(err.contains("no account lineage"), "{err}");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn inflated_fee_request_is_refused() {
        let ctx = offline_ctx("inflated");
        let (req, _) = cold_send(&ctx, pay_to());
        let floor = ctx.network.fee_floor;
        // the rate ceiling is the device's own, FEE_RATE_MULTIPLE times its network floor
        assert!(check_fee_rate(&ctx, &SignRequest { fee_rate: FEE_RATE_MULTIPLE * floor, ..req.clone() }).is_ok());
        let err = check_fee_rate(&ctx, &SignRequest { fee_rate: FEE_RATE_MULTIPLE * floor + 1, ..req.clone() }).unwrap_err();
        assert!(err.contains("far above what this network charges") && err.contains("online device"), "{err}");
        // a tampered request: a huge fee rate and a 50 KAS allowance; refused before anything is priced or shown
        let bad = SignRequest { fee_rate: 1_000_000_000, dials: Dials { max_fee: 50 * KAS as i64, ..Dials::testnet() }, ..req.clone() };
        let err = run(&ctx, &bad, "y\n").unwrap_err();
        assert!(err.contains("far above what this network charges"), "{err}");
        assert!(!airgap_dir(&ctx).join("response.json").exists());
        assert_eq!(fee_rate(), floor, "the bad rate never reached the device's pricing");
        // a cold-send sized short on purpose: a tiny payment carries a big storage mass, so the engine needs
        // about 0.08 KAS of fee while the request left 1000 sompi. The request's 50 KAS allowance would let
        // the trim through; the device's own 0.05 KAS allowance refuses it.
        let payment = sompi(0.12);
        let short = SignRequest { output0: 100 * KAS - payment - 1_000, pay: Some((payment, pay_to())), dials: Dials { max_fee: 50 * KAS as i64, ..Dials::testnet() }, ..req.clone() };
        let err = run(&ctx, &short, "y\n").unwrap_err();
        assert!(err.contains("more than the fee allowance of 0.05 KAS"), "{err}");
        // the same request with the allowance set to 1 sompi still signs the honest round: the device judges with its own
        let stingy = SignRequest { dials: Dials { max_fee: 1, ..Dials::testnet() }, ..req.clone() };
        assert_eq!(run(&ctx, &stingy, "y\n"), Ok(true));
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    /// `f` run with a panic turned into the refusal "PANIC".
    fn no_panic<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        caught.unwrap_or_else(|_| Err("PANIC".into()))
    }

    #[test]
    fn malformed_requests_are_refused_in_words() {
        let ctx = offline_ctx("malformed");
        let (req, _) = cold_send(&ctx, pay_to());
        let op = TransactionOutpoint::new(Hash::from_bytes([8u8; 32]), 0);
        let coin = |amount: u64| vec![(op, amount, 5_000)];
        // a further coin so big the coins overflow when added, or one that holds nothing
        let huge = SignRequest { extra: coin(u64::MAX), ..req.clone() };
        assert_eq!(no_panic(|| run(&ctx, &huge, "y\n")).unwrap_err(), COINS_OVERFLOW);
        let empty = SignRequest { extra: coin(0), ..req.clone() };
        assert_eq!(no_panic(|| run(&ctx, &empty, "y\n")).unwrap_err(), ZERO_COIN);
        // a decline that would keep more than its coin holds
        let dest_hash = spk_hash(&pay_to());
        let pending = State { stage: 1, dest_hash, amount: KAS as i64, ..locked() };
        let over = SignRequest {
            kind: DECLINE,
            which: 1,
            covenant_id: Some(Hash::from_bytes([5u8; 32])),
            state: Some(pending),
            amount: 10 * KAS,
            output0: 20 * KAS,
            pay: None,
            ..req.clone()
        };
        let err = no_panic(|| run(&ctx, &over, "y\n")).unwrap_err();
        assert!(err.contains("add up to more than the coin"), "{err}");
        // a decline whose dials no account could have: refused in words before a key is picked
        let wild = SignRequest { output0: 9 * KAS, dials: Dials { max_fee: -1, ..Dials::testnet() }, ..over.clone() };
        let err = no_panic(|| run(&ctx, &wild, "y\n")).unwrap_err();
        assert!(err.contains("max fee dial must be above zero"), "{err}");
        let wild = SignRequest { output0: 9 * KAS, dials: Dials { min_remainder: 1, ..Dials::testnet() }, ..over.clone() };
        let err = no_panic(|| run(&ctx, &wild, "y\n")).unwrap_err();
        assert!(err.contains("must be below the min remainder dial"), "{err}");
        // the two cold keys that sign: a sheet that is not here, or one sheet alone, is refused; the keys
        // that leave are the named two, and wiping them leaves no secret behind
        let key = |b: u8| keypair(&[b; 32]);
        let three = || vec![(1, key(11)), (2, key(12)), (3, key(13))];
        assert!(two_cold(vec![(1, key(1)), (2, key(2))], &Some(vec![1, 3])).err().unwrap().contains("cold sheet 3 is not on this device"));
        assert!(two_cold(vec![(2, key(2))], &None).err().unwrap().contains("found 1."));
        assert!(two_cold(three(), &Some(vec![3])).err().unwrap().contains("found 1."));
        let (names, mut two) = two_cold(three(), &Some(vec![3, 1])).unwrap();
        assert_eq!((names, two.0.clone()), (vec!["1".to_string(), "3".to_string()], vec![key(11), key(13)]));
        assert_eq!(two_cold(three(), &None).unwrap().0, vec!["1".to_string(), "2".to_string()]);
        two.wipe();
        assert!(two.0.iter().all(|k| *k != key(11) && *k != key(13)), "wiped keys hold no secret");
        // another network's request is refused before its fee rate reaches this device's pricing
        let floor = ctx.network.fee_floor;
        let mainnet = crate::network::MAINNET.to_string();
        let other = SignRequest { network: mainnet, fee_rate: 2 * floor, ..req.clone() };
        assert!(run(&ctx, &other, "y\n").unwrap_err().contains("made for the mainnet network"));
        assert_eq!(fee_rate(), floor);
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn malformed_answer_signatures_are_refused_in_words() {
        let ctx = offline_ctx("bad-answer");
        let (req, _) = cold_send(&ctx, pay_to());
        assert_eq!(run(&ctx, &req, "y\n"), Ok(true));
        let json = std::fs::read_to_string(airgap_dir(&ctx).join("response.json")).unwrap();
        let (_, fp) = write_request(&ctx, &req);
        for bad in ["zz".repeat(SIG_LEN), "abc".into(), "00".repeat(600), String::new()] {
            let mut resp: SignResponse = serde_json::from_str(&json).unwrap();
            resp.signatures[0] = bad;
            assert_eq!(no_panic(|| rebuild(&ctx, &req, &fp, &resp)).unwrap_err(), BAD_SIGNATURE);
        }
        let resp: SignResponse = serde_json::from_str(&json).unwrap();
        assert!(rebuild(&ctx, &req, &fp, &resp).is_ok(), "the honest answer still rebuilds");
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn old_request_files_still_read() {
        let ctx = offline_ctx("old");
        let (req, _) = cold_send(&ctx, pay_to());
        // the pre-pass-10 layout: the same bytes without the trailing `extra` vector and network tag
        let mut old = borsh::to_vec(&req).unwrap();
        // (the dials and the fee floor, 56 bytes, come after the network tag now)
        old.truncate(old.len() - 56 - 4 - 4 - req.network.len());
        let path = airgap_dir(&ctx).join("request.hex");
        std::fs::create_dir_all(airgap_dir(&ctx)).unwrap();
        std::fs::write(&path, faster_hex::hex_string(&old).to_ascii_uppercase()).unwrap();
        // the one-coin layout is no longer read: a refusal that says what to do instead
        let err = read_request(&path).err().expect("the one-coin layout is refused");
        assert!(err.contains("Make a new one with `cancel --airgap`"), "{err}");
        // the pass-10 layout: `extra` but no network tag; it still signs on a testnet folder
        let mut v2 = borsh::to_vec(&req).unwrap();
        v2.truncate(v2.len() - 56 - 4 - req.network.len());
        std::fs::write(&path, faster_hex::hex_string(&v2).to_ascii_uppercase()).unwrap();
        let (read, _) = read_request(&path).unwrap();
        assert_eq!((read.extra.len(), read.network.as_str()), (0, ""));
        // the layout with the network tag but before the dials and fee floor: read with the testnet preset
        let mut v3 = borsh::to_vec(&req).unwrap();
        v3.truncate(v3.len() - 56);
        std::fs::write(&path, faster_hex::hex_string(&v3).to_ascii_uppercase()).unwrap();
        let (read, _) = read_request(&path).unwrap();
        assert_eq!((read.network.as_str(), read.dials.clone(), read.fee_rate), (req.network.as_str(), Dials::testnet(), crate::network::TESTNET_FEE_FLOOR));
        // a current request carries its own dials and fee floor
        std::fs::write(&path, faster_hex::hex_string(&borsh::to_vec(&req).unwrap()).to_ascii_uppercase()).unwrap();
        let (read, _) = read_request(&path).unwrap();
        assert_eq!((read.dials.clone(), read.fee_rate), (req.dials.clone(), req.fee_rate));
        assert!(check_request_network(&ctx, &read).is_ok());
        // a request made for mainnet is refused on this testnet folder, and the other way round
        let foreign = SignRequest { network: "mainnet".into(), ..req.clone() };
        let err = check_request_network(&ctx, &foreign).unwrap_err();
        assert!(err.contains("made for the mainnet network") && err.contains("--network testnet"), "{err}");
        let mut main_ctx = offline_ctx("old-main");
        main_ctx.network = Network::mainnet();
        assert!(check_request_network(&main_ctx, &req).unwrap_err().contains("made for the testnet network"));
        assert!(check_request_network(&main_ctx, &read).unwrap_err().contains("made for the testnet network"), "no tag counts as testnet");
        std::fs::remove_dir_all(&main_ctx.dir).ok();
        std::fs::remove_dir_all(&ctx.dir).ok();
    }

    #[test]
    fn fingerprint_is_sixteen_hex() {
        let fp = fingerprint(b"request bytes");
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(sha256_hex(b"request bytes").starts_with(&fp));
        assert_ne!(fingerprint(b"request bytes"), fingerprint(b"request byteS"));
    }
}
