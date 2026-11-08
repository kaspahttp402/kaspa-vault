/* A small QR code maker: byte mode, low error correction, versions 1 to 5 (one block each), up to 106 bytes.
   Enough for an account address; no remote loads. Returns an SVG string. */
(function () {
  var DATA = [0, 19, 34, 55, 80, 108], EC = [0, 7, 10, 15, 20, 26], ALIGN = [0, 0, 18, 22, 26, 30];
  var EXP = new Array(512), LOG = new Array(256);
  (function () { var x = 1; for (var i = 0; i < 255; i++) { EXP[i] = x; LOG[x] = i; x <<= 1; if (x & 256) x ^= 0x11d; } for (i = 255; i < 512; i++) EXP[i] = EXP[i - 255]; })();
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

  function matrix(text) {
    var bytes = Array.from(new TextEncoder().encode(text)), ver = 0;
    for (var v = 1; v <= 5; v++) if (bytes.length <= DATA[v] - 2) { ver = v; break; }
    if (!ver) return null;
    var cap = DATA[ver], bits = [];
    function put(val, len) { for (var i = len - 1; i >= 0; i--) bits.push((val >>> i) & 1); }
    put(4, 4); put(bytes.length, 8); bytes.forEach(function (b) { put(b, 8); });
    put(0, Math.min(4, cap * 8 - bits.length));
    while (bits.length % 8) bits.push(0);
    var data = [];
    for (var i = 0; i < bits.length; i += 8) data.push(parseInt(bits.slice(i, i + 8).join(''), 2));
    for (var pad = 0xec; data.length < cap; pad ^= 0xec ^ 0x11) data.push(pad);
    var all = data.concat(ecBytes(data, EC[ver]));

    var size = 17 + 4 * ver, m = [], fn = [];
    for (var y = 0; y < size; y++) { m.push(new Array(size).fill(false)); fn.push(new Array(size).fill(false)); }
    function set(x, y, on) { m[y][x] = on; fn[y][x] = true; }

    for (i = 0; i < size; i++) { set(6, i, i % 2 === 0); set(i, 6, i % 2 === 0); }
    [[3, 3], [size - 4, 3], [3, size - 4]].forEach(function (c) {
      for (var dy = -4; dy <= 4; dy++) for (var dx = -4; dx <= 4; dx++) {
        var d = Math.max(Math.abs(dx), Math.abs(dy)), x = c[0] + dx, y = c[1] + dy;
        if (x >= 0 && x < size && y >= 0 && y < size) set(x, y, d !== 2 && d !== 4);
      }
    });
    if (ver > 1) {
      var p = ALIGN[ver];
      for (var dy = -2; dy <= 2; dy++) for (var dx = -2; dx <= 2; dx++) set(p + dx, p + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    }
    /* format: low correction (01), mask 0 */
    var fd = (1 << 3) | 0, rem = fd;
    for (i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
    var fb = ((fd << 10) | rem) ^ 0x5412;
    function bit(n) { return ((fb >>> n) & 1) === 1; }
    for (i = 0; i <= 5; i++) set(8, i, bit(i));
    set(8, 7, bit(6)); set(8, 8, bit(7)); set(7, 8, bit(8));
    for (i = 9; i < 15; i++) set(14 - i, 8, bit(i));
    for (i = 0; i < 8; i++) set(size - 1 - i, 8, bit(i));
    for (i = 8; i < 15; i++) set(8, size - 15 + i, bit(i));
    set(8, size - 8, true);

    var n = 0, total = all.length * 8;
    for (var right = size - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (var vert = 0; vert < size; vert++) for (var j = 0; j < 2; j++) {
        var x = right - j, up = ((right + 1) & 2) === 0, yy = up ? size - 1 - vert : vert;
        if (fn[yy][x]) continue;
        var on = false;
        if (n < total) { on = ((all[n >>> 3] >>> (7 - (n & 7))) & 1) === 1; n++; }
        m[yy][x] = (x + yy) % 2 === 0 ? !on : on;
      }
    }
    return m;
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
