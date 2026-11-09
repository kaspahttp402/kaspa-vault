/* End-to-end run of the real KaspaVault app on two proxy phones (Android emulators, testnet),
   driven over adb + the
   WebView DevTools socket (proxy.js). Owner makes an account, the helper links in as guardian 2 +
   heir, the helper
   watches, stops the account, the owner reopens it, the heir clock runs out, the helper claims and
   pays 1 KAS back.
   usage: node app/web-tests/e2e-proxy.js env: OWNER=<serial> HELPER=<serial> PIN=<device pin>
   NO_PIN=1
   NO_PIN=1 leaves the system prompts (BiometricPrompt / ANR dialog) to someone else. Stops at the
   first failure. */
'use strict';
const { adb, connect, waitFor, sleep } = require('./proxy.js');

const OWNER = process.env.OWNER || 'emulator-5560';
const HELPER = process.env.HELPER || 'emulator-5562';
const PIN = process.env.PIN || '1234';
const PKG = 'com.kaspavault.wallet';
const MIN = 60000;
const log = (...a) => console.log('     ', ...a);
const ctx = { owner: null, helper: null, code: null, watchId: null, ownerPurse: null };

/* ---- the phone side: keyguard, top activity, system prompts ---- */
function focus(serial) {
  const w = adb(serial, ['shell', 'dumpsys', 'window']);
  const m = w.match(/mCurrentFocus=([^\n]*)/);
  return { all: w, focus: m ? m[1] : '' };
}
function tapText(serial, text) {
  adb(serial, ['shell', 'uiautomator', 'dump', '/sdcard/kv_ui.xml']);
  const xml = adb(serial, ['shell', 'cat', '/sdcard/kv_ui.xml']);
  const re = new RegExp('<node[^>]*text="' + text +
    '"[^>]*bounds="\\[(\\d+),(\\d+)\\]\\[(\\d+),(\\d+)\\]"');
  const m = xml.match(re);
  if (!m) return false;
  const x = Math.round((+m[1] + +m[3]) / 2), y = Math.round((+m[2] + +m[4]) / 2);
  adb(serial, ['shell', 'input', 'tap', String(x), String(y)]);
  return true;
}
/* answers the phone's PIN prompt (BiometricPrompt, seen twice 1 s apart) and taps Wait on an ANR
   dialog (never Enter:
   Enter there is "Close app"). Returns a stop function. */
/* one look at the screen: Wait on an ANR dialog; the PIN on a BiometricPrompt still there 1 s later
   */
async function promptRound(serial, isOn) {
  const f = focus(serial).focus;
  if (/Application Not Responding/i.test(f)) {
    log(serial + ': ANR dialog, tapping Wait:', tapText(serial, 'Wait'));
    await sleep(3000);
    return;
  }
  if (!/BiometricPrompt/.test(f)) return;
  await sleep(1000);
  if (!isOn() || !/BiometricPrompt/.test(focus(serial).focus)) return;
  log(serial + ': answering the PIN prompt');
  adb(serial, ['shell', 'input', 'text', PIN]);
  adb(serial, ['shell', 'input', 'keyevent', '66']);
  await sleep(3000);
}
function answerPrompts(serial) {
  if (process.env.NO_PIN === '1') return () => {};
  let on = true;
  (async () => {
    /* at least 1 s a round: about 6 hours at most */
    for (let round = 0; on && round < 20000; round++) {
      try {
        await promptRound(serial, () => on);
      } catch (e) { /* adb hiccup: look again next round */ }
      await sleep(1000);
    }
  })();
  return () => { on = false; };
}
function preflightOne(serial) {
  if (/isKeyguardShowing=true/.test(focus(serial).all)) {
    log(serial + ': unlocking the device');
    adb(serial, ['shell', 'input', 'keyevent', '224']);
    adb(serial, ['shell', 'wm', 'dismiss-keyguard']);
    adb(serial, ['shell', 'input', 'text', PIN]);
    adb(serial, ['shell', 'input', 'keyevent', '66']);
  }
  const acts = adb(serial, ['shell', 'dumpsys', 'activity', 'activities']);
  if (!new RegExp('(topResumedActivity|mResumedActivity|ResumedActivity)[^\\n]*' +
    PKG.replace(/\./g, '\\.')).test(acts)) {
    log(serial + ': starting the app');
    adb(serial, ['shell', 'am', 'start', '-n', PKG + '/.MainActivity']);
  }
}

/* ---- the page side ---- */
const RETRY = /Try again in a moment\.$/;
/* a KVData call that answers {ok, error}; a network refusal ending "Try again in a moment." is
   retried 3 times, 20 s apart */
async function call(p, expr, what) {
  for (let i = 0; ; i++) {
    const r = await p.ev(expr);
    if (r && r.ok === false && RETRY.test(String(r.error || '')) && i < 3) {
      log(p.serial + ': ' + what + ' refused (' + r.error + '), retry ' + (i + 1) + '/3 in 20 s');
      await sleep(20000);
      continue;
    }
    if (!r || r.ok === false) throw new Error(what + ': ' + (r ? r.error : 'no answer'));
    return r;
  }
}
const phase = (p) => p.ev('KVData.phase()');
const watched = (p, id) =>
  p.ev('KVData.loadWatched().then(function(l){var w=(l||[]).filter(function(x){return x.id===' +
    JSON.stringify(id) + '})[0];' +
    'return w?{roles:w.roles,st:w.st}:null})');

