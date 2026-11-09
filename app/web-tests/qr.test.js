/* Round-trip check for the QR maker: encode with app/web/qr.js, render to pixels, decode with app/web/jsqr.js.
   Run from the repo root: node --test app/web-tests/qr.test.js */
'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const WEB = path.join(__dirname, '..', 'web');
const ctx = { TextEncoder, TextDecoder, console, Uint8ClampedArray, Int32Array, Uint8Array, Float32Array, Math, Array, Object };
ctx.window = ctx; ctx.self = ctx;
vm.createContext(ctx);
vm.runInContext(fs.readFileSync(path.join(WEB, 'jsqr.js'), 'utf8'), ctx, { filename: 'jsqr.js' });
vm.runInContext(fs.readFileSync(path.join(WEB, 'qr.js'), 'utf8'), ctx, { filename: 'qr.js' });
const { KVQR, jsQR } = ctx;

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

function sample(n) { let s = ''; const a = 'kaspa:qz0123456789abcdefghijklmnopqrstuvwxyz{}"[],.:-_ABCDEFGHIJKLMNOPQRSTUVWXYZ'; for (let i = 0; i < n; i++) s += a[(i * 7 + (i >> 3)) % a.length]; return s; }

function roundTrip(text) {
  const m = KVQR.matrix(text);
  assert.ok(m, 'matrix returned null');
  const { buf, w } = render(m);
  const res = jsQR(buf, w, w);
  assert.ok(res, 'jsQR found no code (' + m.length + ' modules)');
  assert.strictEqual(res.data, text);
  return m.length;
}

for (const n of [1, 17, 106, 107, 250, 500, 1000, 1500, 2000, 2953]) {
  test('round-trips ' + n + ' bytes', () => {
    const size = roundTrip(sample(n));
    assert.ok(size >= 21 && size <= 177);
  });
}

test('round-trips UTF-8 text with non-ASCII characters', () => {
  roundTrip('Vault déjà vu: naïve café, Ωμέγα, 日本語, emoji \u{1F512} kaspa:qq');
});

test('version 1 and version 40 sizes', () => {
  assert.strictEqual(KVQR.matrix('a').length, 21);
  assert.strictEqual(KVQR.matrix(sample(2953)).length, 177);
});

test('2954 bytes is too long', () => {
  assert.strictEqual(KVQR.matrix(sample(2954)), null);
  assert.strictEqual(KVQR.svg(sample(2954)), '');
  assert.ok(KVQR.svg('hello').startsWith('<svg'));
});
