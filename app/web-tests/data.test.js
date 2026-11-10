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

test('amounts: at most 8 decimals and never 1e-7, the same shown (fmt) and sent', async () => {
  const h = load({ engine: okEngine() });
  const k = h.D.kasText;
  assert.equal(k(1e-7), '0.0000001');
  assert.equal(k(123.456789012), '123.45678901');
  assert.equal(k(0.1 + 0.2), '0.3');
  assert.equal(k('12.50'), '12.5');
  assert.equal(k(5), '5');
  await h.open();
  await h.D.send('kaspa:qtest00000000000000000000', 1e-7, 'x');
  assert.equal(h.callsOf('send')[0].args.kas, '0.0000001');
});

test('an amount box takes ".5" and "5.", and refuses more than 8 decimals', () => {
  const D = load({ engine: okEngine() }).D;
  assert.equal(D.amountOf('.5'), 0.5);
  assert.equal(D.amountOf('5.'), 5);
  assert.equal(D.amountOf(''), 0);
  assert.ok(Number.isNaN(D.amountOf('1.123456789')));
  assert.ok(Number.isNaN(D.amountOf('0')));
  assert.ok(Number.isNaN(D.amountOf('1e-7')));
});

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
  const FRESH = 'Fingerprint confirmation required. Please try again.';
  const h = load({ engine: m => m === 'status' ? status() : m === 'takeout' ? (++n === 1 ? { ok: false, error: FRESH } : { ok: true }) : { ok: true } });
  await h.open();
  const before = h.prompts.length;
  const r = await h.D.takeOut([], '', 1);
  assert.equal(r.ok, true);
  assert.equal(h.callsOf('takeout').length, 2);
  assert.equal(h.prompts.length - before, 2);
});

test('a second fresh-fingerprint refusal stands, with no third try', async () => {
  const FRESH = 'Fingerprint confirmation required. Please try again.';
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
  assert.match(h.prompts[h.prompts.length - 1].why, /^Pay everything /);
  await h.D.pursePay('kaspa:qdestdestdestdestdestdestdest0', 1.5);
  assert.match(h.prompts[h.prompts.length - 1].why, /^Pay 1\.5 KAS /, 'the prompt names the sum');
  assert.equal(h.callsOf('purse.pay')[1].args.kas, '1.5');
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
  assert.equal(s.args.kas, '118.9');
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
  assert.equal(f.kas, '100.45');
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

const NOT_YET = "The account's funds have not reached your vault yet. Trying again in a moment.";
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
  assert.deepEqual(h.callsOf('fund')[0].args, { kas: '99.9', account: 1 });
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
  assert.deepEqual(h.callsOf('fund').map(c => c.args.kas), ['99.9', '99.9']);
});

test('a rule change whose payment was stopped just waits: no finish offered', async () => {
  const h = load({ engine: okEngine({ rules_next: { words: [] }, pending: null }) });
  await h.open();
  assert.equal(h.D.rulesPending(), null);
  assert.equal(h.D.rulesReady(), false);
});

/* the heir's own purse (heir.purse keyless, heir.pay with this phone's heir key): not an account,
   so no account number */
function heirEngine() {
  return (m, a) => m === 'status' ? status() : m === 'heir.purse' ? { ok: true,
    address: 'kaspa:qheir', kas: 12.5 } : { ok: true, tx: 'ab' };
}

test('the heir purse is read without a fingerprint', async () => {
  const h = load({ engine: heirEngine() });
  await h.open();
  const before = h.prompts.length;
  const r = await h.D.heirPurse();
  assert.equal(r.kas, 12.5);
  assert.equal(h.callsOf('heir.purse').length, 1);
  assert.equal(h.prompts.length, before);
});

test('paying from the heir purse asks one fingerprint and names no account', async () => {
  const h = load({ engine: heirEngine() });
  await h.open();
  const before = h.prompts.length;
  const r = await h.D.heirPay('kaspa:qto', 2);
  assert.equal(r.ok, true);
  assert.equal(h.prompts.length - before, 1);
  assert.deepEqual(h.callsOf('heir.pay')[0].args, { to: 'kaspa:qto', kas: '2' });
  assert.match(h.prompts[h.prompts.length - 1].why, /^Send 2 KAS /, 'the prompt names the sum');
});

