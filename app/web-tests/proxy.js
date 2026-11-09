/* Drive the real app on a proxy phone (an Android emulator or a debuggable test build) over adb + the WebView's
   DevTools socket. Library for e2e-proxy.js; `node app/web-tests/proxy.js <serial>` alone prints what the page shows.
   Needs: adb (ANDROID_HOME/platform-tools or on PATH), a debuggable build (WebView.setWebContentsDebuggingEnabled). */
'use strict';
const { execFileSync } = require('child_process'), path = require('path'), fs = require('fs'), os = require('os');

function adbPath() {
  const sdk = process.env.ANDROID_HOME || path.join(process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'), 'Android', 'Sdk');
  const f = path.join(sdk, 'platform-tools', process.platform === 'win32' ? 'adb.exe' : 'adb');
  return fs.existsSync(f) ? f : 'adb';
}
const ADB = adbPath();
function adb(serial, args, opts) {
  return execFileSync(ADB, ['-s', serial].concat(args), Object.assign({ encoding: 'utf8', timeout: 30000 }, opts || {}));
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* the app's WebView socket: webview_devtools_remote_<pid> (or chrome_devtools_remote on some images) */
function devtoolsSocket(serial) {
  const unix = adb(serial, ['shell', 'cat', '/proc/net/unix']);
  const names = (unix.match(/@(webview_devtools_remote_\d+|chrome_devtools_remote)/g) || []).map((s) => s.slice(1));
  const pid = adb(serial, ['shell', 'pidof', 'com.kaspavault.wallet']).trim();
  return names.find((n) => pid && n.endsWith('_' + pid)) || names[0] || null;
}

/* connect to the app page on `serial`; returns {ev, send, close, errors}. Port is chosen per serial. */
async function connect(serial, port) {
  const sock = devtoolsSocket(serial);
  if (!sock) throw new Error(serial + ': no WebView DevTools socket (is the app running and debuggable?)');
  adb(serial, ['forward', 'tcp:' + port, 'localabstract:' + sock]);
  let list = null;
  for (let i = 0; i < 20 && !list; i++) {
    try { list = await (await fetch('http://127.0.0.1:' + port + '/json')).json(); } catch (e) { await sleep(500); }
  }
  const page = (list || []).find((t) => t.type === 'page' && /index\.html/.test(t.url)) || (list || [])[0];
  if (!page) throw new Error(serial + ': the WebView lists no page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  let id = 0; const pend = {}, errors = [];
  ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.id && pend[d.id]) { pend[d.id](d); delete pend[d.id]; }
    if (d.method === 'Runtime.exceptionThrown') errors.push(d.params.exceptionDetails.text); };
  const send = (method, params) => new Promise((r) => { const i = ++id; pend[i] = r; ws.send(JSON.stringify({ id: i, method, params: params || {} })); });
  await send('Runtime.enable');
  const ev = async (expr) => {
    const r = await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.result && r.result.exceptionDetails) throw new Error(serial + ': ' + (r.result.exceptionDetails.exception || {}).description);
    return r.result && r.result.result ? r.result.result.value : undefined;
  };
  return { serial, ev, send, errors, close: () => { try { ws.close(); } catch (e) {} try { adb(serial, ['forward', '--remove', 'tcp:' + port]); } catch (e) {} } };
}

/* wait until `expr` is truthy in the page, up to `ms`; returns its value or throws with `what` */
async function waitFor(p, expr, ms, what) {
  const until = Date.now() + ms;
  for (;;) {
    const v = await p.ev(expr).catch(() => null);
    if (v) return v;
    if (Date.now() > until) throw new Error(p.serial + ': timed out waiting for ' + what);
    await sleep(1000);
  }
}

module.exports = { adb, connect, waitFor, sleep, devtoolsSocket };

if (require.main === module) {
  (async () => {
    const serial = process.argv[2];
    if (!serial) { console.error('usage: node app/web-tests/proxy.js <adb serial>'); process.exit(2); }
    const p = await connect(serial, 9300);
    console.log('screen:', await p.ev('KVApp && KVApp.ui.cur ? KVApp.ui.cur.name : "?"'));
    console.log('phase:', await p.ev('window.KVData && KVData.phase ? KVData.phase() : "?"'));
    console.log('text:', (await p.ev('document.body.innerText')).slice(0, 400).replace(/\s+/g, ' '));
    p.close();
  })().catch((e) => { console.error('FAIL', e.message); process.exit(1); });
}
