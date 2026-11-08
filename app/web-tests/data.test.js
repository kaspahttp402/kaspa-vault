/* The page's data layer against a fake engine. Run from the repo root: node --test app/web-tests/data.test.js
   Each test is one rule the phone relies on; a failure names the rule in plain words. */
'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { load, tick, status } = require('./harness');

/* an engine that answers status for any account and ok to everything else */
function okEngine(over) {
  return (method, args) => method === 'status' ? status(Object.assign({ account: args.account || 1 }, over)) : { ok: true };
}

test('opening asks for one fingerprint, then reads the account', async () => {
  const h = load({ engine: okEngine() });
  const s = await h.open();
  assert.equal(h.prompts.length, 1);
  assert.equal(s.locked, false);
  assert.equal(s.balance, 120);
  assert.equal(h.callsOf('status')[0].args.account, 1);
});

test('a declined fingerprint keeps the account locked and asks the engine nothing', async () => {
  const h = load({ engine: okEngine(), fingerprint: 'no' });
  const s = await h.open();
  assert.equal(s.locked, true);
  assert.equal(h.calls.length, 0);
});

test('the last balances seen show at the next start, marked as updating', async () => {
  const h = load({ engine: okEngine(), settings: { last_balances: JSON.stringify({ balance: 77, limit: 20, left: 5, safe: 0, purse: 1 }) } });
  h.D.boot();
  assert.equal(h.D.state.balance, 77);
  assert.equal(h.D.state.stale, true);
  await h.D.retryUnlock(); await tick();
  assert.equal(h.D.state.balance, 120);
  assert.equal(h.D.state.stale, false);
  assert.equal(JSON.parse(h.settings.last_balances).balance, 120);
});

test('while the engine connects it says so and keeps the old numbers', async () => {
  const h = load({ engine: m => m === 'status' ? { status: 503, body: { connecting: true } } : { ok: true } });
  await h.open();
  assert.equal(h.D.state.connected, false);
  assert.match(h.D.state.error, /Connecting/);
});

test('an engine status marked stale is shown but not saved as the last good one', async () => {
  const h = load({ engine: okEngine({ stale: true, balance_kas: 50 }), settings: { last_balances: JSON.stringify({ balance: 77 }) } });
  await h.open();
  assert.equal(h.D.state.balance, 50);
  assert.equal(h.D.state.stale, true);
  assert.equal(h.D.state.connected, false);
  assert.equal(JSON.parse(h.settings.last_balances).balance, 77);
});

test('every money call names the account on screen', async () => {
  const h = load({ engine: okEngine({ accounts: [1, 2] }) });
  await h.open();
  await h.D.setAccount(2); await tick();
  assert.equal(h.callsOf('status').pop().args.account, 2);
  await h.D.send('kaspa:qdestdestdestdestdestdestdest0', 1.5);
  await h.D.topup(2);
  await h.D.stop();
  for (const m of ['send', 'topup', 'stop']) assert.equal(h.callsOf(m)[0].args.account, 2, m + ' carries account 2');
});

test('an account outside 1 to 8 is refused by the page', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  assert.equal(await h.D.setAccount(9), false);
  assert.equal(h.D.account, 1);
});

test('a send that is slid twice after a lost answer reuses its id, so it pays once', async () => {
  let n = 0;
  const h = load({ engine: (m, a) => m === 'status' ? status() : m === 'send' ? (++n === 1 ? { status: 0, body: '' } : { ok: true }) : { ok: true } });
  await h.open();
  await h.D.send('kaspa:qdestdestdestdestdestdestdest0', 1.5);
  await h.D.send('kaspa:qdestdestdestdestdestdestdest0', 1.5);
  const sends = h.callsOf('send');
  assert.equal(sends.length, 2);
  assert.equal(sends[0].args.id, sends[1].args.id);
});

test('no money moves without a fingerprint', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  h.setFingerprint('no');
  const r = await h.D.send('kaspa:qdestdestdestdestdestdestdest0', 1);
  assert.equal(r.ok, false);
  assert.equal(h.callsOf('send').length, 0);
});

test('a move that wants a fresh fingerprint asks once more and tries once more', async () => {
  let n = 0;
  const FRESH = 'This one needs your fingerprint again. Try once more.';
  const h = load({ engine: m => m === 'status' ? status() : m === 'takeout' ? (++n === 1 ? { ok: false, error: FRESH } : { ok: true }) : { ok: true } });
  await h.open();
  const before = h.prompts.length;
  const r = await h.D.takeOut([], '', 1);
  assert.equal(r.ok, true);
  assert.equal(h.callsOf('takeout').length, 2);
  assert.equal(h.prompts.length - before, 2);
});

