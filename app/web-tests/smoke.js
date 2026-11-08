/* Opens the desktop demo (demo.html, Sample mode) in a headless Chromium, off screen, and visits every screen in the
   SCR table, then taps every visible button on each; any script error is a failure. Catches what the data-layer tests
   cannot: a helper that lost a variable, a screen that throws while drawing. Run from the repo root: node app/web-tests/smoke.js
   Uses a Playwright Chromium headless shell: KV_CHROME names its executable, otherwise it is looked up in the Playwright
   browser cache (PLAYWRIGHT_BROWSERS_PATH or the platform default). No window ever opens. */
'use strict';
const { spawn } = require('child_process'), fs = require('fs'), os = require('os'), path = require('path');
function findChrome() {
  if (process.env.KV_CHROME) return process.env.KV_CHROME;
  const home = os.homedir();
  const roots = [process.env.PLAYWRIGHT_BROWSERS_PATH,
    process.platform === 'win32' ? path.join(process.env.LOCALAPPDATA || path.join(home, 'AppData', 'Local'), 'ms-playwright')
      : process.platform === 'darwin' ? path.join(home, 'Library', 'Caches', 'ms-playwright') : path.join(home, '.cache', 'ms-playwright')];
  const exe = process.platform === 'win32' ? 'chrome-headless-shell.exe' : 'chrome-headless-shell';
  for (const root of roots) {
    if (!root || !fs.existsSync(root)) continue;
    const shells = fs.readdirSync(root).filter((d) => d.startsWith('chromium_headless_shell-')).sort().reverse();
    for (const d of shells) {
      for (const sub of fs.readdirSync(path.join(root, d))) {
        const f = path.join(root, d, sub, exe);
        if (fs.existsSync(f)) return f;
      }
    }
  }
  console.error('smoke: no Chromium headless shell found. Set KV_CHROME to its executable, or install one with: npx playwright install chromium-headless-shell');
  process.exit(2);
}
const CH = findChrome();
/* --live: the phone page itself (index.html) with a fake bridge injected before it starts, so the phone's own branches run */
const LIVE = process.argv.includes('--live');
const PAGE = 'file:///' + path.join(__dirname, '..', 'web', LIVE ? 'index.html' : 'demo.html').replace(/\\/g, '/');
const FAKE_BRIDGE = `(function () {
  var settings = { live_local: '1', toured: '1' };
  function later(f) { setTimeout(f, 5); }
  function status(a) {
    return { ok: true, account: a || 1, accounts: [1, 2], next_account: 3, rules_next: (a === 2 ? { dials: {}, words: ['20 KAS a day without waiting'] } : null),
      balance_kas: 120, limit_kas: 20, spent_kas: 0, left_kas: 20, safe_kas: 5, stopped: false, purse_kas: 3,
      purse_address: 'kaspa:qpursepursepursepursepursepurse0', safe_address: 'kaspa:qsafesafesafesafesafesafesafe0',
      notice_seconds: 86400, refresh_seconds: 0, sheets_on_phone: true, heir: false, history: [] };
  }
  function answer(m, a) {
    if (m === 'status') return status(a.account);
    if (m === 'funding') return { ok: true, kas: 3, address: 'kaspa:qpursepursepursepursepursepurse0' };
    if (m === 'keys.public') return { ok: true };
    if (m === 'rules.change') return { ok: true, route: 'move', steps: ['Stop the account', 'Take the money out', 'Open it again'], fee_kas: 0.001, fee_kas_about: 0.003, dials: {}, words: ['20 KAS a day'] };
    return { ok: true };
  }
  window.KaspaVaultBridge = {
    engine: function () { return true; },
    getSetting: function (k) { return settings[k] || ''; }, setSetting: function (k, v) { settings[k] = String(v); },
    vault: function (cb, id, m, args) { var a = JSON.parse(args); later(function () { window[cb](id, 200, JSON.stringify(answer(m, a))); }); },
    confirmSigner: function (cb) { later(function () { window[cb]('ok'); }); },
    confirmWithBiometric: function (cb) { later(function () { window[cb]('ok'); }); },
    price: function () {}, keysNew: function () {}, copy: function () {}, screen: function () {}, guardOn: function () { return false; }, guardRunning: function () { return false; }
  };
})();`;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const prof = fs.mkdtempSync(path.join(os.tmpdir(), 'kvsmoke-'));
const ch = spawn(CH, ['--headless', '--disable-gpu', '--remote-debugging-port=0', '--window-size=375,812', '--user-data-dir=' + prof, 'about:blank']);
let port = null, buf = '';
const done = code => { try { ch.kill(); } catch (e) {} setTimeout(() => process.exit(code), 300); };
ch.stderr.on('data', d => { buf += d; const m = /DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)\//.exec(buf); if (m && !port) { port = m[1]; run().catch(e => { console.log('FAIL', e.message); done(2); }); } });
setTimeout(() => { console.log('FAIL timeout'); done(2); }, 240000);

async function run() {
  const list = await (await fetch('http://127.0.0.1:' + port + '/json')).json();
  const ws = new WebSocket(list.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise(r => ws.onopen = r);
  let id = 0; const pend = {}; let errors = [];
  ws.onmessage = ev => {
    const m = JSON.parse(ev.data);
    if (m.id && pend[m.id]) { pend[m.id](m); delete pend[m.id]; }
    if (m.method === 'Runtime.exceptionThrown') { const d = m.params.exceptionDetails; errors.push((d.exception && d.exception.description || d.text).split('\n').slice(0, 2).join(' | ')); }
    if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push('console.error ' + m.params.args.map(a => a.value || a.description).join(' '));
  };
  const send = (method, params) => new Promise(r => { const i = ++id; pend[i] = r; ws.send(JSON.stringify({ id: i, method, params: params || {} })); });
  const evaluate = async expr => { const r = await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }); return r.result && r.result.result ? r.result.result.value : undefined; };
  await send('Runtime.enable'); await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', { width: 375, height: 812, deviceScaleFactor: 1, mobile: true });
  if (LIVE) await send('Page.addScriptToEvaluateOnNewDocument', { source: FAKE_BRIDGE });
  const load = async () => { await send('Page.navigate', { url: PAGE }); await sleep(1500); };
  await load();
  if (LIVE) { await evaluate('window.KVData.retryUnlock()'); await sleep(400); }
  if (errors.length) { console.log('✖ the demo page throws while starting:\n  ' + errors.join('\n  ')); return done(1); }
  const screens = await evaluate('Object.keys(window.KVApp.screens)');
  if (!screens || !screens.length) { console.log('✖ window.KVApp.screens is missing'); return done(1); }
  /* screens the app always opens with parameters: the smoke opens them the same way */
  const PARAMS = {
    waiting: { job: 'send', to: 'kaspa:qsmokesmokesmokesmokesmokesmoke0', label: 'Smoke', kas: 1, wait: false },
    shareinvite: { name: 'Sam' },
  };
  /* Sample-only screens: the phone has no route to them (they read Sample's Everyday account) */
  const SAMPLE_ONLY = LIVE ? ['open', 'movein'] : [];
  const bad = {};
  for (const name of screens.filter(s => SAMPLE_ONLY.indexOf(s) < 0)) {
    errors = [];
    const p = JSON.stringify(PARAMS[name] || {});
    await evaluate(`(function(){ try { window.KVApp.go(${JSON.stringify(name)}, ${p}, { root: true }); } catch (e) { setTimeout(function(){ throw e; }); } })()`);
    await sleep(350);
    /* tap each visible button once, back to the same screen between taps; a tap that leaves the page or moves money in Sample is fine */
    const n = await evaluate(`document.querySelectorAll('section.ui:last-of-type button:not([disabled])').length`) || 0;
    for (let i = 0; i < Math.min(n, 12); i++) {
      await evaluate(`(function(){ var b = document.querySelectorAll('section.ui:last-of-type button:not([disabled])')[${i}]; if (b && b.offsetParent !== null) b.click(); })()`);
      await sleep(120);
      await evaluate(`(function(){ var s = document.querySelector('.sheet [data-no], .sheet .ghost'); if (s) s.click(); window.KVApp.go(${JSON.stringify(name)}, ${p}, { root: true }); })()`);
      await sleep(120);
    }
    if (errors.length) bad[name] = errors.slice(0, 3);
  }
  const names = Object.keys(bad);
  console.log(screens.length + ' screens visited, ' + names.length + ' with script errors');
  names.forEach(k => console.log('✖ ' + k + ':\n    ' + bad[k].join('\n    ')));
  done(names.length ? 1 : 0);
}
