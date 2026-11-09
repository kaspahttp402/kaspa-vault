/* Hostile-input and edge checks for the seed-word check and the QR maker.
   Run from the repo root: node --test app/web-tests/seedqr.test.js

   What is tested where:
   - app/web/bip39.js is only the BIP-39 English word list (window.KV_BIP39, an array). It has no check function and no checksum.
   - The one place that checks typed seed words against the list is sheet() inside the takeoutwords screen of app/web/app.js.
     It is a closure, so this file cuts its source (and the line that builds its word list) out of app.js and runs that exact
     text in a sandbox. Nothing is copied by hand, so an edit to app.js is what gets tested.
   - Tests named "... (full list)" run sheet() against the whole KV_BIP39 array, to pin down sheet()'s own case, space and
     unicode handling. Tests without that note use the list exactly as app.js builds it.
   - app/web/qr.js is loaded the same way qr.test.js does it and decoded back with app/web/jsqr.js.

   { todo } marks a check that fails today because of a bug in app/web (the run stays green; remove the todo once fixed).
   { skip } marks a check the app does not try to do at all. */
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { performance } = require('perf_hooks');

const WEB = path.join(__dirname, '..', 'web');
const LAPTOP_LIST = path.join(__dirname, '..', '..', 'testnet-client', 'src', 'bip39-english.txt');

/* ---------- sandbox: bip39.js, qr.js, jsqr.js ---------- */
const ctx = { TextEncoder, TextDecoder, console, Uint8ClampedArray, Int32Array, Uint8Array, Float32Array, Math, Array, Object, String };
ctx.window = ctx; ctx.self = ctx;
vm.createContext(ctx);
for (const f of ['bip39.js', 'jsqr.js', 'qr.js']) vm.runInContext(fs.readFileSync(path.join(WEB, f), 'utf8'), ctx, { filename: f });
const { KVQR, jsQR } = ctx;
const WORDS = Array.from(ctx.KV_BIP39); // main-realm copy of the list

/* ---------- cut sheet() and its list line out of app.js ---------- */
const APP = fs.readFileSync(path.join(WEB, 'app.js'), 'utf8');

function count(hay, needle) { let n = 0, i = -1; while ((i = hay.indexOf(needle, i + 1)) >= 0) n++; return n; }

/* source of the function that starts at `head`, matched by braces (skips strings and comments) */
function cutFunction(src, head) {
  const at = src.indexOf(head);
  if (at < 0) return null;
  let depth = 0, q = null;
  for (let i = src.indexOf('{', at); i < src.length; i++) {
    const c = src[i];
    if (q) { if (c === '\\') i++; else if (c === q) q = null; continue; }
    if (c === "'" || c === '"' || c === '`') { q = c; continue; }
    if (c === '/' && src[i + 1] === '*') { i = src.indexOf('*/', i + 2) + 1; continue; }
    if (c === '/' && src[i + 1] === '/') { i = src.indexOf('\n', i); continue; }
    if (c === '{') depth++;
    else if (c === '}' && --depth === 0) return src.slice(at, i + 1);
  }
  return null;
}

const SHEET_HEAD = 'function sheet(sel) {';
const SHEET_SRC = cutFunction(APP, SHEET_HEAD);
/* the line holding window.KV_BIP39 is "var list = <expr>," : keep <expr> */
const LIST_LINE = (APP.split('\n').find((l) => l.includes('window.KV_BIP39')) || '').trim();
const LIST_EXPR = LIST_LINE.replace(/^var\s+list\s*=\s*/, '').replace(/,$/, '');
const makeSheet = SHEET_SRC ? vm.runInContext('(function (list, el) {\n' + SHEET_SRC + '\nreturn sheet;\n})', ctx, { filename: 'app.js#sheet' }) : null;
const APP_LIST = LIST_EXPR ? vm.runInContext('(' + LIST_EXPR + ')', ctx, { filename: 'app.js#list' }) : null;
const FULL_LIST = ctx.KV_BIP39;

/* what the takeoutwords screen says for this textarea text: the joined words when accepted, else the error line */
function check(text, list = APP_LIST) {
  assert.ok(makeSheet, 'sheet() not found in app.js');
  const el = { querySelector: () => ({ value: text }) };
  return makeSheet(list, el)('[data-w1]');
}
const full = (text) => check(text, FULL_LIST);
const COUNT_ERR = 'Please enter all 24 words.';