/* ---- steps ---- */
async function preflight() {
  preflightOne(OWNER);
  preflightOne(HELPER);
  await sleep(5000);
  ctx.owner = await connect(OWNER, 9301);
  ctx.helper = await connect(HELPER, 9302);
  for (const p of [ctx.owner, ctx.helper]) await waitFor(p,
    'window.KVData && KVData.phase && window.KVApp && KVApp.ui', MIN, 'the page');
  log('owner phase', await phase(ctx.owner), '| helper phase', await phase(ctx.helper));
}

async function unlock() {
  for (const p of [ctx.owner, ctx.helper]) {
    if ((await phase(p)) !== 'locked') continue;
    await sleep(3000); /* the lock screen may press its own button once it is shown */
    if ((await phase(p)) === 'locked' && !(await p.ev('!!KVApp.ui.bio'))) {
      await waitFor(p, 'KVApp.ui.cur && KVApp.ui.cur.name === "lock"', 30000, 'the lock screen');
      await p.ev('(function(){var b=(KVApp.ui.el||document).querySelector("[data-ok]");if(b)' +
        'b.click();return !!b})()');
      log(p.serial + ': pressed the lock screen button once');
    }
    await waitFor(p, 'KVData.phase() !== "locked"', 3 * MIN, 'unlock');
  }
}

async function ownerAccount() {
  const p = ctx.owner;
  if ((await phase(p)) === 'noKeys') {
    await call(p, 'KVData.makeKeys({mode:"phone",heir_x:"",auto_heir:true,hard_limit:false})',
      'makeKeys');
    await waitFor(p, 'KVData.phase() !== "noKeys"', 2 * MIN, 'keys made');
  }
  const f = await call(p, 'KVData.funding()', 'funding');
  ctx.ownerPurse = f.address;
  log('owner purse:', f.address, 'holds', f.kas, 'KAS');
  const ph = await phase(p);
  if (ph !== 'unopened') { log('owner phase', ph, ': account already opened'); return; }
  let kas = f.kas;
  if (!(kas > 0)) {
    console.log('\n  FUND THIS PURSE (testnet): ' + f.address + '\n  waiting up to 15 min...\n');
    const until = Date.now() + 15 * MIN;
    for (;;) {
      const g = await p.ev('KVData.funding()').catch(() => null);
      if (g && g.ok && g.kas > 0) { kas = g.kas; break; }
      if (Date.now() > until) throw new Error('the purse was not funded within 15 min');
      await sleep(15000);
    }
  }
  const amt = Math.floor((kas - 0.1) * 100) / 100;
  if (!(amt > 0)) throw new Error('purse holds too little (' + kas + ' KAS)');
  log('opening the account with', amt, 'KAS');
  await call(p, 'KVData.fund(' + amt + ', null)', 'fund');
  await waitFor(p, 'KVData.phase() === "open"', 10 * MIN, 'owner phase open');
}

async function helperLink() {
  const p = ctx.helper;
  let lc = await p.ev('KVData.linkCode()');
  if (!lc || !lc.ok) {
    const ph = await phase(p);
    if (ph !== 'noKeys') throw new Error('no link code and helper phase is ' + ph);
    await call(p, 'KVData.makeLink("Helper")', 'makeLink');
    lc = await call(p, 'KVData.linkCode()', 'linkCode');
  }
  ctx.code = lc.code;
  log('link code:', String(lc.code).length, 'chars');
}

const linkApplied = (k, r) => {
  const pb = (k && k.public) || {};
  const g = (k && k.stops || []).some((s) => s && s.which === 2 && s.holder === 'guardian');
  return g && !!pb.heir_x && (!r || pb.heir_x === r.heir_x);
};
async function ownerApplyLink() {
  const p = ctx.owner;
  const r = await call(p, 'KVData.readLink(' + JSON.stringify(ctx.code) + ')', 'readLink');
  if (linkApplied(await p.ev('KVData.keysPublic()'), r)) { log('link already applied'); return; }
  await p.ev('KVData.nameHelper(' + JSON.stringify(r.stop_x) + ',"Helper"),KVData.nameHelper(' +
    JSON.stringify(r.heir_x) + ',"Helper"),true');
  await call(p, 'KVData.changeRules({guardian_x:' + JSON.stringify(r.stop_x) +
    ',guardian_which:2,heir_x:' + JSON.stringify(r.heir_x) + '})', 'changeRules');
  await updateNow(p);
  const k = await p.ev('KVData.keysPublic()');
  if (!linkApplied(k,
    r)) throw new Error('keysPublic does not show guardian at stop 2 + the heir: ' +
    JSON.stringify({ stops: k && k.stops, heir_x: k && k.public && k.public.heir_x }));
}
/* updateNow's step lines are kept in the page and printed here */
async function updateNow(p) {
  try {
    await call(p,
      '(window.__kvSteps=[],KVData.updateNow(function(s){window.__kvSteps.push(String(s))}))',
      'updateNow');
  } finally {
    for (const s of (await p.ev('window.__kvSteps||[]').catch(() => [])) || []) log('updateNow:',
      s);
  }
  await waitFor(p, 'KVData.phase() === "open"', 10 * MIN, 'owner phase open after updateNow');
}