test('paying all of the heir purse sends no amount, so the engine takes the fee off the top',
  async () => {
  const h = load({ engine: heirEngine() });
  await h.open();
  await h.D.heirPay('kaspa:qto', 0);
  assert.deepEqual(h.callsOf('heir.pay')[0].args, { to: 'kaspa:qto' });
});

test('a fresh phone is recognised by the engine flag or by either wording, and is not shown as ' +
  'an error', async () => {
  for (const refusal of [{ ok: false, no_keys: true, error: 'Anything' }, { ok: false,
    error: 'This account is not set up yet. Complete setup first.' }, { ok: false,
    error: 'This phone has no keys yet.' }]) {
    const h = load({ engine: (m) => m === 'status' ? refusal : { ok: true } });
    await h.open();
    assert.equal(h.D.state.noKeys, true, JSON.stringify(refusal));
    assert.equal(h.D.state.error, '');
  }
});

test("after a guardian's stop, update now takes the funds out and reopens the account (no rule " +
  "change waiting)", async () => {
  const h = load({ engine: (m, a) => m === 'status' ? status({ account: a.account || 1,
    stopped: true, rules_next: null })
    : m === 'takeout' ? { ok: true, kas: 20, open_kas: 19.9 } : { ok: true } });
  await h.open();
  const r = await h.D.updateNow();
  assert.equal(r.ok, true);
  assert.deepEqual(h.calls.map(c => c.method).filter(m => m !== 'status'), ['takeout', 'fund']);
  assert.deepEqual(h.callsOf('fund')[0].args, { kas: '19.9', account: 1 });
});

/* ---- one rule per engine call that had no test yet ---- */
const LONG = 'x'.repeat(4097);
async function opened(engine) {
  const h = load({ engine: engine || okEngine() });
  await h.open();
  return { h, before: h.prompts.length };
}

test('showing the seeds asks one fingerprint and calls keys.sheets', async () => {
  const { h, before } = await opened();
  const r = await h.D.sheets();
  assert.equal(r.ok, true);
  assert.equal(h.callsOf('keys.sheets').length, 1);
  assert.equal(h.prompts.length - before, 1);
});

test('deleting the seeds asks one fingerprint and sends confirm', async () => {
  const { h, before } = await opened();
  await h.D.burnSheets();
  assert.equal(h.callsOf('keys.burn')[0].args.confirm, true);
  assert.equal(h.prompts.length - before, 1);
});

test('replacing the keys asks one fingerprint and sends confirm (leave_safe only when asked)',
  async () => {
  const { h, before } = await opened();
  await h.D.resetKeys(false);
  await h.D.resetKeys(true);
  const c = h.callsOf('keys.reset');
  assert.equal(c[0].args.confirm, true);
  assert.equal(c[0].args.leave_safe, undefined);
  assert.equal(c[1].args.leave_safe, true);
  assert.equal(h.prompts.length - before, 2);
});

test('declining a withdrawal asks one fingerprint and names the account', async () => {
  const { h, before } = await opened(okEngine({ pending: { to: 'kaspa:qto', amount_kas: 5,
    seconds_left: 100 } }));
  await h.D.declinePending();
  assert.equal(h.callsOf('decline').length, 1);
  assert.equal(h.callsOf('decline')[0].args.account, 1);
  assert.equal(h.prompts.length - before, 1);
});

test('the link code is asked for without a fingerprint', async () => {
  const { h, before } = await opened();
  const r = await h.D.linkCode();
  assert.equal(r.ok, true);
  assert.equal(h.callsOf('link.code').length, 1);
  assert.equal(h.prompts.length, before);
});

test('reading a link code sends the text without a fingerprint', async () => {
  const { h, before } = await opened();
  await h.D.readLink('abc');
  assert.deepEqual(h.callsOf('link.read')[0].args, { text: 'abc' });
  assert.equal(h.prompts.length, before);
});

