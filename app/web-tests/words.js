/* Owner-banned words on screen. Run from the repo root: node app/web-tests/words.js
   Reads every string literal a person can read in app.js, data.js and demo.js and fails when one
   holds a word the owner
   has banned, so later edits cannot bring old names back. Not checked: comments, regex literals,
   object keys, lone
   identifier or selector strings, class/selector arguments, '/api/' paths, HTML tags and attributes
   (except the readable
   placeholder=, aria-label= and title= values) and the glossary's tech: lines (technical by
   design).
   Strings joined with + are checked as one text, so a tag split across pieces is still stripped
   right.
   Exit 1 on any FAIL. WARN lines are hints for a person to judge and never fail the run. */
'use strict';
const fs = require('fs');
const path = require('path');

const WEB = path.join(__dirname, '..', 'web');
const FILES = ['app.js', 'data.js', 'demo.js'];
const BANNED = ['sheet', 'sheets', 'wallet', 'laptop', 'drawer', 'silence', 'silent', 'burn',
  'burned', 'DAA', 'sompi',
  'utxo', 'crypto', 'blockchain', 'pairing', 'keys.json', 'state.json'];
/* Exact literal values (after escapes are read) that may hold a banned word on purpose. Each entry
   needs a comment
   saying why. Prefer fixing the text; an entry here is a promise that the word is meant. */
const ALLOW = [
  'Kaspa, the digital money this app holds. Amounts show in KAS; the network counts in sompi.', // glossary defines this term
  'Wallet', // glossary defines this term
  /* glossary defines this term */
  'A wallet is software that keeps your keys, watches the network for your coins, builds payments and signs them. It does not hold money; the network does.',
  'DAA score', // glossary defines this term
  /* glossary defines this term */
  "DAA score is Kaspa's clock. It counts blocks, about ten a second, so a wait of one day is written as a number of blocks rather than hours.",
  'UTXO model', // glossary defines this term
  'Sompi', // glossary defines this term
  'A sompi is the smallest piece of Kaspa, like a cent to a dollar but much smaller. One KAS is one hundred million sompi.', // glossary defines this term
];
/* the two key phrases the owner kept */
const KEY_OK = [/\bmake my keys\b/gi, /\breplace your keys\b/gi];
/* "safe" as a place, the old name for the vault */
const SAFE_NOUN = /\b(?:the|your) safe\b|\bsafe['’]s\b/i;
const REGEX_AFTER_WORD = new Set(['return', 'typeof', 'instanceof', 'in', 'of', 'new', 'delete',
  'void', 'throw', 'case',
  'do', 'else', 'yield', 'await']);
const SELECTOR_FN = new Set(['querySelector', 'querySelectorAll', 'getElementById',
  'getElementsByClassName', 'closest',
  'matches', 'add', 'remove', 'toggle', 'contains', 'getAttribute', 'hasAttribute',
    'removeAttribute', 'replace']);
const ENT = { nbsp: ' ', rsquo: "'", lsquo: "'", apos: "'", amp: '&', quot: '"', lt: '<', gt: '>',
  '#39': "'", '#8217': "'" };
const KEEP = /\b(?:placeholder|aria-label|title)\s*=\s*(?:"([^"]*)"|'([^']*)')/gi;

/* the end of a string literal whose quote c opened before j; a template's ${...} is skipped by
   skipExpr */
function stringEnd(src, j, c, skipExpr) {
  const n = src.length;
  let expr = false;
  while (j < n && src[j] !== c) {
    if (src[j] === '\\') { j += 2; continue; }
    if (c === '`' && src[j] === '$' && src[j + 1] === '{') {
      expr = true; j = skipExpr(j + 2); continue;
    }
    if (src[j] === '\n' && c !== '`') break; // unterminated: stop at the line end
    j++;
  }
  return { j, expr };
}
/* the end of a regex literal whose slash opened before j, past its flags */
function regexEnd(src, j) {
  const n = src.length;
  let cls = false;
  while (j < n && src[j] !== '\n') {
    const ch = src[j];
    if (ch === '\\') { j += 2; continue; }
    if (ch === '[') cls = true; else if (ch === ']') cls = false; else if (ch === '/' &&
      !cls) break;
    j++;
  }
  j++;
  while (j < n && /[a-z]/i.test(src[j])) j++;
  return j;
}
/* Tokens: s (string literal), w (word), p (one punctuation char), re (regex literal). Comments are
   dropped. */
