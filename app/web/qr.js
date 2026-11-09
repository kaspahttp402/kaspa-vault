/* A small QR code maker: byte mode, low error correction (L), versions 1 to 40, up to 2953 bytes.
   Picks the smallest version that fits and the mask with the lowest standard penalty; no remote loads. Returns an SVG string. */
(function () {
  /* level L: error correction codewords per block, and number of blocks, for versions 1..40 (index 0 unused) */
  var EC = [0, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30,
    30, 30, 30, 30, 30, 30, 30, 30, 30, 30];
  var BLOCKS = [0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19,
    20, 21, 22, 24, 25];
  var EXP = new Array(512), LOG = new Array(256);
  (function () {
    var x = 1;
    for (var i = 0; i < 255; i++) { EXP[i] = x; LOG[x] = i; x <<= 1; if (x & 256) x ^= 0x11d; }
    for (i = 255; i < 512; i++) EXP[i] = EXP[i - 255];
  })();
  function mul(a, b) { return a && b ? EXP[LOG[a] + LOG[b]] : 0; }

  function ecBytes(data, n) {
    var gen = [1];
    for (var i = 0; i < n; i++) {
      var next = new Array(gen.length + 1).fill(0);
      for (var j = 0; j < gen.length; j++) { next[j] ^= gen[j]; next[j + 1] ^= mul(gen[j], EXP[i]); }
      gen = next;
    }
    var rem = new Array(n).fill(0);
    data.forEach(function (b) {
      var f = b ^ rem[0]; rem.shift(); rem.push(0);
      for (var k = 0; k < n; k++) rem[k] ^= mul(gen[k + 1], f);
    });
    return rem;
  }

  /* total codewords a version holds: all modules minus the function patterns, in whole bytes */
  function totalCodewords(v) {
    var r = (16 * v + 128) * v + 64;
    if (v >= 2) { var na = Math.floor(v / 7) + 2; r -= (25 * na - 10) * na - 55; if (v >= 7) r -= 36; }
    return Math.floor(r / 8);
  }
  function dataCodewords(v) { return totalCodewords(v) - EC[v] * BLOCKS[v]; }

  function alignPositions(v) {
    if (v === 1) return [];
    var na = Math.floor(v / 7) + 2, size = 17 + 4 * v;
    var step = v === 32 ? 26 : Math.ceil((v * 4 + 4) / (na * 2 - 2)) * 2, out = [6];
    for (var pos = size - 7; out.length < na; pos -= step) out.splice(1, 0, pos);
    return out;
  }

  /* data codewords (mode, count, bytes, terminator, padding) for the smallest version that fits, or null */
  function encode(bytes) {
    var ver = 0;
    for (var v = 1; v <= 40; v++) if (4 + (v < 10 ? 8 : 16) + bytes.length * 8 <= dataCodewords(v) * 8) { ver = v; break; }
    if (!ver) return null;
    var cap = dataCodewords(ver), bits = [];
    function put(val, len) { for (var i = len - 1; i >= 0; i--) bits.push((val >>> i) & 1); }
    put(4, 4); put(bytes.length, ver < 10 ? 8 : 16); bytes.forEach(function (b) { put(b, 8); });
    put(0, Math.min(4, cap * 8 - bits.length));
    while (bits.length % 8) bits.push(0);
    var data = [];
    for (var i = 0; i < bits.length; i += 8) data.push(parseInt(bits.slice(i, i + 8).join(''), 2));
    for (var pad = 0xec; data.length < cap; pad ^= 0xec ^ 0x11) data.push(pad);
    return { ver: ver, data: data };
  }

  /* split into blocks (short ones first), add EC to each, then interleave data and EC column by column */
  function interleave(ver, data) {
    var nb = BLOCKS[ver], ec = EC[ver], total = totalCodewords(ver);
    var shortLen = Math.floor(total / nb) - ec, nShort = nb - total % nb, blocks = [], eccs = [], k = 0, out = [], i, b;
    for (b = 0; b < nb; b++) {
      var len = shortLen + (b < nShort ? 0 : 1), blk = data.slice(k, k + len);
      k += len; blocks.push(blk); eccs.push(ecBytes(blk, ec));
    }
    for (i = 0; i <= shortLen; i++) for (b = 0; b < nb; b++) if (i < blocks[b].length) out.push(blocks[b][i]);
    for (i = 0; i < ec; i++) for (b = 0; b < nb; b++) out.push(eccs[b][i]);
    return out;
  }

  function grid(size) { var g = []; for (var y = 0; y < size; y++) g.push(new Array(size).fill(false)); return g; }

  /* finders, timing, alignment, version blocks and reserved format areas */
  function drawFunctions(ver) {
    var size = 17 + 4 * ver, m = grid(size), fn = grid(size), i;
    function set(x, y, on) { m[y][x] = on; fn[y][x] = true; }
    for (i = 0; i < size; i++) { set(6, i, i % 2 === 0); set(i, 6, i % 2 === 0); }
    [[3, 3], [size - 4, 3], [3, size - 4]].forEach(function (c) {
      for (var dy = -4; dy <= 4; dy++) for (var dx = -4; dx <= 4; dx++) {
        var d = Math.max(Math.abs(dx), Math.abs(dy)), x = c[0] + dx, y = c[1] + dy;
        if (x >= 0 && x < size && y >= 0 && y < size) set(x, y, d !== 2 && d !== 4);
      }
    });
    var ap = alignPositions(ver), last = ap.length - 1;
    ap.forEach(function (py, a) {
      ap.forEach(function (px, b) {
        if ((a === 0 && b === 0) || (a === 0 && b === last) || (a === last && b === 0)) return;
        for (var dy = -2; dy <= 2; dy++) for (var dx = -2; dx <= 2; dx++) set(px + dx, py + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
      });
    });
    if (ver >= 7) {
      var rem = ver;
      for (i = 0; i < 12; i++) rem = (rem << 1) ^ ((rem >>> 11) * 0x1f25);
      var vb = (ver << 12) | rem;
      for (i = 0; i < 18; i++) { var on = ((vb >>> i) & 1) === 1, a2 = size - 11 + i % 3, b2 = Math.floor(i / 3); set(a2, b2, on); set(b2, a2, on); }
    }
    drawFormat(m, fn, 0);
    return { m: m, fn: fn, size: size };
  }

  /* format: low correction (01) and the mask number, BCH-protected, both copies, plus the dark module */
  function drawFormat(m, fn, mask) {
    var size = m.length, fd = (1 << 3) | mask, rem = fd, i;
    for (i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
    var fb = ((fd << 10) | rem) ^ 0x5412;
    function set(x, y, n) { m[y][x] = ((fb >>> n) & 1) === 1; fn[y][x] = true; }
    for (i = 0; i <= 5; i++) set(8, i, i);
    set(8, 7, 6); set(8, 8, 7); set(7, 8, 8);
    for (i = 9; i < 15; i++) set(14 - i, 8, i);
    for (i = 0; i < 8; i++) set(size - 1 - i, 8, i);
    for (i = 8; i < 15; i++) set(8, size - 15 + i, i);
    m[size - 8][8] = true; fn[size - 8][8] = true;
  }

  /* zigzag the codewords into the free modules, unmasked; remainder bits stay light */
  function placeData(m, fn, all) {
    var size = m.length, n = 0, total = all.length * 8;
    for (var right = size - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (var vert = 0; vert < size; vert++) for (var j = 0; j < 2; j++) {
        var x = right - j, up = ((right + 1) & 2) === 0, y = up ? size - 1 - vert : vert;
        if (fn[y][x]) continue;
        m[y][x] = n < total && ((all[n >>> 3] >>> (7 - (n & 7))) & 1) === 1;
        n++;
      }
    }
  }

  var MASKS = [
    function (x, y) { return (x + y) % 2 === 0; },
    function (x, y) { return y % 2 === 0; },
    function (x) { return x % 3 === 0; },
    function (x, y) { return (x + y) % 3 === 0; },
    function (x, y) { return (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0; },
    function (x, y) { return x * y % 2 + x * y % 3 === 0; },
    function (x, y) { return (x * y % 2 + x * y % 3) % 2 === 0; },
    function (x, y) { return ((x + y) % 2 + x * y % 3) % 2 === 0; }
  ];
  function masked(base, fn, k) {
    var f = MASKS[k], m = base.map(function (row, y) { return row.map(function (on, x) { return fn[y][x] ? on : on !== f(x, y); }); });
    drawFormat(m, fn, k);
    return m;
  }

  /* standard penalty rules 1 (runs) and 3 (finder-like 1:1:3:1:1 with 4 light) along one line */
  function lineScore(line) {
    var s = 0, run = 1, i, str = line.map(function (b) { return b ? '1' : '0'; }).join('');
    for (i = 1; i <= line.length; i++) {
      if (i < line.length && line[i] === line[i - 1]) { run++; continue; }
      if (run >= 5) s += run - 2;
      run = 1;
    }
    [/(?=10111010000)/g, /(?=00001011101)/g].forEach(function (re) { s += 40 * (str.match(re) || []).length; });
    return s;
  }
  function penalty(m) {
    var size = m.length, s = 0, dark = 0, x, y;
    for (y = 0; y < size; y++) {
      s += lineScore(m[y]);
      s += lineScore(m.map(function (row) { return row[y]; }));
      for (x = 0; x < size; x++) {
        if (m[y][x]) dark++;
        if (x && y && m[y][x] === m[y - 1][x] && m[y][x] === m[y][x - 1] && m[y][x] === m[y - 1][x - 1]) s += 3;
      }
    }
    return s + 10 * Math.max(0, Math.ceil(Math.abs(dark * 20 - size * size * 10) / (size * size)) - 1);
  }

  function matrix(text) {
    var enc = encode(Array.from(new TextEncoder().encode(text)));
    if (!enc) return null;
    var f = drawFunctions(enc.ver), best = null, bestScore = Infinity;
    placeData(f.m, f.fn, interleave(enc.ver, enc.data));
    for (var k = 0; k < 8; k++) {
      var c = masked(f.m, f.fn, k), sc = penalty(c);
      if (sc < bestScore) { best = c; bestScore = sc; }
    }
    return best;
  }

  window.KVQR = {
    matrix: matrix,
    svg: function (text) {
      var m = matrix(text); if (!m) return '';
      var d = '', q = 2;
      m.forEach(function (row, y) { row.forEach(function (on, x) { if (on) d += 'M' + (x + q) + ' ' + (y + q) + 'h1v1h-1z'; }); });
      var s = m.length + q * 2;
      return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ' + s + ' ' + s + '" shape-rendering="crispEdges" role="img" aria-label="Your account address as a square code"><path fill="#10163d" d="' + d + '"/></svg>';
    }
  };
})();