test('a link code over 4096 characters is refused by the page, the engine is not asked', async () =>
  {
  const { h } = await opened();
  const r = await h.D.readLink(LONG);
  assert.equal(r.ok, false);
  assert.equal(h.callsOf('link.read').length, 0);
});

test('a watch code over 4096 characters is refused by the page, the engine is not asked',
  async () => {
  const { h } = await opened();
  const r = await h.D.watchImport(LONG);
  assert.equal(r.ok, false);
  assert.equal(h.callsOf('watch.import').length, 0);
});

test('a watch code is imported without a fingerprint, then the watched list is read', async () => {
  const { h, before } = await opened();
  const r = await h.D.watchImport('code');
  await tick();
  assert.equal(r.ok, true);
  assert.deepEqual(h.callsOf('watch.import')[0].args, { text: 'code' });
  assert.ok(h.callsOf('watch.list').length >= 1);
  assert.equal(h.prompts.length, before);
});

function watchEngine() {
  return (method, args) => {
    if (method === 'status') return status({ account: args.account || 1 });
    if (method === 'watch.list') return { ok: true, watched: [{ id: 'w1', name: 'Mum' }] };
    if (method === 'watch.status') return { ok: true, balance_kas: 9 };
    return { ok: true };
  };
}

test('the watched accounts are listed and each one read without a fingerprint', async () => {
  const { h, before } = await opened(watchEngine());
  const list = await h.D.loadWatched();
  assert.equal(list.length, 1);
  assert.deepEqual(h.callsOf('watch.status').pop().args, { id: 'w1' });
  assert.equal(list[0].st.balance_kas, 9);
  assert.equal(h.prompts.length, before);
});

test('forgetting a watched account needs no fingerprint and sends its id', async () => {
  const { h, before } = await opened(watchEngine());
  await h.D.watchForget('w1');
  assert.deepEqual(h.callsOf('watch.forget')[0].args, { id: 'w1' });
  assert.equal(h.prompts.length, before);
});

for (const [fn, method] of [['watchStop', 'watch.stop'], ['watchDecline', 'watch.decline'],
  ['watchClaim', 'watch.claim']]) {
  test(`a guardian or heir move (${method}) asks one fingerprint and sends only ` +
    'the id', async () => {
    const { h, before } = await opened(watchEngine());
    const r = await h.D[fn]('w1');
    assert.equal(r.ok, true);
    assert.deepEqual(h.callsOf(method)[0].args, { id: 'w1' });
    assert.equal(h.prompts.length - before, 1);
  });
}

test('the watch code is made by guard.export for this account, without a fingerprint', async () => {
  const eng = (method, args) => method === 'status' ? status({ account: args.account || 1 }) :
    method === 'guard.export' ? { kind: 'kv-watch', v: 1 } : { ok: true };
  const { h, before } = await opened(eng);
  const r = await h.D.watchCode('Me');
  assert.equal(r.ok, true);
  assert.deepEqual(JSON.parse(r.code), { kind: 'kv-watch', v: 1 });
  assert.deepEqual(h.callsOf('guard.export')[0].args, { account: 1, name: 'Me' });
  assert.equal(h.prompts.length, before);
});

/* ---- security fixes from docs/surface/page-review.md ---- */
test('one prompt at a time: a second money call is refused, the first completes', async () => {
  const { h, before } = await opened();
  const [a, b] = await Promise.all([h.D.stop(), h.D.send('kaspa:qtest', 1)]);
  assert.equal(a.ok, true);
  assert.equal(b.ok, false);
  assert.equal(b.error, 'Fingerprint needed. Try again when the sensor is ready.');
  assert.equal(h.prompts.length, before + 1);
  assert.equal(h.callsOf('stop').length, 1);
  assert.equal(h.callsOf('send').length, 0);
  const c = await h.D.send('kaspa:qtest', 1); /* the prompt is free again afterwards */
  assert.equal(c.ok, true);
});