/* ---------- BIP-39 maths (node crypto), to make and judge test phrases ---------- */
function toMnemonic(entropy) {
  const cs = entropy.length / 4; // checksum bits
  const hash = crypto.createHash('sha256').update(entropy).digest();
  let bits = '';
  for (const b of entropy) bits += b.toString(2).padStart(8, '0');
  bits += [...hash].map((b) => b.toString(2).padStart(8, '0')).join('').slice(0, cs);
  return bits.match(/.{11}/g).map((g) => WORDS[parseInt(g, 2)]).join(' ');
}
function checksumOk(phrase) {
  const idx = phrase.split(' ').map((w) => WORDS.indexOf(w));
  if (idx.some((i) => i < 0)) return false;
  const bits = idx.map((i) => i.toString(2).padStart(11, '0')).join('');
  const csLen = bits.length / 33, entBits = bits.slice(0, bits.length - csLen);
  const ent = Buffer.from(entBits.match(/.{8}/g).map((b) => parseInt(b, 2)));
  return toMnemonic(ent) === phrase;
}

const VALID = 'legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title';
const CHANGED = VALID.split(' ').map((w, i) => (i === 5 ? 'abandon' : w)).join(' '); // word 6 swapped for another list word
const ABANDON23 = Array(23).fill('abandon');

/* =====================  bip39.js: the word list  ===================== */

test('the word list has exactly 2048 words', () => {
  assert.strictEqual(WORDS.length, 2048);
});

test('every list word is plain lower-case a to z (no hidden unicode or spaces)', () => {
  const bad = WORDS.filter((w) => !/^[a-z]{3,8}$/.test(w));
  assert.deepStrictEqual(bad, []);
});

test('no word appears twice', () => {
  assert.strictEqual(new Set(WORDS).size, WORDS.length);
});

test('the list is in alphabetical order', () => {
  assert.deepStrictEqual(WORDS, [...WORDS].sort());
});

test('the first four letters of every word are unique', () => {
  assert.strictEqual(new Set(WORDS.map((w) => w.slice(0, 4))).size, WORDS.length);
});

test('the list matches the laptop service list word for word', { skip: !fs.existsSync(LAPTOP_LIST) && 'testnet-client/src/bip39-english.txt not found' }, () => {
  const laptop = fs.readFileSync(LAPTOP_LIST, 'utf8').split(/\r?\n/).filter(Boolean);
  assert.deepStrictEqual(WORDS, laptop);
});

test('the list gives the official BIP-39 test-vector phrases', () => {
  assert.strictEqual(toMnemonic(Buffer.alloc(32, 0x00)), [...ABANDON23, 'art'].join(' '));
  assert.strictEqual(toMnemonic(Buffer.alloc(32, 0x7f)), VALID);
  assert.strictEqual(toMnemonic(Buffer.alloc(32, 0x80)),
    'letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless');
  assert.strictEqual(toMnemonic(Buffer.alloc(32, 0xff)), [...Array(23).fill('zoo'), 'vote'].join(' '));
});

/* =====================  app.js sheet(): the seed-word check  ===================== */

test('the seed check is found in app.js exactly once', () => {
  assert.strictEqual(count(APP, SHEET_HEAD), 1, 'expected one "' + SHEET_HEAD + '" in app.js');
  assert.ok(SHEET_SRC && SHEET_SRC.includes('indexOf'), 'could not cut sheet() out of app.js');
  assert.strictEqual(count(APP, 'window.KV_BIP39'), 1, 'expected one use of window.KV_BIP39 in app.js');
  assert.ok(Array.isArray(APP_LIST), 'the list line did not give an array: ' + LIST_LINE);
});

/* BUG: bip39.js sets KV_BIP39 to an array; String(array) joins it with commas, so split(/\s+/) gives ONE long item.
   sheet() then skips the list check because of its "list.length > 100" guard, and any 24 tokens pass. */
test('the list the seed check uses holds all 2048 words', () => {
  assert.strictEqual(APP_LIST.length, 2048);
});

test('every list word is accepted', () => {
  for (let i = 0; i < WORDS.length; i += 24) {
    const batch = WORDS.slice(i, i + 24);
    while (batch.length < 24) batch.push('zoo');
    const phrase = batch.join(' ');
    assert.strictEqual(check(phrase), phrase, 'refused batch starting at word ' + (i + 1));
  }
});

test('a word that is not on the list is refused', () => {
  assert.strictEqual(check([...ABANDON23, 'notaword'].join(' ')), 'Word 24 is not on the list.');
});

test('a word not on the list is refused, naming its place (full list)', () => {
  assert.strictEqual(full(['qwerty', ...ABANDON23].join(' ')), 'Word 1 is not on the list.');
  assert.strictEqual(full([...ABANDON23.slice(0, 11), 'bitcoins', ...ABANDON23.slice(11)].join(' ')), 'Word 12 is not on the list.');
});

