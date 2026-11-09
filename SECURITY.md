# Security

This page states what KaspaVault protects, against whom, how each protection was checked, and what is still open.
"Proven" means an engine test or a testnet run recorded in [docs/TESTING.md](docs/TESTING.md). "Mitigated" means a design or
code measure without a full proof. "Open" means known and not solved.

Terms: this page keeps the engine's words for the covenant. The *safe* is the 2-of-3 multisig address the app calls
the vault, opened by any two of the three vault seeds; each *stop key* sits behind one of the app's emergency stop
buttons.

## Reporting a vulnerability

Report security issues privately through GitHub private vulnerability reporting: open this repository's
**Security** tab and choose **Report a vulnerability**. Include the affected file or entry, the steps or transaction
shape, and what an attacker gains. Please do not open a public issue for a vulnerability. There is no email address
for reports and no bounty; reports are credited if you wish.

## Assets

| Asset | Where it lives | Loss if taken |
|---|---|---|
| Money in a protected account | On chain, under the covenant | Up to the remaining limit at once; anything more only after the notice delay, and only if nobody declines or stops it |
| Money in the safe | On chain, a 2-of-3 multisig address | Everything in the safe, but only with two of the three vault seeds |
| The card (spend key) | Sealed on the phone | Spending within the limit; requesting larger payments, which are visible and stoppable |
| Stop key 1 | Sealed on the phone | Freezing the account into the owner's safe, or declining requests; never taking money |
| Stop keys 2 and 3 | Each with a guardian (a trusted person, sealed in KaspaVault on their own phone), or a key nobody holds | As stop key 1 |
| The purse key | Sealed on the phone | The purse balance (no rules) |
| Vault seeds | Paper, three places; sealed on the phone until deleted there | Any two open the safe |
| Heir key | Sealed in KaspaVault on the heir's own phone; never on the owner's phone | The account, but only after the full inactivity period |

## Attackers and outcomes

| Attacker | What they get | What stops them | Status |
|---|---|---|---|
| Thief with the unlocked phone | Payments up to the remaining limit | The limit; larger payments wait out the notice delay and raise an alarm; a stop key elsewhere can decline or stop | Proven (limit, delay, decline, stop on testnet) |
| Thief with the locked phone | Nothing while the lock holds | Keys sealed in the Android Keystore behind a strong biometric or the device credential; screenshots blocked on money and word screens; no backup | Mitigated |
| Someone relaying or watching a payment | Nothing | SIGHASH_ALL is required by the covenant, so outputs and arguments cannot be altered | Proven |
| Someone sending coins to the vault address | Nothing | Coins without lineage can leave only by stop or inheritance; the wallet follows the covenant id | Proven (contract); wallet side follows lineage |
| Someone trying to make money unmovable | Nothing lasting | Every continuation and every payout to the safe, the heir or a requested destination is at least `minRemainder`; the stop works from any state and may bring outside coins for the fee | Proven |
| A stop-key holder who turns hostile | Freezing the account into the owner's safe | The stop pays only the fixed safe address | Proven |
| An impatient heir | Nothing before the inactivity period ends | Relative timelock on coin age; any owner activity restarts it; a waiting payment is honoured first | Proven (early inherit refused by the node; inherit from Pending on testnet) |
| A stranger submitting keyless entries | The right to pay the fee | `finish` pays only the recorded destination; `inheritOpen` has a pinned output shape and pays only the heir | Proven |
| A lying or stalled Kaspa node | A delayed or missing alarm | The guard asks two independent nodes and warns when they disagree or go quiet | Mitigated |
| A bug in the rules | Unknown | Engine proofs for each entry, mutation testing of the `require` lines, adversarial reviews by separate AI model sessions (no human audit), testnet runs of the time rules listed in TESTING.md | Mitigated; see known limits |

## Review and hardening

The code was hardened in repeated rounds. In each round a reviewer listed the ways into the piece and attacked it, a
builder removed or locked what was not needed, and a separate prover re-ran the suites and the specific attacks. The
reviewers were separate AI model sessions given the code without the authors' reasons; they were not independent
human auditors, and no third-party audit has been done. Outcomes, by piece:

**Covenant.**
- First adversarial review: no way to take more than the rules allow and no permanent freeze, but three gaps, all
  fixed and proven: signature hash type not pinned (a SIGHASH_NONE signature could have let a relayer redirect a
  payment; now SIGHASH_ALL is required), a remainder that could be too small to mine (the `minRemainder` floor), and
  outside coins leaving through the keyless finish (lineage check on the input).
- Entry-by-entry review: no high findings; two medium and several low findings fixed, among them inheritance from a
  Pending coin now pays the owner's committed request first, requests below the floor or to malformed destinations are
  refused, every payout to the safe, the heir or a requested destination is at least `minRemainder`, and a finished request no longer leaves its destination in
  the account's state.
- Second adversarial review: no path lets anyone but the named keys take money, and every bad state still has an
  exit. One low finding fixed: a single-output finish now ends the covenant lineage.
- Review of decline and keyless inheritance: no money gap; the keyless entry's output shape was pinned so a submitter
  cannot take a cut.