test('link code and helper names: no control or direction marks, 40 at most', async () => {
  const evil = 'Sam\u202e\u2066\n\u200flivE' + 'x'.repeat(60);
  const eng = (m, a) => (m === 'link.read' ? { ok: true, name: evil } : okEngine()(m, a));
  const h = load({ engine: eng, settings: { helper_names: JSON.stringify({ bb: evil }) } });
  await h.open();
  const r = await h.D.readLink('abc');
  for (const n of [r.name, h.D.helperNames().bb]) {
    assert.equal(/[\u0000-\u001f\u200e\u200f\u202a-\u202e\u2066-\u2069]/.test(n), false);
    assert.equal(n.length, 40);
    assert.equal(n.slice(0, 7), 'SamlivE');
    assert.equal(n.slice(-1), '\u2026');
  }
});

test('a take-out marker on an account open again starts afresh: stop, take-out, fund', async () => {
  const left = { account: 1, kas: 0, now: true, out: true, open: 5 };
  const h = load({ engine: nowEngine(), settings: { rules_move: JSON.stringify(left) } });
  await h.open();
  const r = await h.D.updateNow();
  assert.equal(r.ok, true);
  const moves = h.calls.map((c) => c.method).filter((m) => m !== 'status');
  assert.deepEqual(moves, ['stop', 'takeout', 'takeout', 'fund']);
  assert.deepEqual(h.callsOf('fund')[0].args, { kas: '99.9', account: 1 });
});

/* ---- the paper stop (docs/PAPER_STOP.md) ---- */
const PAPER = ('zebra zero zone zoo youth young yellow yard wrist wrong wolf wisdom winter wink'
  + ' wing wine window width wide whisper wheat whale weasel walnut').split(' ');
function paperEngine(over) {
  const o = over || {};
  return (method, args) => {
    if (method === 'status') return status({ account: args.account || 1 });
    if (method === 'paper.make') return { ok: true, words: PAPER.slice(), slot: 2, error: null };
    if (method === 'paper.confirm') return { ok: true, route: 'move', steps: [], slot: 2,
      rule_words: ['Emergency stop button 2 becomes your paper stop.'], error: null };
    if (o[method]) return o[method];
    if (method === 'paper.sheet') return { ok: true, watch_code: o.code ? o.code() : 'code-1' };
    if (method === 'paper.check')
      return { ok: true, kas: 42.5, hops: 3, live: true, stage: 'open' };
    if (method === 'paper.stop') return { ok: true, tx: 'ab', kas: 42.5, hops: 3 };
    const stops = [{ which: 2, holder: o.holder || 'paper' }];
    if (method === 'keys.public') return { ok: true, public: {}, stops };
    return { ok: true };
  };
}

test('making a paper stop asks one fingerprint, names the account, answers 24 words', async () => {
  const { h, before } = await opened(paperEngine());
  const r = await h.D.paperMake();
  assert.equal(r.ok, true);
  assert.equal(r.words.length, 24);
  assert.equal(h.callsOf('paper.make')[0].args.account, 1);
  assert.equal(h.prompts.length - before, 1);
});

test('the code to print and the account check need no fingerprint', async () => {
  const { h, before } = await opened(paperEngine());
  assert.equal((await h.D.paperSheet()).watch_code, 'code-1');
  assert.equal(h.callsOf('paper.sheet')[0].args.account, 1);
  assert.equal((await h.D.paperCheck(' code-1 ')).kas, 42.5);
  assert.deepEqual(h.callsOf('paper.check')[0].args, { watch_code: 'code-1' });
  assert.equal(h.prompts.length, before);
});

test('a paper stop needs no fingerprint, sends the words once, no setting keeps one', async () => {
  const { h, before } = await opened(paperEngine());
  const words = PAPER.slice();
  const r = await h.D.paperStop(words, 'code-1');
  assert.equal(r.ok, true);
  assert.equal(h.prompts.length, before);
  const c = h.callsOf('paper.stop');
  assert.equal(c.length, 1);
  assert.deepEqual(c[0].args, { words: PAPER.join(' '), watch_code: 'code-1' });
  assert.equal(words.length, 0, "the caller's array is emptied");
  const kept = new Set(JSON.stringify(h.settings).toLowerCase().match(/[a-z]+/g));
  assert.deepEqual(PAPER.filter((w) => kept.has(w)), []);
});