function lex(src) {
  const toks = [], n = src.length;
  let i = 0, line = 1;
  const lines = (a, b) => { for (let k = a; k < b; k++) if (src.charCodeAt(k) === 10) line++; };
  const regexOk = t => !t || (t.type === 'p' && '(,=:[!&|?{};+-*%<>~^'.includes(t.v)) ||
    (t.type === 'w' && REGEX_AFTER_WORD.has(t.v));
  function skipQuoted(j) { const q = src[j]; j++; while (j < n &&
    src[j] !== q) j += src[j] === '\\' ? 2 : 1; return j + 1; }
  function skipExpr(j) {
    let depth = 1;
    while (j < n && depth > 0) {
      const ch = src[j];
      if (ch === '"' || ch === "'" || ch === '`') { j = skipQuoted(j); continue; }
      if (ch === '{') depth++; else if (ch === '}') depth--;
      j++;
    }
    return j;
  }
  while (i < n) {
    const c = src[i], d = src[i + 1];
    if (c === '\n') { line++; i++; continue; }
    if (/\s/.test(c)) { i++; continue; }
    if (c === '/' && d === '/') { while (i < n && src[i] !== '\n') i++; continue; }
    if (c === '/' && d === '*') { const e = src.indexOf('*/', i + 2), stop = e < 0 ? n : e +
      2; lines(i, stop); i = stop; continue; }
    if (c === '"' || c === "'" || c === '`') {
      const start = i, at = line;
      const { j, expr } = stringEnd(src, i + 1, c, skipExpr);
      lines(start, j);
      toks.push({ type: 's', raw: src.slice(start + 1, j), line: at, expr });
      i = j + 1;
      continue;
    }
    if (c === '/' && regexOk(toks[toks.length - 1])) {
      const j = regexEnd(src, i + 1);
      toks.push({ type: 're', line });
      i = j;
      continue;
    }
    if (/[\w$]/.test(c)) {
      let j = i;
      while (j < n && /[\w$]/.test(src[j])) j++;
      toks.push({ type: 'w', v: src.slice(i, j), line });
      i = j;
      continue;
    }
    toks.push({ type: 'p', v: c, line });
    i++;
  }
  return toks;
}

/* Two string tokens belong to one text when only "+ expression +" sits between them (no , ; : ? at
   the top level). */
function joinable(toks, a, b) {
  if (b === a + 1 || toks[a + 1].v !== '+' || toks[b - 1].v !== '+') return false;
  let depth = 0;
  for (let k = a + 1; k < b; k++) {
    const t = toks[k];
    if (t.type !== 'p') continue;
    if ('([{'.includes(t.v)) depth++;
    else if (')]}'.includes(t.v)) { if (--depth < 0) return false; }
    else if (depth === 0 && ',;:?'.includes(t.v)) return false;
  }
  return depth === 0;
}

function chains(toks) {
  const out = [];
  let cur = null, last = -1;
  toks.forEach((t, k) => {
    if (t.type !== 's') return;
    if (cur && joinable(toks, last, k)) cur.push(k); else { cur = [k]; out.push(cur); }
    last = k;
  });
  return out;
}

/* The object key whose value holds token k ('' when none), e.g. 'tech' for  tech: 'a' + 'b'. */
function fieldOf(toks, k) {
  let depth = 0;
  for (let j = k - 1, stop = Math.max(0, k - 600); j >= stop; j--) {
    const t = toks[j];
    if (t.type !== 'p') continue;
    if (')]'.includes(t.v)) depth++;
    else if ('(['.includes(t.v)) { if (depth-- === 0) return ''; }
    else if (depth === 0 && ',;{}'.includes(t.v)) return '';
    else if (depth === 0 && t.v === ':') {
      const key = toks[j - 1], before = toks[j - 2];
      if (key && before && (key.type === 'w' || key.type === 's') && before.type === 'p' &&
        '{,'.includes(before.v)) {
        return key.type === 'w' ? key.v : key.raw;
      }
    }
  }
  return '';
}

function isKey(toks, k) {
  const next = toks[k + 1], prev = toks[k - 1];
  if (prev && prev.type === 'w' && prev.v === 'case') return true;
  return !!(next && next.v === ':' && prev && prev.type === 'p' && '{,'.includes(prev.v));
}

/* first argument of a selector/class call, or the value given to className */
function codeArg(toks, k) {
  const p1 = toks[k - 1], p2 = toks[k - 2], p3 = toks[k - 3];
  if (p1 && p1.v === '(' && p2 && p2.type === 'w' && SELECTOR_FN.has(p2.v)) return true;
  if (p1 && p1.v === '=' && p2 && (p2.v === 'className' || (p2.v === '+' && p3 &&
    p3.v === 'className'))) return true;
  return false;
}