test('a second fresh-fingerprint refusal stands, with no third try', async () => {
  const FRESH = 'This one needs your fingerprint again. Try once more.';
  const h = load({ engine: m => m === 'status' ? status() : m === 'takeout' ? { ok: false, error: FRESH } : { ok: true } });
  await h.open();
  const r = await h.D.takeOut([], '', 1);
  assert.equal(r.ok, false);
  assert.equal(h.callsOf('takeout').length, 2);
});

test('engine words that would confuse a person never reach the screen', async () => {
  const h = load({ engine: m => m === 'status' ? status() : { ok: false, error: 'utxo not found in script' } });
  await h.open();
  const r = await h.D.topup(1);
  assert.equal(r.ok, false);
  assert.doesNotMatch(r.error, /utxo|script/i);
});

test('the heir sheet sends an address as an address and a key as a key, and refuses anything else itself', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  await h.D.setHeir('kaspa:qheirheirheirheirheirheirheir0');
  await h.D.setHeir('AB'.repeat(32));
  const bad = await h.D.setHeir('not an address');
  const sets = h.callsOf('heir.set');
  assert.equal(sets.length, 2);
  assert.equal(sets[0].args.heir_address, 'kaspa:qheirheirheirheirheirheirheir0');
  assert.equal(sets[1].args.heir_x, 'ab'.repeat(32));
  assert.equal(bad.ok, false);
});

test('changing the rules needs no fingerprint and names the account', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  const before = h.prompts.length;
  await h.D.changeRules({ cap_kas: 50, delay: null });
  const c = h.callsOf('rules.change')[0];
  assert.equal(h.prompts.length, before);
  assert.equal(c.args.account, 1);
  assert.equal(c.args.cap_kas, 50);
  assert.equal('delay' in c.args, false);
});

test('a record this app cannot read is not shown as an unopened account', async () => {
  const h = load({ engine: okEngine({ old_rules: true, notice: 'This account was made by an older version.', stopped: false, balance_kas: 0, safe_kas: 0 }) });
  await h.open();
  assert.equal(h.D.state.oldRules, true);
  assert.equal(h.D.state.unopened, false);
});

test('pay from the purse sends the address and leaves the amount out for everything', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  await h.D.pursePay('kaspa:qdestdestdestdestdestdestdest0', 0);
  const p = h.callsOf('purse.pay')[0];
  assert.equal(p.args.to, 'kaspa:qdestdestdestdestdestdestdest0');
  assert.equal('kas' in p.args, false);
});

test('money can move only on a fresh network answer, whatever the phase says', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  assert.equal(h.D.canMove(), true);
  h.setEngine(okEngine({ stale: true, stopped: true, balance_kas: 0, safe_kas: 40 }));
  await h.D.setAccount(1); await tick();
  assert.equal(h.D.phase(), 'stopped');
  assert.equal(h.D.canMove(), false);
  h.setEngine(m => m === 'status' ? { status: 503, body: { connecting: true } } : { ok: true });
  await h.D.setAccount(1); await tick();
  assert.equal(h.D.canMove(), false);
});

/* the one state model (added by the phase refactor); skipped until KVData.phase exists */
test('one phase per situation, in order', { skip: !load().D.phase && 'KVData.phase not added yet' }, async () => {
  const h = load({ engine: okEngine() });
  h.D.boot();
  assert.equal(h.D.phase(), 'locked');
  await h.D.retryUnlock(); await tick();
  assert.equal(h.D.phase(), 'open');
  h.setEngine(m => m === 'status' ? { status: 200, body: { ok: false, error: 'no keys yet' } } : { ok: true });
  await h.D.setAccount(1); await tick();
  assert.equal(h.D.phase(), 'noKeys');
  h.setEngine(okEngine({ stopped: true, balance_kas: 0, safe_kas: 0 }));
  await h.D.setAccount(1); await tick();
  assert.equal(h.D.phase(), 'unopened');
  h.setEngine(okEngine({ stopped: true, balance_kas: 0, safe_kas: 40 }));
  await h.D.setAccount(1); await tick();
  assert.equal(h.D.phase(), 'stopped');
  h.setEngine(okEngine({ rules_next: { dials: {}, words: ['20 KAS a day'] } }));
  await h.D.setAccount(1); await tick();
  assert.equal(h.D.phase(), 'rulesNext');
});

/* a rule change on this phone: the money moves to the purse, then finish, stop and fund in that order */
const PURSE = 'kaspa:qpursepursepursepursepursepurse0';
function rulesEngine(over) {
  const st = { pending: { to: PURSE, amount_kas: 100, seconds_left: 0 }, stopped: false };
  return (method, args) => {
    if (over && over[method]) return over[method](args);
    if (method === 'status')
      return status({ account: args.account || 1, rules_next: { words: ['Daily limit: 50 KAS'] }, pending: st.pending, stopped: st.stopped });
    if (method === 'finish') st.pending = null;
    if (method === 'stop') st.stopped = true;
    if (method === 'funding') return { ok: true, kas: 100.5, address: PURSE };
    return { ok: true };
  };
}