test('a paper stop without 24 words or a code asks the engine nothing', async () => {
  const { h } = await opened(paperEngine());
  const short = PAPER.slice(0, 23);
  assert.equal((await h.D.paperStop(short, 'code-1')).ok, false);
  assert.equal(short.length, 0);
  assert.equal((await h.D.paperStop(PAPER.slice(), '')).ok, false);
  assert.equal((await h.D.paperCheck('x'.repeat(4097))).ok, false);
  assert.equal(h.callsOf('paper.stop').length + h.callsOf('paper.check').length, 0);
});

test('the paper stop asks to be printed again only after the watch code changed', async () => {
  let code = 'code-1';
  const { h } = await opened(paperEngine({ code: () => code }));
  assert.equal(await h.D.paperDue(), true);
  h.D.paperShown('code-1');
  assert.match(h.settings.paper_shown, /^[0-9a-f]{8}$/);
  assert.equal(await h.D.paperDue(), false);
  code = 'code-2';
  assert.equal(await h.D.paperDue(), true);
});

test('without a paper stop there is no reminder and no code is asked for', async () => {
  const { h } = await opened(paperEngine({ holder: 'guardian' }));
  assert.equal(await h.D.paperDue(), false);
  assert.equal(h.callsOf('paper.sheet').length, 0);
});

/* ---- people changes need a fingerprint given just now (Bridge PEOPLE_FIELDS, heir.set) ---- */
const FRESH = 'Fingerprint confirmation required. Please try again.';
test('a guardian, heir or paper change asks one fresh fingerprint; dials alone (heir_delay too) ' +
  'ask none', async () => {
  const { h, before } = await opened();
  await h.D.changeRules({ heir_delay: 100, cap_kas: 5 });
  assert.equal(h.prompts.length, before);
  const people = [{ guardian_x: 'ab', guardian_which: 2 }, { guardian_x: '', guardian_which: 3 },
    { heir_x: '' }, { heir_address: 'kaspa:qheir' }, { paper_x: 'ab' }];
  for (const p of people) {
    const n = h.prompts.length;
    await h.D.changeRules(p);
    assert.equal(h.prompts.length - n, 1, JSON.stringify(p));
    assert.equal(h.prompts[h.prompts.length - 1].kind, 'signer');
  }
  assert.equal(h.callsOf('rules.change').length, 1 + people.length);
});

test('a people change refused for an old fingerprint asks once more and tries once more; a ' +
  'declined fingerprint sends nothing', async () => {
  let n = 0;
  const { h, before } = await opened((m, a) => m === 'status' ? status({ account: a.account || 1 })
    : m === 'rules.change' && ++n === 1 ? { ok: false, error: FRESH } : { ok: true });
  const r = await h.D.changeRules({ guardian_x: '', guardian_which: 3 });
  assert.equal(r.ok, true);
  assert.equal(h.callsOf('rules.change').length, 2);
  assert.equal(h.prompts.length - before, 2);
  h.setFingerprint('no');
  assert.equal((await h.D.changeRules({ heir_x: '' })).ok, false);
  assert.equal(h.callsOf('rules.change').length, 2);
});

test('naming or removing the heir (heir.set) asks one fresh fingerprint first', async () => {
  const { h, before } = await opened();
  await h.D.setHeir('');
  assert.equal(h.prompts.length - before, 1);
  assert.deepEqual(h.callsOf('heir.set')[0].args, { heir_x: '' });
});

/* ---- the paper stop after the review fixes: make keeps a pending stop, confirm stores it ---- */
test('paper.make sends a slot only when the owner chose 2 or 3', async () => {
  const { h } = await opened(paperEngine());
  await h.D.paperMake();
  await h.D.paperMake(3);
  await h.D.paperMake(7);
  assert.deepEqual(h.callsOf('paper.make').map((c) => c.args.slot), [undefined, 3, undefined]);
});

