/* The phone page on any screen shape: Android 16 ignores the portrait lock on tablets and
   foldables, so each main screen is measured at a phone (portrait, landscape) and a tablet
   (portrait, landscape): no sideways scroll, the tab bar on screen and lined up with the content,
   the main button reachable, no clipped words, the content column capped on wide screens.
   Run from the repo root: node app/web-tests/layout.js (headless Chromium, off screen, like
   phone.js; KV_CHROME overrides). --boxes [WxH] prints key boxes (at 390x844 unless given)
   instead, for a before/after diff. --shot <dir> saves Home at 1280x800 and 844x390 there. */
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
  console.error('layout: no Chromium headless shell found (set KV_CHROME)');
  process.exit(2);
}
const PAGE = 'file:///' + path.join(__dirname, '..', 'web', 'index.html').replace(/\\/g, '/');
const SIZES = [[390, 844], [844, 390], [800, 1280], [1280, 800]];
const SCREENS = ['home', 'send', 'receive', 'people', 'settings', 'protection', 'watchcode',
  'heirpurse', 'howitworks'];
const PARAMS = { howitworks: '{ step: 6 }' }; /* the walk-through step that shows a peek of Home */
const BOXES = process.argv.includes('--boxes');
const AT = /^(\d+)x(\d+)$/.exec(process.argv[process.argv.indexOf('--boxes') + 1] || '');
const SHOT = process.argv.includes('--shot') ? process.argv[process.argv.indexOf('--shot') + 1]
  : null;

/* the fake engine (as in phone.js), plus KV_layout: what is wrong with the screen, in words */
const FAKE = `(function () {
  var settings = { live_local: '1', toured: '1' };
  function answer(m, a) {
    if (m === 'status') return { ok: true, account: a.account || 1, accounts: [1],
      next_account: 2, balance_kas: 120, limit_kas: 20, spent_kas: 0, left_kas: 20, safe_kas: 5,
      stopped: false, purse_kas: 3, notice_seconds: 86400, refresh_seconds: 0,
      sheets_on_phone: true, heir: false, history: [],
      purse_address: 'kaspa:qpurse000000000000000000000' };
    if (m === 'keys.public') return { ok: true, public: { cancel_x: 'aa', cancel2_x: 'bb',
      cancel3_x: 'cc' } };
    if (m === 'heir.purse') return { ok: true, address: 'kaspa:qheir', kas: 12.5 };
    return { ok: true };
  }
  window.KaspaVaultBridge = {
    engine: function () { return true; },
    getSetting: function (k) { return settings[k] || ''; },
    setSetting: function (k, v) { settings[k] = String(v); },
    vault: function (cb, id, m, args) {
      var r = JSON.stringify(answer(m, JSON.parse(args)));
      setTimeout(function () { window[cb](id, 200, r); }, 5);
    },
    confirmSigner: function (cb) { setTimeout(function () { window[cb]('ok'); }, 5); },
    confirmWithBiometric: function (cb) { setTimeout(function () { window[cb]('ok'); }, 5); },
    price: function () {}, copy: function () {}, screen: function () {},
    guardOn: function (on) { return !!on; }, guardRunning: function () { return false; },
    guardOff: function (cb) { setTimeout(function () { window[cb]('ok'); }, 5); }
  };
  function seen(e) { var r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }
  function all(ui, q) { return [].filter.call(ui.querySelectorAll(q), seen); }
  window.KV_layout = function () {
    var bad = [], W = innerWidth, H = innerHeight, ui = KVApp.ui.el, cs = getComputedStyle(ui);
    var sw = document.documentElement.scrollWidth;
    if (sw > W + 1) bad.push('page scrolls sideways (' + sw + ' > ' + W + ')');
    var off = all(ui, '*').filter(function (e) { var r = e.getBoundingClientRect();
      return r.left < -1 || r.right > W + 1; })[0]; /* glows (::before) may spill, boxes not */
    if (off) bad.push('off screen: ' + off.tagName.toLowerCase() + '.' + off.className);
    var ur = ui.getBoundingClientRect(), left = ur.left + parseFloat(cs.paddingLeft),
      right = ur.right - parseFloat(cs.paddingRight);
    var tabs = document.getElementById('tabs'), tb = tabs.getBoundingClientRect();
    var on = !document.body.classList.contains('notabs'), floor = on ? tb.top : H;
    if (on) {
      var b = all(tabs, 'button');
      if (tb.top < 0 || tb.bottom > H + 1 || b.some(function (x) { var r =
        x.getBoundingClientRect(); return r.left < -1 || r.right > W + 1 || r.bottom > H + 1; }))
        bad.push('tab bar off screen');
      var bl = b[0].getBoundingClientRect().left,
        br = b[b.length - 1].getBoundingClientRect().right;
      if (Math.abs(bl - left) > 2 || Math.abs(br - right) > 2) bad.push('tabs ' +
        Math.round(bl) + '-' + Math.round(br) + ' not lined up with the content ' +
        Math.round(left) + '-' + Math.round(right));
    }
    var pill = all(ui, '.pill')[0];
    if (pill) { /* scrolled all the way down, the button is at its highest: above the tabs? */
      ui.style.scrollBehavior = 'auto'; ui.scrollTop = ui.scrollHeight;
      var p = pill.getBoundingClientRect();
      if (p.left < -1 || p.right > W + 1 || p.bottom > floor + 1 || p.height > floor) bad.push(
        'main button "' + pill.textContent.trim() + '" out of reach (' + Math.round(p.top) + '-' +
        Math.round(p.bottom) + ', floor ' + Math.round(floor) + ')');
      ui.scrollTop = 0;
    }
    all(ui, '.line,.sub,.label,.item .t').forEach(function (e) {
      if (e.scrollWidth > e.clientWidth + 2) bad.push('clipped "' +
        e.textContent.trim().slice(0, 30) + '" (' + e.scrollWidth + ' > ' + e.clientWidth + ')');
    });
    all(ui, '.peek .glass').forEach(function (g) { /* a peek stays a phone: glass edge to edge */
      var r = g.getBoundingClientRect(), k = g.closest('.peek').getBoundingClientRect();
      if (Math.abs(r.left - k.left) > 1 || Math.abs(r.right - k.right) > 1) bad.push(
        'peek glass ' + Math.round(r.left) + '-' + Math.round(r.right) + ' not edge to edge');
    });
    var wide = Math.max.apply(null, [0].concat(all(ui, '.list,.pill,.glass>*').map(function (e) {
      return e.getBoundingClientRect().width; })));
    if (wide > 562) bad.push('content stretches to ' + Math.round(wide) + ' px');
    var row = all(ui, '[data-helpers] .item')[0]; /* People: a row's pop-up sheet, centred */
    if (row) {
      row.click();
      var q = document.querySelector('#sheet .pane'), sr = q && q.getBoundingClientRect();
      if (!q || sr.width > Math.min(W, 616) + 1 || Math.abs(sr.left + sr.right - W) > 2)
        bad.push('sheet ' + (q ? Math.round(sr.left) + '-' + Math.round(sr.right) : 'missing'));
      KV_back();
    }
    return bad;
  };
  window.KV_boxes = function () { /* the key boxes, for the portrait before/after diff */
    var ui = KVApp.ui.el, out = {};
    [['big', '.big'], ['pill', '.pill'], ['list', '.list'], ['glass', '.glass'],
      ['top', '.top']].forEach(function (q) {
      var e = all(ui, q[1])[0];
      if (e) { var r = e.getBoundingClientRect(); out[q[0]] = [r.left, r.top, r.width,
        r.height].map(Math.round).join(','); }
    });
    var t = document.getElementById('tabs').getBoundingClientRect();
    out.tabs = [t.left, t.top, t.width, t.height].map(Math.round).join(',');
    out.scroll = ui.scrollHeight + '/' + ui.clientHeight;
    return out;
  };
})();`;

