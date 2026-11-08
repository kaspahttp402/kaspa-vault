/* House rules for the page, checked on every round. Run from the repo root: node app/web-tests/pagelint.js
   Exit 1 when a rule breaks. Limits are dials at the top; raise one only with a reason written beside it. */
'use strict';
const fs = require('fs');
const path = require('path');

const WEB = path.join(__dirname, '..', 'web');
const FILES = ['app.js', 'data.js', 'scene.js'];
const LIMIT = {
  depth: 8,        // deepest brace nesting inside any one file (screens sit inside the SCR table: 2 levels are the table itself)
  fnLines: 80,     // longest function body; the scene's draw loop and the two module wrappers are exempt (see EXEMPT)
  codeLine: 160,   // a line of logic; lines that are only a string literal (help text, HTML) are not counted
};
const EXEMPT = { 'scene.js': 2, 'data.js': 1, 'app.js': 1 }; // the N largest functions per file are module wrappers or the draw loop
const BANNED = [
  [/\beval\s*\(|new Function\s*\(/, 'no eval or new Function (the CSP forbids them)'],
  [/\sstyle\s*=\s*\\?["']/, 'no inline style attributes (the CSP forbids them; use a class)'],
  [/<script/i, 'no script tags built in strings'],
  [/console\.log\s*\(/, 'no console.log in app.js or data.js (an engine answer must never reach the log)', f => f !== 'scene.js'],
  [/document\.write\s*\(/, 'no document.write'],
];

const STRIP = /'(?:\\.|[^'\\\n])*'|"(?:\\.|[^"\\\n])*"|\/\*[\s\S]*?\*\/|\/\/[^\n]*/g;
let failures = 0;
function fail(msg) { failures++; console.log('  ✖ ' + msg); }

for (const f of FILES) {
  const src = fs.readFileSync(path.join(WEB, f), 'utf8');
  const code = src.replace(STRIP, m => m.replace(/[^\n]/g, ' '));
  const srcLines = src.split('\n'), codeLines = code.split('\n');
  console.log(f + ': ' + srcLines.length + ' lines');

  let depth = 0, maxDepth = 0, maxAt = 0;
  codeLines.forEach((l, i) => { for (const ch of l) { if (ch === '{') { depth++; if (depth > maxDepth) { maxDepth = depth; maxAt = i + 1; } } else if (ch === '}') depth--; } });
  if (maxDepth > LIMIT.depth) fail(`${f}:${maxAt} nesting ${maxDepth} deep (limit ${LIMIT.depth})`);

  const sizes = [];
  const re = /function\s*[\w$]*\s*\([^)]*\)\s*\{/g; let m;
  while ((m = re.exec(code))) {
    let d = 0;
    for (let k = m.index + m[0].length - 1; k < code.length; k++) {
      if (code[k] === '{') d++;
      else if (code[k] === '}' && --d === 0) { sizes.push({ lines: code.slice(m.index, k).split('\n').length, at: code.slice(0, m.index).split('\n').length }); break; }
    }
  }
  sizes.sort((a, b) => b.lines - a.lines).slice(EXEMPT[f] || 0).filter(s => s.lines > LIMIT.fnLines)
    .forEach(s => fail(`${f}:${s.at} function of ${s.lines} lines (limit ${LIMIT.fnLines})`));

  codeLines.forEach((l, i) => { if (l.trimEnd().length > LIMIT.codeLine && l.replace(/\s/g, '').length > 40) fail(`${f}:${i + 1} a line of logic ${l.trimEnd().length} long (limit ${LIMIT.codeLine})`); });

  srcLines.forEach((l, i) => { for (const [rx, why, only] of BANNED) if ((!only || only(f)) && rx.test(l)) fail(`${f}:${i + 1} ${why}`); });
}
console.log(failures ? failures + ' rule(s) broken' : 'all house rules hold');
process.exit(failures ? 1 : 0);