test('the words read back go to paper.confirm after one fresh fingerprint, as one string, kept ' +
  'in no setting', async () => {
  const { h, before } = await opened(paperEngine());
  const r = await h.D.paperConfirm(PAPER.slice());
  assert.equal(r.ok, true);
  assert.deepEqual(r.rule_words, ['Emergency stop button 2 becomes your paper stop.']);
  assert.equal(h.prompts.length - before, 1);
  assert.deepEqual(h.callsOf('paper.confirm')[0].args, { account: 1, words: PAPER.join(' ') });
  const kept = new Set(JSON.stringify(h.settings).toLowerCase().match(/[a-z]+/g));
  assert.deepEqual(PAPER.filter((w) => kept.has(w)), []);
  assert.equal((await h.D.paperConfirm(PAPER.slice(0, 23))).ok, false);
  assert.equal(h.callsOf('paper.confirm').length, 1);
});

test('a refused paper check keeps the vault address and code; the wait answer shows as is',
  async () => {
  const no = { ok: false, live: false, stage: null, vault_address: 'kaspa:qvault', code: '1a2b3c4d',
    error: 'Please wait a moment and try again.' };
  const { h } = await opened(paperEngine({ 'paper.check': no }));
  const r = await h.D.paperCheck('code-1');
  assert.deepEqual([r.ok, r.error], [false, 'Please wait a moment and try again.']);
  assert.deepEqual([r.vault_address, r.code], ['kaspa:qvault', '1a2b3c4d']);
});

test('a paper stop answer carries confirmed, the vault address and the code', async () => {
  const sent = { ok: true, tx: 'ab', kas: 4, hops: 0, confirmed: false,
    vault_address: 'kaspa:qvault', code: '1a2b3c4d', error: null };
  const { h } = await opened(paperEngine({ 'paper.stop': sent }));
  const r = await h.D.paperStop(PAPER.slice(), 'code-1');
  assert.deepEqual([r.confirmed, r.vault_address, r.code], [false, 'kaspa:qvault', '1a2b3c4d']);
});

/* take funds out with two vault seeds: keyless, no fingerprint, seeds as two strings */
const SEED_B = PAPER.slice().reverse();
test('paper.takeout sends two seed strings and the code, no `to` for the purse', async () => {
  const { h, before } = await opened(paperEngine({ 'paper.takeout':
    { ok: true, tx: 'ab', kas: 5, to: 'kaspa:qpurse', confirmed: false, error: null } }));
  const seeds = [PAPER.join(' '), SEED_B.join(' ')];
  const r = await h.D.paperTakeout(seeds, 'code-1', '');
  const c = h.callsOf('paper.takeout');
  assert.equal(c.length, 1);
  assert.deepEqual(c[0].args, { seeds: [PAPER.join(' '), SEED_B.join(' ')], watch_code: 'code-1' });
  assert.equal('to' in c[0].args, false, 'to is left out, never null');
  assert.equal(seeds.length, 0, "the caller's seeds are emptied");
  assert.equal(h.prompts.length, before, 'no fingerprint');
  assert.deepEqual([r.ok, r.confirmed], [true, false]);
});

test('paper.takeout sends a typed address as to, and refuses all but two seeds', async () => {
  const { h } = await opened(paperEngine());
  const dest = 'kaspa:qdestdestdestdestdest0';
  await h.D.paperTakeout([PAPER.join(' '), SEED_B.join(' ')], 'code-1', dest);
  assert.equal(h.callsOf('paper.takeout')[0].args.to, 'kaspa:qdestdestdestdestdest0');
  const one = await h.D.paperTakeout([PAPER.join(' ')], 'code-1', '');
  const long = await h.D.paperTakeout(['a'.repeat(1500), 'b'.repeat(600)], 'code-1', '');
  assert.equal(h.callsOf('paper.takeout').length, 1);
  const two = 'Type two vault seeds of 24 words each.';
  assert.deepEqual([one.error, long.error], [two, two]);
});

