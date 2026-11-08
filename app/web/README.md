# The KaspaVault page

The whole app the user sees is this folder: one HTML page loaded by the Android shell in `app/android`.
It runs on the phone only. There is no server and no laptop; the engine is a process on the same phone.

## Files

- `index.html`: the shell of the page, the CSP, and the script tags in load order.
- `style.css`: all the styling. There are no inline styles anywhere.
- `app.js`: the screens, the help, the glossary and the walk-through. Everything the user reads is here.
- `data.js`: the state (`KVData`), the calls to the engine, the settings, the price history and Sample mode.
- `scene.js`: the moving art behind the screens (one ribbon). Nothing else depends on it; it can be swapped.
- `qr.js`: a small QR code maker that returns an SVG string. No remote loads.
- `jsqr.js`, `bip39.js`: vendored, unchanged. The QR reader and the word list for checking seed words.
- `fonts/`: the one font (Outfit), served from the page itself.

## The two network paths

1. The engine. `data.js` turns a method and path (`POST /api/send` and so on) into a named engine method and
   hands it to Java with `bridge.vault(...)`. Java owns the engine process, checks the method against its own
   allow-list, runs it and answers the page through a callback. The page never opens a socket.
2. Prices. The chart and the ribbon use the KAS price from CoinGecko. The page builds the URL and asks Java to
   fetch it; Java accepts only that one URL prefix (`Bridge.java`) and the CSP allows only that host.

Nothing else leaves the phone. In a desktop browser (Sample mode) there is no bridge and path 1 is never used.

## CSP and the no-inline rule

`index.html` sets `default-src 'none'; script-src 'self'; style-src 'self'; ...` with no `unsafe-inline`.
So: no `onclick="..."` in markup, no `<script>` bodies, no `style="..."` attributes. Behaviour is wired in
`after(el)` with `el.querySelector(...).onclick = ...`; looks are classes in `style.css`. A CSS variable that
must change at run time is set with `el.style.setProperty`, which the CSP allows.

## What the page must never log

Nothing from the engine's answers: no balances, addresses, sheets, seed words or error bodies in `console.log`.
Java logs the method names only, never the bodies. Keep it that way; a logcat line is a screenshot that lasts.

## How screens are built

`SCR` in `app.js` is a table of screens. Each entry has some of:

- `html(params)`: returns the screen's markup as a string.
- `after(el, params)`: wires buttons and inputs on the new element.
- `live(el, params)`: subscribes the element to state changes (balances, countdowns) while it is shown.
- `scene()`: what the ribbon should do behind this screen (mode, snap).
- `money: true`: the screen shows words or moves money. Java gets the screen name (`bridge.screen`) and sets
  FLAG_SECURE from it, so these screens never appear in screenshots or the recents view.
- `tab`: the screen is one of the bottom tabs.

`go(name, params)` pushes a screen on the stack; `show` builds it from the table and runs `after` then `live`.
Small repeats have one helper each near the top of `app.js`: `isKaspaAddress`, `readPurse`, `wireSeg`.

## How money moves

Every call that moves money goes through `moneyAction(why, path, body)` in `data.js`:

1. `signerUnlock(why)` asks Java for a fingerprint, with `why` as the prompt text the user reads.
2. On yes, `liveAction` sends the call to the engine and polls the state once it is done.
3. Java takes the key for that one move itself; no key bytes ever reach this page.

If the engine answers that the move needs a fresh fingerprint (irreversible moves do, even inside the signer
window), `moneyAction` asks once more and repeats the call once. A second refusal is returned as it is.
Every error text passes `plain()`, which swaps anything in bank-unfriendly words for a plain fallback.

## Settings

`D.getSetting(key)` and `D.setSetting(key, value)` keep page settings in Java's preferences. Java answers only
for keys in the `SETTINGS` set in `MainActivity.java`: a new page setting must be added to that list first, or
it silently reads as empty. In a desktop browser the same keys live in `localStorage` under `kv_`.

## Sample mode

With no bridge (a desktop browser) the page runs on a seeded, made-up account so the screens can be read and
tuned. It is a demo only: no engine, no fingerprint, no network except prices. A phone with the engine sets
`live_local` on first start and runs Live from then on; `D.isLocal()` is that path and every real behaviour
lives behind it. Sample on a phone exists only for browsing a build with no engine, and says so on every screen.
