/* People on the phone page (index.html, a fake engine): who holds each emergency stop button, the
   two tabs, and what a
   claim paid to this phone's heir address. Each case is a rule the screen relies on; a failure
   names it in plain words.
   Run from the repo root: node app/web-tests/phone.js (headless Chromium, off screen, like
   smoke.js; KV_CHROME overrides) */
'use strict';
const { spawn } = require('child_process'), fs = require('fs'), os = require('os'), path =
  require('path');
function findChrome() {
  if (process.env.KV_CHROME) return process.env.KV_CHROME;
  const home = os.homedir();
  const roots = [process.env.PLAYWRIGHT_BROWSERS_PATH,
    process.platform === 'win32' ? path.join(process.env.LOCALAPPDATA || path.join(home, 'AppData',
      'Local'), 'ms-playwright')
      : process.platform === 'darwin' ? path.join(home, 'Library', 'Caches', 'ms-playwright') :
        path.join(home, '.cache', 'ms-playwright')];
  const exe = process.platform === 'win32' ? 'chrome-headless-shell.exe' : 'chrome-headless-shell';
  for (const root of roots) {
    if (!root || !fs.existsSync(root)) continue;
    for (const d of fs.readdirSync(root).filter((x) =>
      x.startsWith('chromium_headless_shell-')).sort().reverse())
      for (const sub of fs.readdirSync(path.join(root, d))) { const f = path.join(root, d, sub,
        exe); if (fs.existsSync(f)) return f; }
  }
  console.error('people: no Chromium headless shell found (set KV_CHROME)');
  process.exit(2);
}
const PAGE = 'file:///' + path.join(__dirname, '..', 'web', 'index.html').replace(/\\/g, '/');

/* the fake engine: stop keys aa (button 1), bb (2), cc (3); `stops` and `heirKas` vary per case */
function fake(c) {
  const pub = JSON.stringify(Object.assign({ cancel_x: 'aa', cancel2_x: 'bb',
    cancel3_x: c.slot3 || 'cc' }, c.heirX ? { heir_x: c.heirX } : {}));
  const settings = JSON.stringify(Object.assign({ live_local: '1', toured: '1' }, c.names ?
    { helper_names: JSON.stringify(c.names) } : {}));
  return `(function () {
  var settings = ${settings}, EXTRA = ${JSON.stringify(c.answers || {})};
  window.__kvSettings = settings; window.__kvAnswers = EXTRA; window.__kvPaper = []; window.__kvSigner = 0;
  function status(a) { return { ok: true, account: a || 1, accounts: [1], next_account: 2, balance_kas: 120, limit_kas: 20, spent_kas: 0, left_kas: 20,
    safe_kas: 5, stopped: false, purse_kas: 3, notice_seconds: 86400, refresh_seconds: 0, sheets_on_phone: true, heir: false,
    history: ${JSON.stringify(c.history || [])},
    alerts: ${JSON.stringify(c.alerts || [])} }; }
  function answer(m, a) {
    if (m.slice(0, 6) === 'paper.') window.__kvPaper.push({ m: m, a: a });
    if (EXTRA[m]) return EXTRA[m];
    var purse = { purse_address: 'kaspa:qpurse000000000000000000000' };
    if (m === 'status') return Object.assign(status(a.account), purse);
    if (m === 'keys.public') return { ok: true,
      public: ${pub}${c.stops ? ', stops: ' + JSON.stringify(c.stops) : ''} };
    if (m === 'heir.purse') return { ok: true, address: 'kaspa:qheir', kas: ${c.heirKas || 0} };
    var said = ${JSON.stringify(c.answers || {})}; /* a case's own engine answers, by method */
    if (said[m]) return said[m];
    return { ok: true };
  }
  var guard = false, signedAt = 0, slow = ${c.slowSend || 5};
  window.KV_unsign = function () { signedAt = 0; };
  window.KaspaVaultBridge = {
    engine: function () { return true; },
    getSetting: function (k) { return settings[k] || ''; },
    setSetting: function (k, v) { /* like Java: screenshots go on only within 90 s of a fingerprint */
      if (k === 'screenshots' && v === 'on' && settings[k] !== 'on' && !(Date.now() - signedAt < 90000)) return;
      settings[k] = String(v);
    },
    vault: function (cb, id, m, args) {
      var r = JSON.stringify(answer(m, JSON.parse(args)));
      setTimeout(function () { window[cb](id, 200, r); }, m === 'send' || m === 'paper.takeout' ? slow : 5);
    },
    confirmSigner: function (cb) {
      signedAt = Date.now();
      window.KV_signs = (window.KV_signs || 0) + 1;
      window.__kvSigner++;
      setTimeout(function () { window[cb]('ok'); }, 5);
    },
    confirmWithBiometric: function (cb) { setTimeout(function () { window[cb]('ok'); }, 5); },
    price: function () {}, copy: function () {}, screen: function () {},
    keysNew: function (cb, id) {
      var r = JSON.stringify(answer('keys.new', {}));
      setTimeout(function () { window[cb](id, 200, r); }, 5);
    },
    guardOn: function (on) { guard = !!on; return !!on; },
    guardRunning: function () { return guard; },
    guardOff: function (cb) { setTimeout(function () { guard = false; window[cb]('ok'); }, 5); }
  };
})();`;
}

const ROWS =
  "[].map.call(document.querySelectorAll('[data-helpers] .item'), function (b) { return " +
    "b.querySelector('.t').textContent + ' | ' + b.querySelector('.d').textContent; })";
const CASES = [
  { name: 'an older engine (no stops list): unnamed stop buttons 2 and 3 are not in use',
    want: ['Emergency Stop 2 | Not in use', 'Emergency Stop 3 | Not in use'], noRemove: true },
  { name: 'the engine says who holds each button: a guardian shows, an unheld button is not in use',
    stops: [{ which: 1, holder: 'you' }, { which: 2, holder: 'guardian' }, { which: 3,
      holder: 'unheld' }], want: ['Someone | Guardian', 'Emergency Stop 3 | Not in use'] },
  { name: "'unknown' falls back to the names of links the owner applied; a button that is the " +
    "owner's own is not listed",
    stops: [{ which: 2, holder: 'unknown' }, { which: 3, holder: 'you' }], names: { bb: 'Sam' },
      want: ['Sam | Guardian'] },
  { name: 'the paper stop shows on the button the engine names (3 here), as Paper stop',
    stops: [{ which: 2, holder: 'unheld' }, { which: 3, holder: 'paper' }], noRemove: true,
    want: ['Emergency Stop 2 | Not in use', 'Paper stop | Emergency Stop 3'] },
  { name: 'funds a claim paid show under You guard as Received as heir', heirKas: 12.5,
    heirRow: true, want: ['Emergency Stop 2 | Not in use', 'Emergency Stop 3 | Not in use'] },
];

/* the money screens (docs/surface/page-review.md): each check returns what went wrong, in words */
const TO = 'kaspa:qtest00000000000000000000';
const JOB = "KVApp.go('waiting', { job: 'send', to: '" + TO + "', kas: 1 }, { root: true })";
const CUR = 'KVApp.ui.cur.name';
const ARMED = "!!document.querySelector('[data-armed]')";
const NOTE = "(document.querySelector('[data-armed]').previousElementSibling || {}).textContent";
const NOTES =
  "[].some.call(document.querySelectorAll('.sub'), function (e) { return /^To kaspa:/.test(" +
    "e.textContent); })";
const SHOT = "KaspaVaultBridge.getSetting('screenshots')";
const FLIP = "document.querySelector('.switch[aria-label=\"Allow screenshots\"]').click();"
  + " document.querySelector('[data-save]').click()";
