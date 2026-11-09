//! `watch --auto-stop` (the public challenge): the racer that answers every waiting withdrawal by
//! itself. A child of watch.rs so it shares the Watcher (judge, alerts) without widening it; the
//! decision and the transaction live in autostop.rs.

use super::*;
use crate::autostop::{Choice, Ledger};

/// Bank words for the racer's alerts (alerts.json; the phone shows them).
const GAVE_UP: &str =
    "The automatic emergency stop could not get through. Press the emergency stop now.";
const NOT_RUN: &str = "The automatic emergency stop could not run. Press the emergency stop now.";
const FAILED_ONCE: &str = "The automatic emergency stop failed once and will try again. \
     Press the emergency stop if you can.";
const DECLINED: &str =
    "A withdrawal request was declined automatically; the account is Locked again.";
const CANCELLED: &str = "The automatic emergency stop sent everything to the vault.";
const RACER_DOWN: &str = "The automatic emergency stop stopped. Press the emergency stop yourself.";

impl Watcher<'_> {
    /// `--auto-stop`: whether a stop for the waiting coin `c` may go out now: Some(sends so far),
    /// or
    /// None while one is in flight (RETRY, still in the mempool) or after TRIES_PER_COIN sends.
    async fn stop_due(&mut self, c: &Coin) -> Option<u32> {
        let Some((id, op, at, tries)) = self.stop_sent.clone() else { return Some(0) };
        if op != c.0 {
            return Some(0);
        }
        if id.is_empty() && tries < autostop::TRIES_PER_COIN {
            return (at.elapsed() >= autostop::FAIL_RETRY).then_some(tries);
        }
        if at.elapsed() < autostop::RETRY || self.open_in_mempool(&c.0).await {
            return None;
        }
        if tries >= autostop::TRIES_PER_COIN {
            if tries == autostop::TRIES_PER_COIN {
                self.stop_sent = Some((id, op, at, tries + 1));
                let raw = format!(
                    "AUTO-STOP GAVE UP after {tries} sends on the waiting coin {op}; run `cancel` by hand now"
                );
                self.alert(Kind::WithdrawalPending, GAVE_UP, &raw);
            }
            return None;
        }
        let again = "is gone from the mempool and the waiting coin is still here; sending again";
        self.say(Level::Info, &format!("auto-stop: tx {id} {again}"));
        Some(tries)
    }

    /// `--auto-stop` racer, one tick (every second, the primary node only, its own state and coin):
    /// a
    /// waiting withdrawal is answered the moment it shows, never behind the careful multi-node
    /// follow.
    /// Safe to be wrong: a stop for a request that is not real is refused by the network. True:
    /// closed.
    async fn race_tick(&mut self) -> bool {
        let rpc = self.ctx.rpc();
        let Ok(info) = rpc.get_block_dag_info().await else { return false };
        let addr = self.ctx.vault_address(&self.state);
        let Ok((ours, _)) = fetch_coins(rpc, &addr, self.cov).await else { return false };
        let Some(c) = self.coin.clone() else {
            self.race_find(ours, info.sink);
            return false;
        };
        if let Some(now) = ours.into_iter().find(|(op, _)| *op == c.0) {
            self.race_low = Some(info.sink);
            self.coin = Some(now.clone());
            if self.state.stage == 1 || autostop::over(self.race_end, unix_now()) {
                let state = self.state.clone();
                self.stop_now(&now, state).await;
            } else if let Some(tx) = self.mempool_spender(&now).await {
                return self.race_move(tx, &now).await;
            }
            return false;
        }
        match self.block_spender(&c).await {
            Some(tx) => self.race_move(tx, &c).await,
            None => {
                self.race_lost += 1;
                if self.race_lost >= autostop::RACE_LOST {
                    self.coin = None;
                }
                false
            }
        }
    }

    /// No coin followed: adopt the one coin at the state's address, or every RACE_LOST ticks take
    /// the
    /// state from state.json (the careful watcher keeps it current) in case this racer fell behind.
    fn race_find(&mut self, ours: Vec<Coin>, sink: Hash) {
        if let [one] = ours.as_slice() {
            let (amount, stage) = (kas(one.1.amount), self.state.stage);
            let coin = one.0;
            let line = format!("auto-stop: following {coin} of {amount:.4} KAS (stage {stage})");
            self.say(Level::Info, &line);
            (self.coin, self.race_low, self.race_lost) = (Some(one.clone()), Some(sink), 0);
            return;
        }
        self.race_lost += 1;
        if self.race_lost % autostop::RACE_LOST != 0 {
            return;
        }
        if let Ok(Some(v)) = self.ctx.try_load_vault() {
            if v.covenant_id == self.cov && v.state != self.state {
                let stage = v.state.stage;
                let line = format!("auto-stop: coin not found; state.json says stage {stage}");
                self.say(Level::Info, &line);
                self.state = v.state;
            }
        }
    }

    /// A spender of the followed coin `c` was found: follow it. A request is answered at once from
    /// the
    /// request transaction itself (its output 0 is the waiting coin). True when the account closed.
    async fn race_move(&mut self, tx: RpcTransaction, c: &Coin) -> bool {
        let o = self.judge(&tx, c);
        let Ok(t) = Transaction::try_from(tx) else { return false };
        self.race_lost = 0;
        self.coin = None;
        if o.closed {
            let line = format!("auto-stop: the account closed ({}, tx {})", o.entry, t.id());
            self.say(Level::Info, &line);
            return true;
        }
        let Some(next) = o.next.clone() else { return false };
        self.state = next.clone();
        if let Err(e) = autostop::save_race(&self.ctx.dir, &self.cov.to_string(), &next) {
            self.say(Level::Info, &format!("auto-stop: {e}"));
        }
        if o.entry != "request" {
            return false;
        }
        let Some(out) = t.outputs.first() else { return false };
        let spk = out.script_public_key.clone();
        let entry = UtxoEntry::new(out.value, spk, 0, false, Some(self.cov));
        let pending: Coin = (TransactionOutpoint::new(t.id(), 0), entry);
        self.coin = Some(pending.clone());
        let line = format!("auto-stop: request {} seen on {}; answering now", t.id(), self.primary);
        self.say(Level::Info, &line);
        self.stop_now(&pending, next).await;
        false
    }

    /// The primary node's mempool: a transaction spending `c`, if one waits there.
    async fn mempool_spender(&self, c: &Coin) -> Option<RpcTransaction> {
        let addr = self.ctx.vault_address(&self.state);
        let rpc = self.ctx.rpc();
        let entries = rpc.get_mempool_entries_by_addresses(vec![addr], false, false).await.ok()?;
        let mut sending = entries.into_iter().flat_map(|e| e.sending);
        sending.find(|m| spends(&m.transaction, &c.0)).map(|m| m.transaction)
    }

    /// The primary node's blocks since the coin was last seen: the transaction that spent `c`.
    async fn block_spender(&self, c: &Coin) -> Option<RpcTransaction> {
        let mut page = self.race_low?;
        for n in 1..=autostop::RACE_PAGES {
            let r = self.ctx.rpc().get_blocks(Some(page), true, true).await.ok()?;
            let mut txs = r.blocks.into_iter().flat_map(|b| b.transactions);
            if let Some(t) = txs.find(|t| spends(t, &c.0)) {
                return Some(t);
            }
            match step(&r.block_hashes, page, n) {
                Step::Next(h) => page = h,
                _ => return None,
            }
        }
        None
    }

    /// Sends the answer for the waiting coin `c` whose state is `state` (Pending), once per coin,
    /// capped.
    async fn stop_now(&mut self, c: &Coin, state: State) {
        let Some(a) = self.auto_stop.clone() else { return };
        let Some(tries) = self.stop_due(c).await else { return };
        self.stop_sent = Some((String::new(), c.0, Instant::now(), tries + 1));
        let mut v = match self.ctx.try_load_vault() {
            Ok(Some(v)) if v.covenant_id == self.cov => v,
            other => {
                let missing = "state.json is missing or names another account";
                let why = other.err().unwrap_or_else(|| missing.into());
                self.alert(Kind::WithdrawalPending, NOT_RUN, &format!("AUTO-STOP not sent: {why}"));
                return;
            }
        };
        let asked = state.amount.max(0) as u64;
        v.state = state;
        let mut ledger = Ledger::load(&self.ctx.dir);
        let now = unix_now();
        let mut choice = match autostop::over(self.race_end, now) {
            true => Choice::Cancel,
            false => autostop::choose(ledger.recent(now, a.window_secs), a.cap),
        };
        let mut sent = autostop::send(self.ctx, &v, c, choice, a.which).await;
        if choice == Choice::Decline {
            if let Err(autostop::SendErr::Build(e)) = &sent {
                let note =
                    format!("auto-stop: a decline cannot be built ({e}); sending cancel instead");
                self.say(Level::Info, &note);
                choice = Choice::Cancel;
                sent = autostop::send(self.ctx, &v, c, choice, a.which).await;
            }
        }
        match sent {
            Ok((id, fee)) => {
                self.stop_landed(&a, &mut ledger, (choice, id, fee, asked), c, now, tries)
            }
            Err(e) => {
                let (what, secs) = (choice.entry(), autostop::FAIL_RETRY.as_secs());
                let raw = format!("AUTO-STOP {what} not sent: {e}; trying again in {secs}s");
                self.alert(Kind::WithdrawalPending, FAILED_ONCE, &raw);
            }
        }
    }

    /// A stop went out: the ledger, the proof file line with both explorer links, and the alert.
    fn stop_landed(
        &mut self,
        a: &AutoStop,
        ledger: &mut Ledger,
        sent: (Choice, String, u64, u64),
        c: &Coin,
        now: u64,
        tries: u32,
    ) {
        let (choice, id, fee, asked) = sent;
        self.stop_sent = Some((id.clone(), c.0, Instant::now(), tries + 1));
        match choice {
            Choice::Decline => ledger.note_decline(now, a.window_secs),
            Choice::Cancel => ledger.cancel = Some(id.clone()),
        }
        let count = ledger.recent(now, a.window_secs);
        let asked_kas = format!("{:.8} KAS", kas(asked));
        let attack = c.0.transaction_id.to_string();
        // asked 0: no request was waiting: the end of the challenge (everything to the vault)
        let line = match asked {
            0 => autostop::end_line(&stamp(), &id, fee, &a.explorer),
            _ => autostop::proof_line(&stamp(), &attack, &asked_kas, choice, &id, fee, &a.explorer),
        };
        let round = autostop::Round {
            at: now,
            attack: attack.clone(),
            asked,
            answer_kind: choice.entry().into(),
            answer: id.clone(),
            fee,
        };
        let dir = &self.ctx.dir;
        let round = if asked == 0 { Ok(()) } else { autostop::append_round(dir, &round) };
        let saves = [ledger.save(dir), autostop::append_proof(dir, &line), round];
        for r in saves {
            if let Err(e) = r {
                self.say(Level::Info, &format!("auto-stop: {e}"));
            }
        }
        let link = autostop::tx_link(&a.explorer, &id);
        let cap = a.cap.map(|c| c.to_string()).unwrap_or_else(|| "no cap".into());
        let raw = format!(
            "AUTO-STOP: {} sent with stop key {}, fee {:.8} KAS, tx {id} ({link}); declines in the \
             window: {count} ({cap})",
            choice.entry(),
            a.which,
            kas(fee),
        );
        match choice {
            Choice::Decline => self.alert(Kind::WithdrawalDeclined, DECLINED, &raw),
            Choice::Cancel => self.alert(Kind::Stopped, CANCELLED, &raw),
        }
    }
}