test("a paper.takeout refusal reaches the screen as the engine said it", async () => {
  const no = "These vault seeds do not open this account's vault.";
  const { h } = await opened(paperEngine({ 'paper.takeout': { ok: false, error: no } }));
  const r = await h.D.paperTakeout([PAPER.join(' '), SEED_B.join(' ')], 'code-1', '');
  assert.deepEqual([r.ok, r.error], [false, no]);
});

test('watch.code asks for the watched account by id and answers its code', async () => {
  const { h } = await opened(paperEngine({ 'watch.code': { ok: true, watch_code: 'W-1' } }));
  const r = await h.D.watchedCode('w7');
  assert.deepEqual(h.callsOf('watch.code')[0].args, { id: 'w7' });
  assert.equal(r.watch_code, 'W-1');
});

/* saved prices (setting price_saved) as the page keeps them: n hourly points to now, price f(i) */
function savedPrices(ranges) {
  const o = {}, now = Date.now();
  for (const r of Object.keys(ranges)) {
    const { n, f } = ranges[r];
    o[r] = { at: now, p: Array.from({ length: n }, (_, i) => [now - (n - 1 - i) * 3600000, f(i)]) };
  }
  return JSON.stringify(o);
}

test('the price summary: now, change, high, low, and the 7-day average from 1-week prices', () => {
  const saved = savedPrices({
    month: { n: 720, f: i => 0.05 + i / 10000 },   // 0.05 -> 0.1219, steady rise
    week: { n: 168, f: i => (i % 2 ? 0.1 : 0.08) }, // average 0.09
  });
  const D = load({ settings: { price_saved: saved } }).D;
  const s = D.priceSummary('month');
  assert.ok(Math.abs(s.now - 0.1219) < 1e-9);
  assert.ok(Math.abs(s.change - 0.0719) < 1e-9);
  assert.ok(Math.abs(s.pct - 143.8) < 1e-6);
  assert.deepEqual([+s.high.toFixed(9), s.low], [0.1219, 0.05]);
  assert.ok(Math.abs(s.avg - 0.09) < 1e-9);
  assert.deepEqual([s.fromWeek, s.avgHours], [true, 167]);
});

test('without 1-week prices the average uses the period\'s last 7 days, and says how far', () => {
  const D = load({ settings: { price_saved: savedPrices({ month: { n: 720, f: () => 0.2 } }) } }).D;
  const s = D.priceSummary('month');
  assert.deepEqual([s.fromWeek, s.avgHours, +s.avg.toFixed(9)], [false, 168, 0.2]);
  const day = savedPrices({ day: { n: 24, f: i => i + 1 } });
  const d = load({ settings: { price_saved: day } }).D.priceSummary('day');
  assert.deepEqual([d.avgHours, d.avg], [23, 12.5]);
  assert.equal(load().D.priceSummary('month'), null);
});

test('market.supply is asked keyless with no fingerprint and answers plain numbers', async () => {
  const sup = { ok: true, mined_kas: 26934120554, max_kas: '28704026601', reward_kas: 3.27,
    next_cut_at: 'Dec 7, 2026', next_reward_kas: 3.09, error: null };
  const { h, before } = await opened(paperEngine({ 'market.supply': sup }));
  const s = await h.D.marketSupply();
  assert.deepEqual(h.callsOf('market.supply')[0].args, {});
  assert.equal(h.prompts.length, before);
  assert.deepEqual({ ...s },
    { mined: 26934120554, max: 28704026601, reward: 3.27, next: 3.09, cutAt: 'Dec 7, 2026' });
});

test('market.supply hides on a refusal or an old engine; Sample has its own numbers', async () => {
  const { h } = await opened(paperEngine({ 'market.supply': { ok: false, error: 'busy' } }));
  assert.equal(await h.D.marketSupply(), null);
  const old = await opened(okEngine((m) => m));
  old.h.setEngine((m, a) =>
    m === 'status' ? status({ account: a.account || 1 }) : { status: 404, body: '' });
  assert.equal(await old.h.D.marketSupply(), null);
  const sample = await load({ settings: { live_local: '0' } }).D.marketSupply();
  assert.ok(sample.max > sample.mined && sample.mined > 0 && sample.reward > 0 && sample.cutAt);
});