test('the refusal never repeats the typed word (full list)', () => {
  const said = full([...ABANDON23, 'secretword'].join(' '));
  assert.ok(!said.includes('secretword'), said);
});

test('a BIP-39 four-letter short form is refused (full list)', () => {
  assert.strictEqual(full([...ABANDON23, 'aban'].join(' ')), 'Word 24 is not on the list.');
});

test('upper and mixed case are accepted and lower-cased (full list)', () => {
  const words = [...ABANDON23, 'art'];
  assert.strictEqual(full(words.join(' ').toUpperCase()), words.join(' '));
  assert.strictEqual(full(words.map((w, i) => (i % 2 ? w.toUpperCase() : w[0].toUpperCase() + w.slice(1))).join(' ')), words.join(' '));
});

test('extra spaces, tabs and new lines around and between words are accepted and tidied (full list)', () => {
  const words = [...ABANDON23, 'art'];
  const messy = '\n\t  ' + words.map((w, i) => w + (i % 3 === 0 ? '\n' : i % 3 === 1 ? '\t\t' : '   ')).join('') + '  \r\n';
  assert.strictEqual(full(messy), words.join(' '));
});

test('23 words are refused', () => {
  assert.strictEqual(check(ABANDON23.join(' ')), COUNT_ERR);
});

test('25 words are refused', () => {
  assert.strictEqual(check([...ABANDON23, 'abandon', 'art'].join(' ')), COUNT_ERR);
});

test('empty and spaces-only input are refused', () => {
  assert.strictEqual(check(''), COUNT_ERR);
  assert.strictEqual(check('   \n\t  '), COUNT_ERR);
});

test('a valid 24-word phrase passes', () => {
  assert.strictEqual(check(VALID), VALID);
});

test('the test phrases: the valid one has a good checksum, the one-word change does not', () => {
  assert.ok(checksumOk(VALID));
  assert.ok(CHANGED.split(' ').every((w) => WORDS.includes(w)), 'changed phrase must use list words only');
  assert.ok(!checksumOk(CHANGED));
});

test('changing one word to another list word is refused (checksum)', { skip: 'bip39.js has no checksum check (neither does sheet() in app.js)' }, () => {
  assert.notStrictEqual(check(CHANGED), CHANGED);
});

test('a Cyrillic "а" inside a word is refused (full list)', () => {
  assert.strictEqual(full(['аbandon', ...ABANDON23].join(' ')), 'Word 1 is not on the list.');
});

test('a zero-width space inside a word is refused (full list)', () => {
  assert.strictEqual(full(['aban​don', ...ABANDON23].join(' ')), 'Word 1 is not on the list.');
});

test('a no-break space between words counts as a word gap (full list)', () => {
  const words = [...ABANDON23, 'art'];
  assert.strictEqual(full(words.join(' ')), words.join(' '));
});

test('the Kelvin sign "K" lower-cases to a plain k and is accepted (full list)', () => {
  const words = [...ABANDON23.slice(1), 'kite', 'art'];
  assert.strictEqual(full(words.join(' ').replace('kite', 'Kite')), words.join(' '));
});

test('a 10,000-character input is answered in under 200 ms', () => {
  const inputs = [
    'abandon '.repeat(1250),                                       // ~1250 words
    [...ABANDON23, 'x'.repeat(10000 - 23 * 8)].join(' '),         // 24 words, last one huge
    ' '.repeat(10000),                                             // nothing but spaces
    'a'.repeat(10000),                                             // one long word
  ];
  for (const s of inputs) {
    for (const list of [APP_LIST, FULL_LIST]) {
      const t0 = performance.now();
      check(s, list);
      const ms = performance.now() - t0;
      assert.ok(ms < 200, 'took ' + ms.toFixed(1) + ' ms for ' + s.length + ' chars');
    }
  }
  assert.strictEqual(full(inputs[1]), 'Word 24 is not on the list.');
});

/* =====================  qr.js: the QR maker  ===================== */

function render(m, px = 4, quiet = 4) {
  const w = (m.length + quiet * 2) * px, buf = new Uint8ClampedArray(w * w * 4).fill(255);
  m.forEach((row, y) => row.forEach((on, x) => {
    if (!on) return;
    for (let dy = 0; dy < px; dy++) for (let dx = 0; dx < px; dx++) {
      const i = (((y + quiet) * px + dy) * w + (x + quiet) * px + dx) * 4;
      buf[i] = buf[i + 1] = buf[i + 2] = 0;
    }
  }));
  return { buf, w };
}
function decode(m) { const { buf, w } = render(m); return jsQR(buf, w, w); }
function roundTrip(text) {
  const m = KVQR.matrix(text);
  assert.ok(m, 'matrix returned null');
  const res = decode(m);
  assert.ok(res, 'jsQR found no code (' + m.length + ' modules)');
  assert.strictEqual(res.data, text);
  return m.length;
}
const flat = (m) => m.map((r) => r.map((b) => (b ? 1 : 0)).join('')).join('\n');
function sample(n) { let s = ''; const a = 'kaspatest:qz0123456789abcdefghijklmnopqrstuvwxyz{}"[],.:-_ABCDEFGHIJKLMNOPQRSTUVWXYZ'; for (let i = 0; i < n; i++) s += a[(i * 7 + (i >> 3)) % a.length]; return s; }
const bytes = (s) => Buffer.byteLength(s, 'utf8');