const CH = findChrome(), prof = fs.mkdtempSync(path.join(os.tmpdir(), 'kvlayout-'));
const ch = spawn(CH, ['--headless', '--disable-gpu', '--remote-debugging-port=0',
  '--user-data-dir=' + prof, 'about:blank']);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let port = null, buf = '';
const done = (code) => { try { ch.kill(); } catch (e) {} setTimeout(() => process.exit(code),
  300); };
ch.stderr.on('data', (d) => { buf += d; const m =
  /listening on ws:\/\/127\.0\.0\.1:(\d+)\//.exec(buf); if (m && !port) { port =
  m[1]; run().catch((e) => { console.log('FAIL', e.message); done(2); }); } });
setTimeout(() => { console.log('FAIL timeout'); done(2); }, 180000);

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
  await send('Page.addScriptToEvaluateOnNewDocument', { source: FAKE });
  let failed = 0, n = 0;
  const sizes = BOXES ? [AT ? [+AT[1], +AT[2]] : SIZES[0]] : SHOT ? [SIZES[3], SIZES[1]] : SIZES;
  for (const [w, h] of sizes) {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h,
      deviceScaleFactor: 1, mobile: true });
    errors.length = 0;
    await send('Page.navigate', { url: PAGE }); await sleep(2000);
    await ev('window.KVData.retryUnlock()'); await sleep(400);
    for (const s of SHOT ? ['home'] : SCREENS) {
      await ev("KVApp.go('" + s + "', " + (PARAMS[s] || '{}') + ", { root: true })");
      await sleep(1100);
      if (SHOT) {
        const shot = await send('Page.captureScreenshot', { format: 'png', clip: { x: 0, y: 0,
          width: w, height: h, scale: 0.5 } });
        const f = path.join(SHOT, 'layout-home-' + w + 'x' + h + '.png');
        fs.writeFileSync(f, Buffer.from(shot.result.data, 'base64')); console.log('shot', f);
        continue;
      }
      if (BOXES) { console.log(s, JSON.stringify(await ev('KV_boxes()'))); continue; }
      const bad = (await ev('KV_layout()')) || ['KV_layout did not run'];
      if (errors.length) bad.push('script errors: ' + errors.splice(0).join(' | '));
      console.log((bad.length ? 'FAIL ' : 'ok   ') + w + 'x' + h + ' ' + s +
        (bad.length ? ': ' + bad.join('; ') : ''));
      n++; failed += bad.length ? 1 : 0;
    }
  }
  if (!BOXES && !SHOT) console.log(n - failed + '/' + n + ' screen shapes hold');
  done(failed ? 1 : 0);
}