async function watchCode() {
  const wc = await call(ctx.owner, 'KVData.watchCode("Owner")', 'watchCode');
  /* the code stays inside node */
  const r = await ctx.helper.ev('KVData.watchImport(' + JSON.stringify(wc.code) + ')');
  const list = (await ctx.helper.ev('KVData.loadWatched()')) || [];
  const w = list.filter((x) => x.roles && x.roles.guardian === 2 && x.roles.heir === true).pop();
  if (!w) throw new Error('watchImport ' + (r && r.ok ? 'ok' : r && r.error) +
    ', but no watched account with roles {guardian:2, heir:true}: ' + JSON.stringify(list.map((x) =>
    x.roles)));
  if (!(r && r.ok)) log('watchImport said', r && r.error, '- the account was already watched');
  ctx.watchId = w.id;
  log('watched id', w.id, 'roles', JSON.stringify(w.roles));
}

async function stopAndReopen() {
  const h = ctx.helper, o = ctx.owner;
  await call(h, 'KVData.watchStop(' + JSON.stringify(ctx.watchId) + ')', 'watchStop');
  await waitFor(o, 'KVData.phase() === "stopped"', 10 * MIN, 'owner phase stopped');
  log('owner sees stopped; reopening');
  await updateNow(o);
  /* a watched copy no longer follows a reopen by itself (same-address decoy coins): the helper
     imports the
     owner's fresh watch code, and the claim uses the id that import returns */
  const wc = await call(o, 'KVData.watchCode("Owner")', 'watchCode after the reopen');
  /* the code stays inside node */
  const r = await h.ev('KVData.watchImport(' + JSON.stringify(wc.code) + ')');
  if (r && r.ok && r.id) ctx.watchId = r.id;
  else log('watchImport said', r && r.error, '- keeping watched id', ctx.watchId);
  log('fresh watch code imported; watched id', ctx.watchId);
  const until = Date.now() + 10 * MIN;
  for (;;) {
    const w = await watched(h, ctx.watchId).catch(() => null);
    if (w && w.st && w.st.stage === 'open') break;
    if (Date.now() > until) throw new Error('helper watch stage is ' + (w && w.st && w.st.stage) +
      ', not open');
    await sleep(15000);
  }
}

async function heirClaim() {
  const h = ctx.helper;
  const until = Date.now() + 10 * MIN;
  let last = -1;
  for (;;) {
    const w = await watched(h, ctx.watchId).catch(() => null);
    const st = (w && w.st) || {};
    if (st.heir_due || st.heir_seconds_left === 0) break;
    if (st.heir_seconds_left !== last) { last = st.heir_seconds_left; log('heir clock:', last,
      's left'); }
    if (Date.now() > until) throw new Error('the heir clock did not run out within 10 min (' +
      last + ' s left)');
    await sleep(20000);
  }
  await call(h, 'KVData.watchClaim(' + JSON.stringify(ctx.watchId) + ')', 'watchClaim');
  const hp = await waitFor(h,
    'KVData.heirPurse().then(function(r){return r&&r.ok&&r.kas>0?r:null})', 10 * MIN,
    'heir purse > 0');
  log('heir purse holds', hp.kas, 'KAS');
  if (!ctx.ownerPurse) ctx.ownerPurse = (await call(ctx.owner, 'KVData.funding()',
    'funding')).address;
  await call(h, 'KVData.heirPay(' + JSON.stringify(ctx.ownerPurse) + ', 1)', 'heirPay');
}

const STEPS = [preflight, unlock, ownerAccount, helperLink, ownerApplyLink, watchCode,
  stopAndReopen, heirClaim];

(async () => {
  console.log('owner ' + OWNER + ', helper ' + HELPER + (process.env.NO_PIN === '1' ?
    ' (prompts left to someone else)' : ''));
  let n = 0;
  for (const s of STEPS) {
    /* both phones, all through each step */
    const stops = [answerPrompts(OWNER), answerPrompts(HELPER)];
    try {
      await s();
      n++;
      console.log('ok   ' + s.name);
    } catch (e) {
      console.log('FAIL ' + s.name + ': ' + (e && e.message ? e.message : e));
      break;
    } finally {
      stops.forEach((f) => f());
    }
  }
  console.log(n + '/' + STEPS.length + ' steps');
  for (const p of [ctx.owner, ctx.helper]) if (p) { if (p.errors.length) log(p.serial +
    ' page errors:', p.errors.slice(0, 5).join(' | ')); p.close(); }
  process.exit(n === STEPS.length ? 0 : 1);
})();