const FILL = "var t = document.querySelector('[data-to]'); t.value = '" + TO + "';"
  + " t.dispatchEvent(new Event('input')); var a = document.querySelector('[data-amt]');"
  + " a.value = '5'; a.dispatchEvent(new Event('input'));";
const SLIDE = "var i = document.querySelector('.slide i'); for (var k = 0; k < 12; k++)"
  + " i.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight' }));"
  + " i.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));";
async function lockAfterJob(open, ev) {
  const bad = [];
  await open({ slowSend: 1500 });
  await ev(JOB); await sleep(300);
  await ev('window.KV_pause()'); await sleep(2000);
  const away = await ev(CUR), back = await ev('window.KV_resume(); ' + CUR);
  if (away !== 'lock' || back !== 'lock') bad.push('put away mid-send: ' + away + ' / ' + back);
  await open({ slowSend: 300 });
  await ev(JOB); await sleep(1200);
  const front = await ev(CUR);
  if (front !== 'sent') bad.push('a send kept in front ended on ' + front + ', not sent');
  return bad;
}
async function sendSlide(open, ev) {
  const bad = [];
  await open({});
  await ev("KVApp.go('send', {}, { root: true })"); await sleep(800);
  await ev(FILL + " document.querySelector('[data-ok]').click()"); await sleep(500);
  if (!(await ev(ARMED))) return ['Send did not arm the slide'];
  const note = await ev(NOTE), whole = 'To kaspa: ' + TO.slice(6).replace(/(.{4})(?=.)/g, '$1 ');
  if (note !== whole) bad.push('the slide does not show the whole address: ' + note);
  await ev("document.querySelector('[data-c]').click()"); await sleep(200);
  if (await ev(ARMED)) bad.push('a payee chip left the slide armed');
  if (await ev(NOTES)) bad.push('the address line stayed after the slide went');
  await ev("document.querySelector('[data-ok]').click()"); await sleep(500);
  await ev("KVApp.ui.draft.to = '" + TO + "'"); /* a change that did not disarm */
  await ev(SLIDE); await sleep(400);
  const err = await ev("document.querySelector('[data-e]').textContent"), cur = await ev(CUR);
  if (!/The payment changed/.test(err) || cur !== 'send')
    bad.push('a changed draft was not refused: ' + cur + ' ' + err);
  return bad;
}
async function guardOff(open, ev) {
  const bad = [];
  await open({});
  await ev("KVApp.go('watcher', {}, { root: true })"); await sleep(800);
  const tap = (v) => ev("document.querySelector('[data-guard] [data-v=\"" + v + "\"]').click()");
  await tap('1'); await sleep(1100);
  if (!(await ev('KaspaVaultBridge.guardRunning()'))) bad.push('On did not start the guard');
  await tap('0'); await sleep(1100);
  if (await ev('KaspaVaultBridge.guardRunning()')) bad.push('Off did not stop the guard');
  const on = await ev("document.querySelector('[data-guard] [data-v=\"0\"]').className");
  if (!/on/.test(on)) bad.push('the switch does not show Off');
  return bad;
}
/* Allow screenshots: On asks a fresh fingerprint first (Java keeps it only within 90 s of one); Off
   is free */
async function shotsOn(open, ev) {
  const bad = [];
  await open({});
  await ev("KVApp.go('prefs', {}, { root: true })"); await sleep(800);
  await ev('KV_unsign()');
  await ev(FLIP); await sleep(800);
  if ((await ev(SHOT)) !== 'on') bad.push('On did not stick: ' + (await ev(SHOT)));
  const signs = await ev('window.KV_signs');
  await ev(FLIP); await sleep(800);
  if ((await ev(SHOT)) === 'on') bad.push('Off did not stick');
  if ((await ev('window.KV_signs')) !== signs) bad.push('Off asked for a fingerprint');
  return bad;
}
/* engine flags: a Delete the engine did not finish never shows success; Make my keys on a phone
   with an
   account goes Home */
const NOT_BURNT = 'The vault seeds could not be deleted from this phone. Nothing changed.';
async function engineFlags(open, ev) {
  const bad = [];
  await open({ answers: { 'keys.burn': { ok: false, burn_not_done: true, error: NOT_BURNT } } });
  await ev("KVApp.go('burnsheets', {}, { root: true })"); await sleep(800);
  await ev("document.querySelector('[data-sure] [data-v=\"1\"]').click()");
  await ev("document.querySelector('[data-ok]').click()"); await sleep(400);
  await ev(SLIDE); await sleep(800);
  const said = await ev("document.querySelector('[data-e]').textContent"), cur = await ev(CUR);
  if (said !== NOT_BURNT || cur !== 'burnsheets') bad.push('Delete not done showed: ' + cur + ' ' +
    said);
  await open({ answers: { 'keys.new': { ok: false, already_has_keys: true,
    error: 'This phone has keys.' } } });
  await ev("KVApp.go('makekeys', {}, { root: true })"); await sleep(800);
  await ev("document.querySelector('[data-phone]').click()"); await sleep(1200);
  if ((await ev(CUR)) !== 'home') bad.push('Make my keys with keys already made ended on ' +
    (await ev(CUR)));
  return bad;
}
/* a brand-new account opens How it works first (once); its last step leads on to the covenant */
async function newWalk(open, ev) {
  const bad = [], made = "KVApp.go('makekeys', {}, { root: true })";
  await open({ answers: { 'keys.new': { ok: true } } });
  await ev("window.__kvSettings.toured = ''; " + made); await sleep(800);
  await ev("document.querySelector('[data-phone]').click()"); await sleep(1200);
  if ((await ev(CUR)) !== 'howitworks') return ['new keys opened ' + (await ev(CUR))];
  const last = "{ intro: true, then: 'keysmade', step: 99 }";
  await ev("KVApp.go('howitworks', " + last + ", { replace: true })");
  await sleep(800);
  await ev("document.querySelector('[data-done]').click()"); await sleep(900);
  if ((await ev(CUR)) !== 'keysmade') bad.push('the last step led to ' + (await ev(CUR)));
  await ev(made); await sleep(800);
  await ev("document.querySelector('[data-phone]').click()"); await sleep(1200);
  if ((await ev(CUR)) !== 'keysmade') bad.push('a second time opened ' + (await ev(CUR)));
  return bad;
}
/* Home: no guardian and no heir shows the lost-phone row; a guardian or the paper stop hides it */
const LONE = "!document.querySelector('[data-lone]').hidden";
async function loneRow(open, ev) {
  const bad = [];
  await open({});
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1200);
  if (!(await ev(LONE))) bad.push('no guardian and no heir, but the row is hidden');
  const lone = await ev("document.querySelector('[data-lone]').textContent");
  if (!/Don't change your phone's fingerprints/.test(lone)) bad.push('lost-phone row: ' + lone);
  await open({ stops: [{ which: 2, holder: 'guardian' }] });
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1200);
  if (await ev(LONE)) bad.push('a guardian holds stop 2, but the row shows');
  await open({ stops: [{ which: 2, holder: 'unheld' }, { which: 3, holder: 'paper' }] });
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1200);
  if (await ev(LONE)) bad.push('the paper stop holds stop 3, but the row shows');
  return bad;
}
/* the paper stop (docs/PAPER_STOP.md): make, words, read back, Update now, the code;
   and a stop from paper on a phone with no keys */
const W24 = ('zebra zero zone zoo youth young yellow yard wrist wrong wolf wisdom winter wink'
  + ' wing wine window width wide whisper wheat whale weasel walnut').split(' ');