- Mutation testing after every change: every `require` whose removal is not covered by a neighbouring check is killed
  by a test. The survivors are five documented redundant checks. The tester does not
  mutate `if` conditions or arithmetic.

**Wallet engine.** Split into narrow modules; the reviewed rules are built into the program and an account whose rule
block or rules file does not match what was recorded at opening is refused; the offline signer shows exactly what it
will sign and waits for a person to confirm; the guard runs with public data only, uses two nodes, survives restarts,
and treats nodes going quiet as a warning, not as "all well"; fee and dust handling refuses, in plain words, payments that would
cost more than allowed or leave a stuck remainder. A broad review of the whole code base found two money gaps and
sixteen weak spots (none in the covenant itself); all confirmed items in the engine and shell were fixed and each fix
was checked by a second reader. A fuzzing run threw 5,334 malformed requests at the offline signer: none was signed.

**Android shell.** An adversarial review of the shell drove the move from a laptop service to an engine inside the
app, removing the network service, pairing and token entirely from the phone build. A later review of the Java code
found three medium issues, all fixed: an engine-method allow-list, a fresh fingerprint for irreversible moves, and
logs without engine bodies. A surface review then removed four JavaScript bridge methods left from laptop pairing (an outbound
HTTPS client and a secret store among them) and six guard methods the page could call but never needs, made the
screen-capture block on money screens follow the current real-money setting, and added `tools/surface_check.py`,
which fails when the page's routes, the bridge's allow-lists, the settings keys and the engine's methods drift apart.

## Android shell protections

- **Fingerprint per signature, short window.** Keys are sealed with a Keystore key that requires user authentication
  (`BIOMETRIC_STRONG` or the device credential; weak biometrics are not accepted) and is invalidated when a new
  fingerprint is enrolled. A key is unsealed only for the signature and wiped afterwards. After a fingerprint, everyday
  payments may sign without a new prompt for a short window the owner sets (0 means one fingerprint per payment).
- **Fresh fingerprint for irreversible moves.** Taking money out of the safe, showing the vault seeds, deleting the vault seeds,
  resetting the keys and changing the heir need a real fingerprint within the last 90 seconds, whatever the window.
- **Method allow-list.** The page can ask the engine only for a fixed list of methods; anything else is refused.
  Settings the page may read or write are also a fixed list. Key bytes never reach the page.
- **No secrets in logs.** Logs carry method names, status codes and sizes only; never balances, addresses, vault seeds,
  words or engine answer bodies.
- **WebView lockdown.** Only the bundled pages load and run scripts; every other request is blocked; no file or
  content access from the page; mixed content never allowed; geolocation off; the camera is granted only to the bundled
  page and only for video (the QR reader). The page's content security policy allows no inline script or style and
  connects only to itself and one price host. The price feed is fetched by Java from a single fixed URL prefix and only
  well-formed price points are passed on.
- **Screens.** Screens that show words or move money are always `FLAG_SECURE` in live mode: no screenshots, no screen
  recording, no recents thumbnail. A copied address is wiped from the clipboard after a minute.
- **Backups off.** `android:allowBackup="false"` keeps wallet files out of cloud backup; the Keystore keys that seal
  the secrets cannot leave the device in any case. Cleartext traffic is disabled.
- **Not debuggable** unless explicitly built so for testing with the debug key.

## Known limits

- **Within-limit theft.** A thief with the unlocked phone can spend up to the remaining limit. Set the limit with that
  in mind.
- **A rooted, unlocked phone** can copy a key while it is unsealed. The limit and the notice delay, not the hardware,
  are the guard against that.
- **The card can keep the coin alive.** Small spends by a thief holding the card keep restarting the heir clock. The
  stop keys are the answer.
- **Android may sleep the guard.** Doze can hold the network on battery with the screen off; without the battery
  exemption, alarms arrive at the next maintenance window. The notice delay should outlast that gap, and a second
  watch-only guard on another device covers the rest.
- **Two nodes are not consensus.** The guard trusts what two independent nodes agree on.
- **Privacy.** Balances, requests and destinations are public on chain.
- **Fees on small amounts.** Kaspa's storage mass makes small outputs expensive; requests below the remainder floor
  are refused, and small payments belong in the purse.
- **Proof coverage.** The testnet runs cover the covenant up to the version with decline and keyless inheritance; the
  hard-limit dial is covered by the engine proofs and mutation testing, not yet on chain. The mainnet proof round has not
  been run.
- **The heir path does not check the notice period.** From a Pending coin the heir payout checks only the heir
  delay, so the wallet requires an heir delay of at least ten times the notice delay; anyone deploying the covenant
  another way must do the same (see Deployer rules in [docs/CONTRACT.md](docs/CONTRACT.md)).
- **Offline signer robustness.** In fuzzing, 76 of 5,334 malformed inputs made the command-line offline signer abort instead
  of refusing cleanly. None was signed.
- **Supply chain.** There is no reproducible-build attestation or store listing yet. Verify the APK's signing
  certificate and build id against the published values before installing.
- **Self-custody.** Nobody can recover lost keys or vault seeds. Losing two of the three vault seeds loses the safe;
  keeping two vault seeds together with the phone gives a thief everything.