function watchCodeLike(size) {
  const hex = (n, seed) => Array.from({ length: n }, (_, i) => '0123456789abcdef'[(i * 5 + seed) % 16]).join('');
  const o = {
    v: 1, kind: 'watch', net: 'testnet-10', name: 'Sam',
    covenant: hex(64, 1), script: hex(140, 2),
    addresses: { everyday: 'kaspatest:qz' + sample(61).replace(/[^a-z0-9]/g, 'q'), vault: 'kaspatest:pr' + sample(61).replace(/[^a-z0-9]/g, 'p') },
    keys: [hex(64, 3), hex(64, 4), hex(64, 5)], cap: '1500.00000000', delayDaa: 86400, heir: hex(64, 6), pad: '',
  };
  const fill = size - bytes(JSON.stringify(o));
  o.pad = hex(fill, 7);
  return JSON.stringify(o);
}

test('round-trips an empty string (0 bytes)', () => {
  assert.strictEqual(roundTrip(''), 21);
});

test('round-trips 1 byte', () => {
  assert.strictEqual(roundTrip('k'), 21);
});

test('round-trips 2953 bytes, the most version 40-L holds', () => {
  assert.strictEqual(roundTrip(sample(2953)), 177);
});

test('round-trips 230 and 231 bytes, where the length field grows to 16 bits (version 9 to 10)', () => {
  assert.strictEqual(roundTrip(sample(230)), 53);
  assert.strictEqual(roundTrip(sample(231)), 57);
});

test('round-trips a 1,535-byte watch-code-like JSON', () => {
  const code = watchCodeLike(1535);
  assert.strictEqual(bytes(code), 1535);
  assert.doesNotThrow(() => JSON.parse(code));
  roundTrip(code);
});

test('round-trips 2-, 3- and 4-byte UTF-8 characters', () => {
  roundTrip('Ωμέγα ñ é ü · 日本語 한국어 € · \u{1F512}\u{1F511}\u{10348} · é (combining)');
});

test('round-trips multibyte text right at the 2953-byte limit', () => {
  const text = '€'.repeat(984) + 'a'; // 984 x 3 bytes + 1 = 2953
  assert.strictEqual(bytes(text), 2953);
  assert.strictEqual(roundTrip(text), 177);
});

test('2954 bytes gives null and an empty svg, without throwing', () => {
  let m, s;
  assert.doesNotThrow(() => { m = KVQR.matrix(sample(2954)); s = KVQR.svg(sample(2954)); });
  assert.strictEqual(m, null);
  assert.strictEqual(s, '');
});

test('the limit counts bytes, not characters: 985 three-byte characters (2955 bytes) give null', () => {
  const text = '€'.repeat(985);
  assert.strictEqual(text.length, 985);
  assert.strictEqual(KVQR.matrix(text), null);
});

test('a 100,000-byte input is refused in under 200 ms', () => {
  const big = 'x'.repeat(100000);
  const t0 = performance.now();
  assert.strictEqual(KVQR.matrix(big), null);
  assert.ok(performance.now() - t0 < 200);
});

test('the same input gives the same matrix and svg every time', () => {
  for (const text of ['', 'k', watchCodeLike(1535), sample(2953)]) {
    assert.strictEqual(flat(KVQR.matrix(text)), flat(KVQR.matrix(text)));
    assert.strictEqual(KVQR.svg(text), KVQR.svg(text));
  }
});

test('a lone UTF-16 surrogate is sent as U+FFFD, so the text changes', () => {
  const res = decode(KVQR.matrix('ab\uD800cd'));
  assert.strictEqual(res.data, 'ab�cd');
});

test('null and undefined are refused, not drawn as a code', { todo: 'qr.js has no input check: null draws "null", undefined draws an empty code' }, () => {
  assert.strictEqual(KVQR.matrix(null), null);
  assert.strictEqual(KVQR.matrix(undefined), null);
  assert.strictEqual(KVQR.svg(null), '');
});