const PW = JSON.stringify(W24);
const PICK = '(function () { var W = ' + PW + ", d = document.querySelector('[data-w]'),"
  + " n = parseInt(d.textContent.replace('Word ', ''), 10),"
  + " bs = document.querySelectorAll('[data-pick] button');"
  + ' [].filter.call(bs, function (b) { return b.textContent === W[n - 1]; })[0].click(); })()';
const FILLW = 'var W = ' + PW + "; var ins = document.querySelectorAll('[data-pw]');"
  + ' [].forEach.call(ins, function (b, i) { b.value = W[i]; });';
const SHOWN = "[].map.call(document.querySelectorAll('.words div'),"
  + ' function (d) { return d.lastChild.textContent; }).join(" ")';
const PAPER_HIDDEN = "document.querySelector('[data-paper]').hidden";
const SHEET_HAS = (sel) => "!!document.querySelector('#sheet " + sel + "')";
const SEC = "document.querySelector('[data-code]').closest('section')"; /* Use a paper stop */
const VAULT = 'kaspa:qvault0000000000000000000000';
const HERE = 'KVApp.ui.el.textContent'; /* the screen on show, not the ones behind it */
const IN_SEC = (sel) => SEC + ".querySelector('" + sel + "')";
/* paper.make answers the words and slot only; paper.confirm (the words read back) answers
   rules.change's shape, its rule lines in rule_words */
const MADE = { ok: true, words: W24, slot: 3, error: null };
const CONFIRMED = { ok: true, route: 'move', kind: 'notice',
  now: { ok: true, fee_kas_about: 0.003 },
  fee_kas_about: 0.002, steps: [], rule_words: ['Emergency stop button 3 becomes your paper stop.'],
  slot: 3, error: null };
const PAPER_CALLS = (m) => 'JSON.stringify(window.__kvPaper.filter(function (c) {'
  + " return c.m === '" + m + "'; }).map(function (c) { return c.a; }))";
async function paperMake(open, ev) {
  const bad = [];
  await open({ stops: [{ which: 2, holder: 'guardian' }, { which: 3, holder: 'paper' }],
    answers: { 'paper.make': MADE, 'paper.confirm': CONFIRMED, takeout: { ok: true, open_kas: 10 },
      'paper.sheet': { ok: true, watch_code: '{"public_sha256":"1a2b3c4d99","public":{}}' } } });
  await ev("KVApp.go('paperstop', {}, { root: true })"); await sleep(500);
  await ev("document.querySelector('[data-make]').click()"); await sleep(700);
  const shown = await ev(SHOWN);
  if ((await ev(CUR)) !== 'papershow' || shown !== W24.join(' '))
    return ['words not shown: ' + shown];
  if ((await ev('window.__kvSigner')) !== 1) bad.push('make did not ask exactly one fingerprint');
  await ev("document.querySelector('[data-wrote]').click()"); await sleep(400);
  for (let k = 0; k < 3; k++) { await ev(PICK); await sleep(250); }
  await sleep(400);
  if ((await ev(CUR)) !== 'paperstop' || (await ev('KVApp.ui.paper')) !== null)
    bad.push('the read-back did not end on Paper stop with the words gone');
  if ((await ev('window.__kvSigner')) !== 2)
    bad.push('paper.confirm did not ask one more fingerprint');
  const conf = JSON.stringify([{ account: 1, words: W24.join(' ') }]);
  if ((await ev(PAPER_CALLS('paper.confirm'))) !== conf)
    bad.push('paper.confirm did not get the 24 words');
  const review = await ev("(document.querySelector('#sheet') || {}).textContent || ''");
  if (!/becomes your paper stop/.test(review)) bad.push('the review does not show the rule lines');
  if (!(await ev(SHEET_HAS('[data-now]')))) return bad.concat('no Update now');
  await ev("document.querySelector('#sheet [data-now]').click()"); await sleep(300);
  await ev(SLIDE); await sleep(2500);
  const cur = await ev(CUR);
  if (cur !== 'papers') return bad.concat('Update now ended on ' + cur);
  await sleep(600);
  await ev("document.querySelector('[data-paperrow=stop]').click()"); await sleep(600);
  if ((await ev(CUR)) !== 'paperprint') return bad.concat('the stop row did not open the code');
  return bad.concat(await paperPrinted(ev));
}
/* after the reopen: the code drawn and its hash kept; Home asks again only once the code changes;
   People shows the paper stop with no Remove */