function codeLike(s) {
  /* identifier, key or selector, no space */
  if (/^[a-z0-9_.\-[\]#="':>*()~^$|]+$/.test(s)) return true;
  return /^[.#[]/.test(s) && /^[a-z0-9_\s.#[\]="'\-:>*,()~+^$|]+$/.test(s); // a selector list
}

function decode(raw) {
  return raw.replace(/\\(u\{[0-9a-fA-F]+\}|u[0-9a-fA-F]{4}|x[0-9a-fA-F]{2}|\r?\n|[\s\S])/g, (m,
    e) => {
    if (e[0] === 'u' && e.length > 1) return String.fromCodePoint(parseInt(e.replace(/[u{}]/g, ''),
      16));
    if (e[0] === 'x' && e.length === 3) return String.fromCharCode(parseInt(e.slice(1), 16));
    if (e === 'n' || e === 't' || e === 'r') return ' ';
    return /^\r?\n$/.test(e) ? '' : e;
  });
}

function kept(tag) {
  const v = [];
  let m;
  KEEP.lastIndex = 0;
  while ((m = KEEP.exec(tag))) v.push(m[1] != null ? m[1] : m[2]);
  return v.join(' ');
}

/* What a person reads: text between tags plus placeholder / aria-label / title values. */
function visible(s) {
  let out = s;
  const gt = out.indexOf('>'), lt = out.indexOf('<');
  if (gt >= 0 && (lt < 0 || gt < lt)) {
    const head = out.slice(0, gt);   // the tail of a tag opened before this text began
    if (/^\s*\/?$|["'=]|^\s+[\w-]+(\s+[\w-]+)*\s*\/?$/.test(head)) out = kept(head) + ' ' +
      out.slice(gt + 1);
  }
  out = out.replace(/<[a-zA-Z/!][^>]*>/g, m => ' ' + kept(m) + ' ').replace(/<[a-zA-Z/!][^>]*$/,
    m => ' ' + kept(m) + ' ');
  return out.replace(/&(#?\w+);/g, (m, e) => (ENT[e] != null ? ENT[e] : ' ')).replace(/\s+/g,
    ' ').trim();
}

const wordRe = w => new RegExp('(?<![\\w])' + w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') +
  '(?![\\w])', 'i');
const BANNED_RE = BANNED.map(w => [w, wordRe(w)]);
function excerpt(t, at) {
  const a = Math.max(0, at - 40), b = Math.min(t.length, at + 50);
  return (a ? '...' : '') + t.slice(a, b) + (b < t.length ? '...' : '');
}

const fails = [], warns = [];
let checked = 0;
for (const f of FILES) {
  const toks = lex(fs.readFileSync(path.join(WEB, f), 'utf8'));
  for (const ch of chains(toks)) {
    const pieces = ch.map(k => toks[k]), k0 = ch[0];
    if (pieces.some(p => p.expr)) continue; // template literal with ${}: not checked
    const vals = pieces.map(p => decode(p.raw));
    if (pieces.length === 1 && (codeLike(vals[0]) || isKey(toks, k0) || codeArg(toks,
      k0))) continue;
    if (vals[0].startsWith('/api/')) continue;
    const field = fieldOf(toks, k0);
    if (field === 'tech') continue;
    const text = visible(vals.join(' '));
    if (!text) continue;
    checked += pieces.length;
    if (ALLOW.includes(text) || vals.some(v => ALLOW.includes(v))) continue;
    const lineOf = re => (pieces.find((p, i) => re.test(vals[i])) || pieces[0]).line;
    for (const [w, re] of BANNED_RE) {
      const m = re.exec(text);
      if (m) fails.push(`FAIL ${f}:${lineOf(re)}: ${w} in: ${excerpt(text, m.index)}`);
    }
    const safe = SAFE_NOUN.exec(text);
    if (safe) {
      const at = excerpt(text, safe.index);
      warns.push(`WARN ${f}:${lineOf(SAFE_NOUN)}: "safe" as a noun in: ${at}`);
    }
    if (field !== 'term' && field !== 'plain') {
      const KEY = /\bkeys?\b(?!\.json)/i, rest = KEY_OK.reduce((t, r) => t.replace(r, ' '), text),
        key = KEY.exec(rest);
      if (key) warns.push(`WARN ${f}:${lineOf(KEY)}: ${key[0]} in: ${excerpt(rest, key.index)}`);
    }
  }
}

fails.concat(warns).forEach(l => console.log(l));
if (fails.length) {
  console.log(`words: ${fails.length} banned-word hits, ${warns.length} warnings, ` +
    `${checked} strings checked`);
  process.exit(1);
}
console.log(`ok   words: ${checked} strings checked` +
  (warns.length ? ` (${warns.length} warnings above)` : ''));