/// `--auto-stop`: the racer task beside the guard. Its own Watcher on the primary node, no follow,
/// no
/// other nodes: it only answers waiting withdrawals, every second, until the account closes or the
/// guard is stopped. A panic here is an alert, never silence.
pub(super) async fn race(
    ctx: Ctx,
    cov: Hash,
    state: State,
    primary: String,
    a: AutoStop,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut r = Watcher::new(&ctx, cov, state, primary, vec![]);
    r.follow = false;
    if let Some(saved) = autostop::load_race(&ctx.dir, &cov.to_string()) {
        r.state = saved;
    }
    let mut ledger = Ledger::load(&ctx.dir);
    r.race_end = ledger.fix_end(unix_now(), a.end_after_secs);
    if let Err(e) = ledger.save(&ctx.dir) {
        r.say(Level::Info, &format!("auto-stop: {e}"));
    }
    if let Some(t) = r.race_end {
        let hours = t.saturating_sub(unix_now()) as f64 / 3600.0;
        let then = "then everything goes to the vault";
        let line = format!("auto-stop: the challenge ends in {hours:.1} h (unix {t}); {then}");
        r.say(Level::Info, &line);
    }
    r.auto_stop = Some(a);
    let (on, stage) = (&r.primary, r.state.stage);
    let line = format!("auto-stop racer started: every second on {on} (stage {stage})");
    r.say(Level::Info, &line);
    loop {
        match AssertUnwindSafe(r.race_tick()).catch_unwind().await {
            Ok(true) => return,
            Ok(false) => {}
            Err(p) => {
                let raw = format!("AUTO-STOP DOWN: panic: {}", panic_text(&*p));
                r.alert(Kind::WatcherSilent, RACER_DOWN, &raw);
                return;
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return;
                }
            }
        }
    }
}