async function paperPrinted(ev) {
  const bad = [];
  if (!(await ev("!!document.querySelector('[data-qr] svg')"))) bad.push('no code drawn');
  const ids = await ev("document.querySelector('[data-ids]').textContent");
  if (!/account code 1a2b3c4d\./.test(ids)) bad.push('the account code to write down: ' + ids);
  const kept = await ev('window.__kvSettings.paper_shown');
  if (!/^r[0-9a-f]{8}$/.test(kept)) bad.push('paper_shown not kept: ' + kept);
  await ev("['s1', 's2', 's3', 'p'].forEach(function (t) { KVData.paperMark(t); });"
    + " KVApp.go('home', {}, { root: true })"); await sleep(900);
  if (!(await ev(PAPER_HIDDEN))) bad.push('Home asks to update a paper stop just printed');
  await ev("window.__kvAnswers['paper.sheet'].watch_code = 'KVW-2';"
    + " KVApp.go('home', {}, { root: true })"); await sleep(900);
  if (await ev(PAPER_HIDDEN)) return bad.concat('Home does not ask after the code changed');
  await ev("document.querySelector('[data-paper]').click()"); await sleep(400);
  if ((await ev(CUR)) !== 'paperprint') bad.push('the Home row did not open the code');
  await ev("KVApp.go('people', {}, { root: true })"); await sleep(1200);
  const rows = await ev(ROWS);
  const at = (rows || []).indexOf('Paper stop | Emergency Stop 3');
  if (at < 0) return bad.concat('People rows ' + JSON.stringify(rows));
  await ev("document.querySelectorAll('[data-helpers] .item')[" + at + '].click()');
  await sleep(300);
  if (await ev(SHEET_HAS('[data-hrm]'))) bad.push('the paper stop offers Remove');
  return bad;
}
async function paperUse(open, ev) {
  const bad = [];
  const fresh = { ok: false, no_keys: true, error: 'This phone has no keys yet.' };
  await open({ answers: { status: fresh,
    'paper.check': { ok: true, kas: 42.5, hops: 3, live: true, stage: 'waiting', code: '1a2b3c4d',
      vault_address: VAULT },
    'paper.stop': { ok: true, tx: 'ab', kas: 42.5, hops: 3, confirmed: false, code: '1a2b3c4d',
      vault_address: VAULT } } });
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(800);
  const entry = "document.querySelector('[data-row] [data-go=paperuse]')";
  if (!(await ev('!!' + entry))) return ['a phone with no keys offers no Use a paper stop'];
  await ev(entry + '.click()'); await sleep(500);
  await ev("document.querySelector('[data-code]').value = 'KVW-printed';"
    + " document.querySelector('[data-check]').click()"); await sleep(500);
  const say = await ev("document.querySelector('[data-say]').textContent");
  if (!/42\.5 KAS/.test(say) || !/A withdrawal is waiting/.test(say))
    bad.push('found line: ' + say);
  const ids = await ev(IN_SEC('[data-ids]') + '.textContent');
  if (ids !== 'Check this matches your paper: 1a2b3c4d. Your vault: ' + VAULT + '.')
    bad.push('ids line: ' + ids);
  bad.push(...await wordTaps(ev));
  await ev(FILLW + " ins[6].value = 'notaword'; " + IN_SEC('[data-stop]') + '.click()');
  await sleep(300);
  const err = await ev(IN_SEC('[data-e]') + '.textContent');
  if (!/Word 7 is not on the word list/.test(err) || /notaword/.test(err) || (await ev(ARMED)))
    bad.push('a wrong word: ' + err);
  await ev(FILLW + ' ' + IN_SEC('[data-stop]') + '.click()'); await sleep(300);
  if (!(await ev(ARMED))) return bad.concat('the stop slide did not arm');
  await ev(SLIDE); await sleep(800);
  const cur = await ev(CUR);
  if (cur !== 'paperdone') bad.push('the stop ended on ' + cur);
  const page = await ev(HERE);
  if (!/Sent\. Waiting for the network to show it\./.test(page) || page.indexOf(VAULT) < 0)
    bad.push('an unconfirmed stop: ' + page);
  return bad.concat(await paperKeptNothing(ev));
}
/* word 1 from 3 letters and a tap: the box fills, the next box takes the focus; paste is refused */
async function wordTaps(ev) {
  const bad = [];
  const attrs = await ev("['autocomplete', 'autocorrect', 'autocapitalize', 'spellcheck']"
    + '.map(function (a) { return ' + IN_SEC('[data-pw]') + '.getAttribute(a); }).join()');
  if (attrs !== 'off,off,none,false') bad.push('a word box lets the keyboard learn: ' + attrs);
  await ev(IN_SEC('[data-pw]') + ".value = 'zeb'; " + IN_SEC('[data-pw]')
    + ".dispatchEvent(new Event('input'))");
  const hint = await ev(IN_SEC('[data-wp]') + '.textContent');
  if (hint !== 'zebra') return ['3 letters offered: ' + hint];
  await ev(IN_SEC('[data-wp] button') + '.click()');
  const got = await ev('[' + IN_SEC('[data-pw]') + '.value, document.activeElement === '
    + SEC + ".querySelectorAll('[data-pw]')[1]]");
  if (JSON.stringify(got) !== '["zebra",true]') bad.push('the tap gave ' + JSON.stringify(got));
  const pasted = await ev("(function () { var e = new Event('paste', { cancelable: true });"
    + ' ' + IN_SEC('[data-pw]') + '.dispatchEvent(e); return e.defaultPrevented; })()');
  if (!pasted) bad.push('a word box takes a paste');
  return bad;
}
/* the check's meaning is the engine's stage alone; a moved account is an out-of-date code */
async function paperMeaning(open, ev) {
  const bad = [];
  const moved = { ok: false, error: 'The account has moved since this paper sheet was printed, and '
    + 'this phone cannot follow it yet.', code: '1a2b3c4d', vault_address: VAULT };
  const said = [[{ ok: true, live: false, stage: 'stopped' }, /^Already stopped: the funds are in/],
    [moved, /^This paper's code is out of date\./], [{ ok: true, live: false, stage: null },
      /^The account could not be found\.$/],
    [{ ok: false, error: 'Please wait a moment and try again.' },
      /^Please wait a moment and try again\.$/]];
  const fresh = { ok: false, no_keys: true, error: 'This phone has no keys yet.' };
  await open({ answers: { status: fresh } });
  await ev("KVApp.go('paperuse', {}, { root: true })"); await sleep(500);
  for (const [answer, want] of said) {
    await ev("window.__kvAnswers['paper.check'] = " + JSON.stringify(answer) + ';'
      + " document.querySelector('[data-code]').value = 'KVW-x';"
      + " document.querySelector('[data-check]').click()"); await sleep(400);
    const e = await ev(IN_SEC('[data-e]') + '.textContent');
    if (!want.test(e)) bad.push(JSON.stringify(answer) + ' said: ' + e);
  }
  return bad;
}
/* the engine cannot tell which button is free: the warning, only buttons with no guardian, slot */
async function paperSlot(open, ev) {
  const bad = [];
  const unsure = { ok: false, words: [], slot: null, error: 'This phone cannot tell whether stop '
    + 'slot 2 or 3 holds a guardian added before it kept its list. Choose the slot for the paper '
    + 'stop.' };
  await open({ stops: [{ which: 2, holder: 'guardian' }, { which: 3, holder: 'unknown' }],
    answers: { 'paper.make': unsure } });
  await ev("KVApp.go('paperstop', {}, { root: true })"); await sleep(500);
  await ev("document.querySelector('[data-make]').click()"); await sleep(700);
  const box = await ev("KVApp.ui.el.querySelector('[data-pbox]').textContent");
  if (!/If a guardian holds this button from before, they lose it/.test(box))
    bad.push('no warning: ' + box);
  const offer = await ev("[].map.call(document.querySelectorAll('[data-slot]'),"
    + ' function (b) { return b.dataset.slot; }).join()');
  if (offer !== '3') return bad.concat('buttons offered: ' + offer);
  await ev("window.__kvAnswers['paper.make'] = " + JSON.stringify(MADE)
    + "; document.querySelector('[data-slot]').click()"); await sleep(700);
  const slots = await ev(PAPER_CALLS('paper.make'));
  if (slots !== JSON.stringify([{ account: 1 }, { account: 1, slot: 3 }]))
    bad.push('make got ' + slots);
  if ((await ev(CUR)) !== 'papershow') bad.push('the chosen slot did not show the words');
  return bad;
}
/* Your rules > Who hears: on a live phone each real guardian and heir is a plain row (no
   switch: their alerts are on their own phone); the made-up Sam row is Sample only */
const HEARS = "(function () { var l = [].filter.call(document.querySelectorAll('.label'),"
  + ' function (e) { return /Who hears/.test(e.textContent); })[0];'
  + " return l ? [].map.call(l.nextElementSibling.querySelectorAll('.item'), function (i) {"
  + " return i.querySelector('.t').textContent + (i.querySelector('.switch') ? ' +switch' : '');"
  + ' }) : null; })()';
async function rulesHear(open, ev) {
  await open({ stops: [{ which: 2, holder: 'guardian' }, { which: 3, holder: 'unheld' }],
    names: { bb: 'Sam', hh: 'Ana' }, heirX: 'hh' });
  await ev("KVApp.go('protection', {}, { root: true })"); await sleep(900);
  const rows = await ev(HEARS);
  const want = ['You +switch', 'Sam', 'Ana'];
  return JSON.stringify(rows) === JSON.stringify(want) ? [] : ['who hears: '
    + JSON.stringify(rows)];
}
/* the guard's alerts list in Alerts; the bell breathes amber until Alerts opens */
const BELL = "document.querySelector('.bell').className";
async function bellNew(open, ev) {
  const bad = [], at = Math.floor(Date.now() / 1000);
  await open({ alerts: [{ at: at, kind: 'spend', text: 'Your account paid 2 KAS.' }] });
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1200);
  if (!/\bnew\b/.test(await ev(BELL))) bad.push('no amber dot: ' + (await ev(BELL)));
  await ev("document.querySelector('.bell').click()"); await sleep(900);
  if (!/Your account paid 2 KAS\.[\s\S]*Today, /.test(await ev(HERE)))
    bad.push('Alerts does not list it: ' + (await ev(HERE)).slice(0, 120));
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1200);
  if (/\bnew\b/.test(await ev(BELL))) bad.push('the dot stayed after Alerts');
  return bad;
}
/* bell for a watched account: a withdrawal waiting on an account this phone guards rings red,
   lists in Alerts by the owner's name, then the bell stays lit (still waiting) but goes quiet */
