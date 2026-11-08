/* Loads app/web/data.js in a Node sandbox with a fake Java bridge standing in for the phone shell and the engine.
   The page only ever reaches the outside through window.KaspaVaultBridge, so this fake is the whole world it sees.
   Nothing here ships: build.sh copies app/web only. */
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const DATA_JS = path.join(__dirname, '..', 'web', 'data.js');

/* engine: function (method, args) -> { status, body } or a body (status 200). Unknown methods answer 404.
   fingerprint: 'ok' or anything else, or a function (why) -> answer, for each confirmSigner / confirmWithBiometric prompt. */
function load(opts) {
  opts = opts || {};
  const settings = Object.assign({ live_local: '1' }, opts.settings || {});
  const calls = [];      // every engine call: { method, args }
  const prompts = [];    // every fingerprint prompt: { kind, why }
  const pending = [];    // callbacks waiting to be delivered (the bridge answers asynchronously, as Java does)
  let fingerprint = opts.fingerprint === undefined ? 'ok' : opts.fingerprint;
  let engine = opts.engine || function () { return { status: 404, body: '' }; };

  const ctx = {
    console, JSON, Math, Date, Promise, Uint8Array, parseInt, parseFloat, isNaN, isFinite, encodeURIComponent, decodeURIComponent,
    crypto: globalThis.crypto,
    location: { search: '' },
    document: { hidden: false, addEventListener() {} },
    localStorage: { getItem() { return null; }, setItem() {} },
    setInterval() { return 0; }, clearInterval() {},   // the page's 1 s tick and 5 s poll never fire: tests drive poll themselves
    setTimeout(f) { deliver(f); return 0; }, clearTimeout() {},   // a wait ends at once (the engine's "trying again" retries)
    fetch() { return Promise.reject(new Error('no network in tests')); },
  };
  ctx.window = ctx;
  function deliver(fn) { pending.push(fn); setImmediate(flush); }
  function flush() { while (pending.length) pending.shift()(); }
  ctx.KaspaVaultBridge = {
    engine() { return true; },
    getSetting(k) { return settings[k] || ''; },
    setSetting(k, v) { settings[k] = String(v); },
    vault(cb, id, method, argsJson) {
      const args = JSON.parse(argsJson);
      calls.push({ method, args });
      let r = engine(method, args);
      if (!r || typeof r.status !== 'number') r = { status: 200, body: r };
      const text = typeof r.body === 'string' ? r.body : JSON.stringify(r.body);
      deliver(() => ctx[cb](id, r.status, text));
    },
    confirmSigner(cb, why) { prompts.push({ kind: 'signer', why }); const a = typeof fingerprint === 'function' ? fingerprint(why) : fingerprint; deliver(() => ctx[cb](a)); },
    confirmWithBiometric(cb, why) { prompts.push({ kind: 'open', why }); const a = typeof fingerprint === 'function' ? fingerprint(why) : fingerprint; deliver(() => ctx[cb](a)); },
    price() {}, keysNew() {}, copy() {},
  };
  vm.createContext(ctx);
  vm.runInContext(fs.readFileSync(DATA_JS, 'utf8'), ctx, { filename: 'data.js' });
  const D = ctx.KVData;
  return {
    D, ctx, calls, prompts, settings,
    setEngine(f) { engine = f; },
    setFingerprint(a) { fingerprint = a; },
    callsOf(method) { return calls.filter(c => c.method === method); },
    /* boot the page and open it with one fingerprint, as the lock screen does */
    async open() { D.boot(); await D.retryUnlock(); await tick(); return D.state; },
  };
}

/* let the fake bridge's answers land */
function tick() { return new Promise(r => setImmediate(() => setImmediate(r))); }

/* a status body as the engine sends it for an open account */
function status(over) {
  return Object.assign({
    ok: true, account: 1, accounts: [1], next_account: 2, rules_next: null,
    balance_kas: 120, limit_kas: 20, spent_kas: 0, left_kas: 20, safe_kas: 0, stopped: false,
    purse_kas: 3, purse_address: 'kaspa:qpursepursepursepursepursepurse0', safe_address: 'kaspa:qsafesafesafesafesafesafesafe0',
    notice_seconds: 86400, refresh_seconds: 0, sheets_on_phone: true, history: [],
  }, over || {});
}

module.exports = { load, tick, status };