test('a rule change that waits its notice sends the move to the purse, from this account', async () => {
  const h = load({ engine: okEngine() });
  await h.open();
  const r = await h.D.moveRules({ ok: true, route: 'move', move_kas: 118.9, to: PURSE, kind: 'notice' });
  assert.equal(r.ok, true);
  const s = h.callsOf('send')[0];
  assert.equal(s.args.to, PURSE);
  assert.equal(s.args.kas, 118.9);
  assert.equal(s.args.account, 1);
});

test('finishing a rule change runs finish, stop, funding, fund in order, and fund carries no rules', async () => {
  const h = load({ engine: rulesEngine() });
  await h.open();
  assert.equal(h.D.rulesReady(), true);
  const r = await h.D.finishRules();
  assert.equal(r.ok, true);
  const order = h.calls.map(c => c.method).filter(m => m !== 'status');
  assert.deepEqual(order, ['finish', 'stop', 'funding', 'fund']);
  const f = h.callsOf('fund')[0].args;
  assert.deepEqual(Object.keys(f).sort(), ['account', 'kas']);
  assert.equal(f.kas, 100.45);
  assert.equal(f.account, 1);
});

test('a refusal at the stop ends the rule change there, and the next try skips what is done', async () => {
  let refuse = true;
  const h = load({ engine: rulesEngine({ stop: () => (refuse ? { ok: false, error: 'The network is busy. Try again in a minute.' } : undefined) }) });
  await h.open();
  const r = await h.D.finishRules();
  assert.equal(r.ok, false);
  assert.equal(r.error, 'The network is busy. Try again in a minute.');
  assert.equal(h.callsOf('funding').length, 0);
  assert.equal(h.callsOf('fund').length, 0);
  refuse = false;
  h.setEngine(rulesEngine());
  await h.D.finishRules();
  assert.equal(h.callsOf('finish').length, 1);
});

const NOT_YET = "The account's money has not reached your safe yet. Trying again in a moment.";
function nowEngine(over) {
  const st = { stopped: false, tries: 0 };
  return (method, args) => {
    if (over && over[method]) {
      const x = over[method](args, st);
      if (x) return x;
    }
    if (method === 'status')
      return status({ account: args.account || 1, rules_next: { words: ['Daily limit: 50 KAS'] }, pending: null, stopped: st.stopped });
    if (method === 'stop') st.stopped = true;
    if (method === 'takeout') return ++st.tries === 1 ? { ok: false, error: NOT_YET } : { ok: true, kas: 100, open_kas: 99.9 };
    return { ok: true };
  };
}

test('update now runs stop, take-out (waiting while the safe has not seen the stop), then reopens with what came out', async () => {
  const h = load({ engine: nowEngine() });
  await h.open();
  const steps = [];
  const r = await h.D.updateNow((s, w) => steps.push(w || s));
  assert.equal(r.ok, true);
  assert.deepEqual(h.calls.map(c => c.method).filter(m => m !== 'status'), ['stop', 'takeout', 'takeout', 'fund']);
  assert.equal(h.callsOf('takeout')[0].args.from_stop, true);
  assert.deepEqual(h.callsOf('fund')[0].args, { kas: 99.9, account: 1 });
  assert.deepEqual(steps, ['stop', 'takeout', NOT_YET, 'fund']);
});

test('an update now refused at the reopen resumes there from Home: no second stop or take-out', async () => {
  let refuse = true;
  const h = load({ engine: nowEngine({ fund: () => (refuse ? { ok: false, error: 'The network is busy. Try again in a minute.' } : null) }) });
  await h.open();
  const r = await h.D.updateNow();
  assert.equal(r.ok, false);
  assert.equal(h.D.rulesReady(), true);
  refuse = false;
  const r2 = await h.D.finishRules();
  assert.equal(r2.ok, true);
  assert.equal(h.callsOf('stop').length, 1);
  assert.equal(h.callsOf('takeout').length, 2);
  assert.deepEqual(h.callsOf('fund').map(c => c.args.kas), [99.9, 99.9]);
});

test('a rule change whose payment was stopped just waits: no finish offered', async () => {
  const h = load({ engine: okEngine({ rules_next: { words: [] }, pending: null }) });
  await h.open();
  assert.equal(h.D.rulesPending(), null);
  assert.equal(h.D.rulesReady(), false);
});