async function bellWatched(open, ev) {
  const bad = [];
  const sam = { id: 'w1', name: 'Sam', roles: { guardian: 2 } };
  await open({ answers: { 'watch.list': { ok: true, watched: [sam] },
    'watch.status': { ok: true, id: 'w1', stage: 'waiting', kas: 9, can_stop: true,
      can_claim: false, pending: { kas: 2, to: 'kaspa:qthief', ends_in_seconds: 600 } } } });
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1500);
  if (!/\bloud\b/.test(await ev(BELL))) bad.push('no red dot: ' + (await ev(BELL)));
  await ev("document.querySelector('.bell').click()"); await sleep(900);
  if (!/A withdrawal of 2 KAS is waiting on Sam\u2019s account/.test(await ev(HERE)))
    bad.push('Alerts does not list it: ' + (await ev(HERE)).slice(0, 160));
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1200);
  const b = await ev(BELL);
  if (/\b(loud|new)\b/.test(b) || !/\bhas\b/.test(b)) bad.push('after Alerts the bell is: ' + b);
  return bad;
}
/* the guided add: each door's first screen says the step and who scans or shows; the owner's
   last step names the person and shows a Done */
async function guideSteps(open, ev) {
  const bad = [];
  await open({});
  const steps = [["document.querySelector('[data-addwatch]').click()", /Step 1 of 3 · You show/],
    ["document.querySelector('[data-addhelper]').click()",
      /Step 1 of 3 · You scan[\s\S]*Scan their code/],
    ["KVApp.go('watchcode', { guide: 'Sam' }, { root: true })",
      /Step 3 of 3[\s\S]*Now let Sam scan/]];
  for (const [act, want] of steps) {
    await ev("KVApp.go('people', {}, { root: true })"); await sleep(900);
    await ev(act); await sleep(900);
    const t = await ev(HERE);
    if (!want.test(t)) bad.push('missing ' + want + ': ' + t.slice(0, 120));
  }
  return bad;
}
/* Home folds a helped account to its name, role and balance; the heir clock shows only after
   a tap, and the vault is a tab between Receive and Price */
async function homeFold(open, ev) {
  const bad = [];
  const sam = { id: 'w1', name: 'Sam', roles: { guardian: 2, heir: true } };
  await open({ answers: { 'watch.list': { ok: true, watched: [sam] },
    'watch.status': { ok: true, id: 'w1', stage: 'open', kas: 40, can_stop: true,
      can_claim: false, heir_seconds_left: 86400 * 200 } } });
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(1500);
  const rows = "document.querySelector('[data-watched]').textContent";
  /* the group folds to one line first (the scene stays in view) */
  if (!/People you help \(1\)/.test(await ev(rows)) || /40 KAS/.test(await ev(rows)))
    bad.push('folded group: ' + (await ev(rows)));
  await ev("document.querySelector('[data-fold=helped]').click()"); await sleep(400);
  if (!/40 KAS/.test(await ev(rows)) || /Heir Unlock/.test(await ev(rows)))
    bad.push('folded row: ' + (await ev(rows)));
  await ev("document.querySelector('[data-wx]').click()"); await sleep(400);
  if (!/Heir Unlock: 200 days left/.test(await ev(rows))) bad.push('open row: ' + (await ev(rows)));
  const tabs = await ev("[].map.call(document.querySelectorAll('#tabs button'), " +
    "function (b) { return b.textContent || b.getAttribute('aria-label'); }).join()");
  if (!/People,Vault$/.test(tabs)) bad.push('tabs: ' + tabs); /* Settings is in the menu */
  return bad;
}
/* panic PIN: kept hashed (never the digits), weak and mismatched PINs refused; on the lock screen a
   wrong PIN says no, the right one opens the duress view: purse only, no tabs, every other screen
   lands back on it, the account's balance is nowhere on it, an emergency stop call is refused,
   and the lock screen's own prompt leads back to it */
const PIN = 'KVApp.ui.el.querySelector';
async function pinType(v) {
  return PIN + "('[data-p]').value = '" + v + "'; " + PIN + "('[data-ok]').click()";
}
async function lockType(ev, v) {
  await ev(PIN + "('[data-pinv]').value = '" + v + "'; " + PIN +
    "('[data-pinv]').dispatchEvent(new Event('input'))");
  await sleep(500);
}
async function panicSetUp(ev, bad) {
  await ev("KVApp.go('panicpin', {})"); await sleep(600);
  await ev(await pinType('123456')); await sleep(200);
  if (!/harder to guess/.test(await ev(HERE))) bad.push('a straight run was taken');
  await ev(await pinType('482916')); await sleep(200);
  await ev(await pinType('482916'));
  /* the PIN is hashed on purpose slowly (PBKDF2 150k): wait for it, up to 6 s on a busy machine */
  let kept = '';
  for (let i = 0; i < 30 && !kept; i++) {
    await sleep(200);
    kept = await ev('window.__kvSettings.panic_pin || ""');
  }
  await sleep(300);
  if (!/^p2:[0-9a-f]{32}:[0-9a-f]{64}$/.test(kept) || kept.indexOf('482916') >= 0)
    bad.push('not kept hashed: ' + kept.slice(0, 20));
}
async function panicDuress(open, ev) {
  const bad = [];
  const purse = { ok: true, kas: 3, address: 'kaspa:qpurse000000000000000000000' };
  await open({ answers: { funding: purse } });
  await panicSetUp(ev, bad);
  /* the lock screen opens the phone's prompt by itself (the fake says yes): held off to type */
  await ev("KVApp.ui.bio = true; KVApp.go('lock', {}, { root: true })"); await sleep(600);
  await ev('KVApp.ui.bio = false; ' + PIN + "('[data-pin]').click()");
  await lockType(ev, '111111');
  if (await ev(CUR) !== 'lock' || !/Not your PIN/.test(await ev(HERE)))
    bad.push('a wrong PIN let in: ' + (await ev(CUR)));
  await lockType(ev, '482916');
  if (await ev(CUR) !== 'duress') return bad.concat('the panic PIN opened ' + (await ev(CUR)));
  const txt = await ev(HERE);
  if (/120|\b(vault|stop|guardian)/i.test(txt)) bad.push('duress shows: ' + txt.slice(0, 70));
  if (!(await ev("document.body.classList.contains('notabs')"))) bad.push('tabs show');
  for (const r of ['people', 'safe', 'protection', 'settings', 'alerts', 'home']) {
    await ev("KVApp.go('" + r + "', {})"); await sleep(150);
    if (await ev(CUR) !== 'duress') bad.push(r + ' opened in the duress view');
  }
  await ev('KVData.stop().then(function (r) { window.__kvStop = r.ok + " " + r.error; })');
  await sleep(600);
  const stop = String(await ev('window.__kvStop'));
  if (!/^false /.test(stop)) bad.push('an emergency stop call went through: ' + stop);
  await ev("KVApp.go('pursepay', {})"); await sleep(600);
  if (await ev("!!document.querySelector('[data-menu]')")) bad.push('a menu on Send');
  await ev("KVApp.go('lock', {}, { root: true })"); await sleep(600);
  await ev(PIN + "('[data-ok]').click()"); await sleep(900);
  if (await ev(CUR) !== 'duress') bad.push('the lock prompt left the duress view');
  return bad;
}
/* Statement: dated engine lines = this month only, undated ones marked, In = received + fund,
   Out = paid; with no dates at all on Live it says Recent activity and claims no totals */
const NOW_S = Math.floor(Date.now() / 1000);
const DATED = [
  { text: 'Paid 2 KAS', tx: 't1', kind: 'paid', kas: 2, at: NOW_S },
  { text: 'Received 5 KAS', tx: 't2', kind: 'received', kas: 5, at: NOW_S },
  { text: 'Paid 9 KAS long ago', tx: 't3', kind: 'paid', kas: 9, at: NOW_S - 40 * 86400 },
  { text: 'Account opened with 10 KAS', tx: 't4', kind: 'fund', kas: 10 },
];
async function statementOf(open, ev, history) {
  await open({ history: history });
  await ev("KVApp.go('statements', {}, { root: true })"); await sleep(900);
  return ev(HERE);
}
async function statementRows(open, ev) {
  const bad = [];
  const a = await statementOf(open, ev, DATED);
  if (!/This month, /.test(a)) bad.push('no This month header');
  if (/long ago/.test(a)) bad.push('a line from another month is listed');
  if (!/Account opened with 10 KAS.*?Date not recorded/.test(a)) bad.push('undated line unmarked');
  if (!/In this month: 5 KAS\. Out this month: 2 KAS\./.test(a))
    bad.push('totals: ' + a.slice(-90));
  const b = await statementOf(open, ev, DATED.map((h) => ({ text: h.text, tx: h.tx })));
  if (!/Recent activity, /.test(b) || /this month:/.test(b)) bad.push('undated: ' + b.slice(-90));
  return bad;
}
/* Settings: no Email alerts switch; About opens Privacy; setup says what nodes see */
async function settingsPrivacy(open, ev) {
  const bad = [];
  await open({});
  await ev("KVApp.go('prefs', {}, { root: true })"); await sleep(400);
  if (/Email alerts/.test(await ev(HERE))) bad.push('Email alerts still shows');
  await ev("KVApp.go('about', {}, { root: true })"); await sleep(400);
  await ev("KVApp.ui.el.querySelector('[data-go=privacy]').click()"); await sleep(400);
  const nodes = /Kaspa network nodes see your account's addresses and payments/;
  if ((await ev(CUR)) !== 'privacy' || !nodes.test(await ev(HERE)))
    bad.push('About did not open the privacy text');
  await ev("KVApp.go('makekeys', {}, { root: true })"); await sleep(400);
  if (!nodes.test(await ev(HERE))) bad.push('setup does not name what nodes see');
  return bad;
}
/* the stop sent the words once, with no fingerprint; no setting and no box keeps any of them */
async function paperKeptNothing(ev) {
  const bad = [];
  const sent = await ev('JSON.stringify(window.__kvPaper.filter(function (c) {'
    + " return c.m === 'paper.stop'; }))");
  const want = [{ m: 'paper.stop', a: { words: W24.join(' '), watch_code: 'KVW-printed' } }];
  if (sent !== JSON.stringify(want)) bad.push('paper.stop got ' + sent);
  if ((await ev('window.__kvSigner')) !== 0) bad.push('a paper stop asked for a fingerprint');
  const all = String(await ev('JSON.stringify(window.__kvSettings)')).toLowerCase();
  const kept = all.match(/[a-z]+/g) || [];
  if (W24.some((w) => kept.indexOf(w) >= 0)) bad.push('a setting holds a stop word');
  const left = await ev("[].some.call(document.querySelectorAll('[data-pw]'),"
    + ' function (i) { return !!i.value; })');
  if (left) bad.push('a word box still holds a word');
  return bad;
}
/* take funds out with two vault seeds (after a paper stop): code, two seeds, purse, review with
   the full addresses, slide, no fingerprint; the seed boxes are emptied after a refusal too */
const PURSE = 'kaspa:qpurse000000000000000000000';
const SEEDS = "var vs = document.querySelectorAll('[data-vs]'); vs[0].value = "
  + JSON.stringify(W24.join(' '))
  + '; vs[1].value = ' + JSON.stringify(W24.slice().reverse().join(' ')) + ';'
  + " document.querySelector('[data-purse]').click();"
  + " document.querySelector('[data-next]').click()";
const OUT_NO = "These vault seeds do not open this account's vault.";
/* a long-stopped account whose history the node no longer holds: paper.check refuses but still
   names the vault, so the take-out goes on to the seeds; with no vault address it stays put */
const CHECK = "The account's history could not be checked. No funds were moved.";
async function paperOutUnchecked(open, ev) {
  const bad = [];
  for (const addr of [VAULT, null]) {
    await open({ answers: { 'paper.check': { ok: false, error: CHECK, code: '1a2b3c4d',
      vault_address: addr } } });
    await ev("KVApp.go('paperout', {}, { root: true })"); await sleep(500);
    await ev("document.querySelector('[data-code]').value = 'KVW-printed';"
      + " document.querySelector('[data-check]').click()"); await sleep(600);
    const seeds = await ev("!document.querySelector('[data-seeds]').hidden");
    const say = await ev("document.querySelector('[data-say]').textContent");
    if (addr && (!seeds || !/not the account's history/.test(say)))
      bad.push('with the vault named it stopped: ' + say);
    if (!addr && seeds) bad.push('with no vault address it went on to the seeds');
  }
  return bad;
}
/* Your papers: a seed row reads the seed back from the phone's copy and comes back Done; a
   paper the phone cannot check is the person's Yes; Home counts what is left */
const COUNT = "document.querySelector('[data-count]').textContent";
async function papersList(open, ev) {
  const bad = [];
  await open({ answers: { 'keys.sheets': { ok: true, sheets: [W24, W24, W24] } } });
  await ev("KVApp.go('papers', {}, { root: true })"); await sleep(700);
  if ((await ev(COUNT)) !== '0 of 4 done') bad.push('count at start: ' + (await ev(COUNT)));
  await ev("document.querySelector('[data-paperrow=s2]').click()"); await sleep(700);
  if ((await ev(CUR)) !== 'sheet') return bad.concat('seed 2 row opened ' + (await ev(CUR)));
  await ev("document.querySelector('[data-go=sheetcheck]').click()"); await sleep(500);
  for (let k = 0; k < 3; k++) { await ev(PICK); await sleep(250); }
  await sleep(700);
  if ((await ev(CUR)) !== 'papers' || (await ev(COUNT)) !== '1 of 4 done')
    bad.push('after reading seed 2 back: ' + (await ev(CUR)) + ' ' + (await ev(COUNT)));
  if ((await ev('KVApp.ui.sheets')) !== null) bad.push('the seeds stayed in the page');
  await ev("document.querySelector('[data-paperrow=p]').click()"); await sleep(400);
  await ev("document.querySelector('#sheet [data-yes]').click()"); await sleep(700);
  if ((await ev(COUNT)) !== '2 of 4 done') bad.push('after Yes: ' + (await ev(COUNT)));
  await ev("KVApp.go('home', {}, { root: true })"); await sleep(900);
  const row = await ev("document.querySelector('[data-paper]').textContent");
  if (row !== 'Your papers: 2 of 4 done') bad.push('Home row: ' + row);
  return bad;
}
async function paperOut(open, ev) {
  const bad = [];
  await open({ answers: { 'paper.check': { ok: true, kas: 5, live: true, stage: 'stopped',
    code: '1a2b3c4d', vault_address: VAULT }, 'paper.takeout': { ok: false, error: OUT_NO } } });
  await ev("KVApp.go('paperdone', { kas: 5 }, { root: true })"); await sleep(400);
  await ev("document.querySelector('[data-go=paperout]').click()"); await sleep(500);
  if ((await ev(CUR)) !== 'paperout') return ['paperdone does not open the take-out screen'];
  await ev("document.querySelector('[data-code]').value = 'KVW-printed';"
    + " document.querySelector('[data-check]').click()"); await sleep(500);
  await ev(SEEDS); await sleep(300);
  const rev = await ev("[document.querySelector('[data-rv]').textContent,"
    + " document.querySelector('[data-rt]').textContent,"
    + " !!document.querySelector('[data-armed]')]");
  if (rev[0] !== VAULT || rev[1].indexOf(PURSE) !== 0 || rev[2]) bad.push('review: ' + rev);
  await ev("document.querySelector('[data-out]').click()"); await sleep(300);
  if (!(await ev(ARMED))) return bad.concat('the take-out slide did not arm');
  await ev(SLIDE); await sleep(800);
  const err = await ev("document.querySelector('[data-e]').textContent");
  const left = await ev("[].some.call(document.querySelectorAll('[data-vs]'),"
    + ' function (t) { return !!t.value; })');
  if (err.indexOf(OUT_NO) !== 0 || left)
    bad.push('refusal: ' + err + (left ? ' (seeds kept)' : ''));
  await ev("window.__kvAnswers['paper.takeout'] = { ok: true, tx: 'ab', kas: 5, to: '" + PURSE
    + "', confirmed: false }"); await ev(SEEDS); await sleep(300);
  await ev("document.querySelector('[data-out]').click()"); await sleep(300);
  await ev(SLIDE); await sleep(800);
  const page = await ev(HERE);
  const sentLine = /Sent\. Waiting for the network to confirm it\./.test(page);
  if ((await ev(CUR)) !== 'paperoutdone' || !sentLine
    || page.indexOf(PURSE) < 0) bad.push('result: ' + page);
  return bad.concat(await paperOutKept(ev));
}
/* the seeds never linger: Change and putting the app away empty them; one slide makes one call;
   the full destination stands right above the slide */
const VS_LEFT = "[].some.call(document.querySelectorAll('[data-vs]'),"
  + ' function (t) { return !!t.value; })';
const SEED_STEP = "!document.querySelector('[data-seeds]').hidden";
const TAKEOUTS = "window.__kvPaper.filter(function (c) { return c.m === 'paper.takeout'; }).length";
async function paperOutGuards(open, ev) {
  const bad = [];
  await open({ slowSend: 1500, answers: { 'paper.check': { ok: true, kas: 5, live: true,
    stage: 'stopped', code: '1a2b3c4d', vault_address: VAULT },
  'paper.takeout': { ok: false, error: OUT_NO } } });
  await ev("KVApp.go('paperout', {}, { root: true })"); await sleep(400);
  await ev("document.querySelector('[data-code]').value = 'KVW-printed';"
    + " document.querySelector('[data-check]').click()"); await sleep(500);
  await ev(SEEDS); await sleep(300);
  await ev("document.querySelector('[data-edit]').click()"); await sleep(200);
  if ((await ev(VS_LEFT)) || !(await ev(SEED_STEP))) bad.push('Change showed the seeds back');
  await ev(SEEDS); await sleep(300);
  await ev("window.__kvSettings.lock_after = '300'; window.KV_pause()"); await sleep(200);
  const away = [await ev(VS_LEFT), await ev(SEED_STEP)];
  await ev('window.KV_resume()'); await sleep(300);
  if (away[0] || !away[1]) bad.push('put away: seeds kept ' + away);
  await ev(SEEDS); await sleep(300);
  await ev("document.querySelector('[data-out]').click()"); await sleep(300);
  const note = await ev("var s = document.querySelector('.slide');"
    + " s && s.kvNote ? s.kvNote.textContent : ''");
  if (String(note).replace(/\s+/g, '') !== 'To' + PURSE) bad.push('slide note: ' + note);
  await ev(SLIDE); await sleep(150);
  const edit = await ev("document.querySelector('[data-edit]').hidden");
  await ev("document.querySelector('[data-edit]').click(); " + SEEDS
    + "; var b = document.querySelector('[data-out]'); if (b) b.click()"); await sleep(300);
  await ev(SLIDE); await sleep(2200);
  if (!edit) bad.push('Change shows while the take-out runs');
  if ((await ev(TAKEOUTS)) !== 1) bad.push('a second slide made another paper.takeout call');
  return bad;
}
async function paperOutKept(ev) {
  const bad = [];
  const sent = await ev('JSON.stringify(window.__kvPaper.filter(function (c) {'
    + " return c.m === 'paper.takeout'; })"
    + '.map(function (c) { return c.a; }))');
  const one = { seeds: [W24.join(' '), W24.slice().reverse().join(' ')],
    watch_code: 'KVW-printed' };
  if (sent !== JSON.stringify([one, one])) bad.push('paper.takeout got ' + sent);
  if ((await ev('window.__kvSigner')) !== 0) bad.push('taking funds out asked for a fingerprint');
  const all = String(await ev('JSON.stringify(window.__kvSettings)')).toLowerCase();
  const kept = all.match(/[a-z]+/g) || [];
  if (W24.some((w) => kept.indexOf(w) >= 0)) bad.push('a setting holds a seed word');
  return bad;
}
const MONEY = [
  ['Home: the lost-phone row shows with no guardian or heir, not with a guardian or paper stop',
    loneRow],
  ['engine flags: Delete not done stays put with its reason; keys already made go Home',
    engineFlags],
  ['Allow screenshots: On asks a fresh fingerprint, Off does not', shotsOn],
  ['put away during a send on a locked screen: the lock comes next, and only then', lockAfterJob],
  ['Send: a payee chip disarms the slide, and the slide refuses a changed draft', sendSlide],
  ['the guard switch: a plain switch both ways that shows what runs', guardOff],
  ['paper stop: one fingerprint, 24 words once, read back, Update now, the code', paperMake],
  ['paper stop with no keys: code, words checked by number, slide, stop', paperUse],
  ['take funds out: code, two vault seeds, full addresses, slide, no fingerprint, boxes emptied',
    paperOut],
  ['take-out: a long-stopped account whose history is gone still reaches the seeds',
    paperOutUnchecked],
  ['Your papers: a seed read back from the phone, a Yes, the count on Home', papersList],
  ['take out: Change and going away empty the seeds, one call per slide, address above it',
    paperOutGuards],
  ['paper check: stopped only on the engine stage; moved = an out-of-date code; wait as is',
    paperMeaning],
  ['paper stop slot unsure: the warning, only a button no guardian holds, then make with slot',
    paperSlot],
  ['Settings: no Email alerts; About opens Privacy; setup names what nodes see', settingsPrivacy],
  ['Your rules: who hears lists the real guardian and heir, plain rows, no Sam', rulesHear],
  ['bell: a new guard alert breathes amber, lists in Alerts, quiet after a look', bellNew],
  ['bell: a withdrawal on a watched account rings red, lists by name, stays lit', bellWatched],
  ['People: two doors, each guided step says who scans and who shows', guideSteps],
  ['Home: helped accounts fold, the heir clock opens on a tap, Vault is a tab', homeFold],
  ['new account: How it works first, once, then the covenant', newWalk],
  ['Statement: this month from dated lines, In and Out, Recent activity when undated',
    statementRows],
  ['panic PIN: kept hashed; the lock screen PIN opens the purse only, nothing else reachable',
    panicDuress],
];

const CH = findChrome(), prof = fs.mkdtempSync(path.join(os.tmpdir(), 'kvpeople-'));
const ch = spawn(CH, ['--headless', '--disable-gpu', '--remote-debugging-port=0',
  '--user-data-dir=' + prof, 'about:blank']);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let port = null, buf = '';
const done = (code) => { try { ch.kill(); } catch (e) {} setTimeout(() => process.exit(code),
  300); };
ch.stderr.on('data', (d) => { buf += d; const m =
  /listening on ws:\/\/127\.0\.0\.1:(\d+)\//.exec(buf); if (m && !port) { port =
  m[1]; run().catch((e) => { console.log('FAIL', e.message); done(2); }); } });
/* the whole run's budget grows with the suite (each test ~5-15 s); one test gets TEST_CAP, so a
   hang names its test instead of silently cutting off every test after it */
const TEST_CAP = 30000, T0 = Date.now();
const RUN_CAP = 20000 * (CASES.length + MONEY.length) + 30000;
setTimeout(() => { console.log('FAIL whole run over ' + RUN_CAP / 1000 + ' s'); done(2); },
  RUN_CAP);
/* resolves with the test's own result, or a one-line failure after TEST_CAP */
function capped(work) {
  let timer = null;
  const late = new Promise((r) => { timer = setTimeout(() =>
    r(['did not finish in ' + TEST_CAP / 1000 + ' s']), TEST_CAP); });
  return Promise.race([work, late]).finally(() => clearTimeout(timer));
}

/* one People case on a fresh page: rows, the two doors, no Remove on an unheld button,
   the heir row */
async function peopleCase(c, ev) {
  const bad = [];
  const rows = await ev(ROWS);
  if (JSON.stringify(rows) !== JSON.stringify(c.want)) bad.push('rows ' + JSON.stringify(rows));
  const doors = await ev("document.querySelectorAll('.door').length");
  if (doors !== 2) bad.push('People has ' + doors + ' doors, not two');
  if (c.noRemove) {
    await ev("document.querySelector('[data-helpers] .item').click()"); await sleep(300);
    if (await ev("!!document.querySelector('#sheet [data-hrm]')")) bad.push('an unheld button ' +
      'offers Remove');
    await ev("var x = document.querySelector('#sheet [data-no]'); if (x) x.click(" +
      ")"); await sleep(300);
  }
  const heir = await ev("(document.querySelector('[data-heirpurse]') || {}).textContent || ''");
  if (c.heirRow ? !/Received as heir/.test(heir) || !/12\.5 KAS/.test(heir) :
    heir !== '') bad.push('heir row ' + JSON.stringify(heir));
  if (c.heirRow) await heirPayBox(ev, bad);
  return bad;
}
/* an heir pay amount box refuses bad input before any slide opens; a good amount passes */
async function heirPayBox(ev, bad) {
  await ev("KVApp.go('heirpurse', {}, { root: true })"); await sleep(900);
  await ev("document.querySelector('[data-to]').value = 'kaspa:qtest00000000000000000000'");
  for (const [amt, refused] of [['abc', true], ['-5', true], ['2.5', false]]) {
    await ev("document.querySelector('[data-e]').textContent = ''; document.querySelector(" +
      "'[data-amt]').value = "
      + JSON.stringify(amt) + "; document.querySelector('[data-ok]').click()"); await sleep(400);
    const err = await ev("document.querySelector('[data-e]').textContent");
    const slid =
      await ev("!!document.querySelector('[data-armed]') || /Send/.test((document.querySelector(" +
        "'#sheet') || {}).textContent || '')");
    if (refused && (!/Enter an amount/.test(err) || slid)) bad.push('heir pay ' + amt +
      ' not refused: ' + JSON.stringify(err) + (slid ? ' (slide opened)' : ''));
    if (!refused && /Enter an amount/.test(err)) bad.push('heir pay ' + amt + ' refused');
  }
  await heirPaySums(ev, bad);
}
/* the typed sum is what the slide names, never "all of it" (the case holds 12.5); Max alone means
   everything, with the sum shown; an edit after arming puts the slide back to its button */
async function heirPaySums(ev, bad) {
  const note = "(function () { var s = document.querySelector('[data-armed]'); return s && " +
    "s.previousSibling ? s.previousSibling.textContent : ''; })()";
  const type = (v) => ev("(function () { var a = document.querySelector('[data-amt]'); a.value = " +
    JSON.stringify(v) + "; a.dispatchEvent(new Event('input')); })()");
  const arm = async () => {
    await ev("document.querySelector('[data-ok]').click()"); await sleep(400);
    return ev(note);
  };
  let n = await ev(note);
  if (!/Send 2\.5 KAS\. To /.test(n)) bad.push('heir slide does not name 2.5 KAS: ' +
    JSON.stringify(n));
  await type('12.5'); await sleep(200);
  if (await ev("!!document.querySelector('[data-armed]')")) bad.push('heir slide stayed armed ' +
    'after the amount was edited');
  n = await arm();
  if (!/Send 12\.5 KAS\. To /.test(n) || /all of it/.test(n)) bad.push('heir pay: a typed 12.5 ' +
    'became everything: ' + JSON.stringify(n));
  await type('1'); await ev("document.querySelector('[data-max]').click()"); await sleep(200);
  n = await arm();
  if (!/Send all of it, 12\.5 KAS\. To /.test(n)) bad.push('heir pay: Max does not name the sum: ' +
    JSON.stringify(n));
}
async function run() {
  const list = await (await fetch('http://127.0.0.1:' + port + '/json')).json();
  const ws = new WebSocket(list.find((t) => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  let id = 0; const pend = {}, errors = [];
  ws.onmessage = (ev) => { const m = JSON.parse(ev.data); if (m.id &&
    pend[m.id]) { pend[m.id](m); delete pend[m.id]; }
    if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.text); };
  const send = (method, params) => new Promise((r) => { const i = ++id; pend[i] =
    r; ws.send(JSON.stringify({ id: i, method, params: params || {} })); });
  const ev = async (e) => (await send('Runtime.evaluate', { expression: e,
    returnByValue: true })).result.result.value;
  await send('Runtime.enable'); await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1,
    mobile: true });
  let failed = 0, script = null;
  for (const c of CASES) {
    if (script) await send('Page.removeScriptToEvaluateOnNewDocument', { identifier: script });
    script = (await send('Page.addScriptToEvaluateOnNewDocument',
      { source: fake(c) })).result.identifier;
    errors.length = 0;
    await send('Page.navigate', { url: PAGE }); await sleep(2000);
    await ev('window.KVData.retryUnlock()'); await sleep(400);
    await ev("KVApp.go('people', {}, { root: true })"); await sleep(1200);
    const bad = await capped(peopleCase(c, ev));
    if (errors.length) bad.push('script errors: ' + errors.join(' | '));
    console.log((bad.length ? 'FAIL ' : 'ok   ') + c.name + (bad.length ? ': ' + bad.join('; ') :
      ''));
    failed += bad.length ? 1 : 0;
  }
  console.log(CASES.length - failed + '/' + CASES.length + ' people rules hold');
  const open = async (c) => {
    if (script) await send('Page.removeScriptToEvaluateOnNewDocument', { identifier: script });
    const added = await send('Page.addScriptToEvaluateOnNewDocument', { source: fake(c) });
    script = added.result.identifier;
    errors.length = 0;
    await send('Page.navigate', { url: PAGE }); await sleep(2000);
    await ev('window.KVData.retryUnlock()'); await sleep(400);
  };
  for (const [name, check] of MONEY) {
    const bad = await capped(check(open, ev));
    if (errors.length) bad.push('script errors: ' + errors.join(' | '));
    const say = bad.length ? 'FAIL ' + name + ': ' + bad.join('; ') : 'ok   ' + name;
    console.log(say);
    failed += bad.length ? 1 : 0;
  }
  console.log('ran in ' + Math.round((Date.now() - T0) / 1000) + ' s');
  done(failed ? 1 : 0);
}
