# Testing

KaspaVault is checked in four layers. Each proves something the others cannot.

| Layer | Proves | Cannot prove |
|---|---|---|
| Engine proofs | The compiled covenant accepts every legitimate transaction shape and refuses every attack shape, in the real Kaspa script engine | That time really passes, or that consensus enforces the timelocks |
| Mutation testing | Every check in the covenant is needed by some test | That the tests cover attacks nobody thought of |
| Testnet runs | Consensus enforces the rules on a live network: timelocks, the limit, lineage, fees and storage mass | Mainnet conditions |
| Wallet and page tests | The engine's account handling, the page's data layer and house rules, and that every screen draws without a script error | The covenant itself |

## 1. Engine proofs

`engine-tests/` compiles `contracts/vault.sil` with the SilverScript compiler and runs complete transactions through
`kaspa-txscript` 2.1.0, the script engine Kaspa nodes use. Each case builds a real transaction, signs it, and checks
that the engine accepts or refuses it.

- `tests/vault.rs`: the test matrix. Must-succeed cases cover spending under the limit, the window reset, request then
  finish (partial and full), cancel from both states, cancel with an outside fee input, cancel of a coin sent straight
  to the address, decline by each stop key, top-up, inheritance from both states and keyless inheritance. Must-fail
  cases cover each attack as a one-line change: spending over the limit at once or across two spends, a timestamp lock
  time to reset the window early, a non-zero sequence, inflated fees, spending or requesting from Pending, wrong keys,
  the card used as a stop key and the reverse, an early finish, a finish to another destination or for another amount,
  burning more than the fee allowance, paying the safe too little or paying elsewhere, the vault coin at input 1, two
  vault coins sharing an output, altered continuation state or script, a stripped covenant binding, two
  continuations, non-SIGHASH_ALL signatures, dust and malformed requests, early inheritance, and requests with the hard
  limit on. The current run accepts 76 legitimate transactions and refuses 244 attacks.
- `tests/review2.rs`: proof cases written by an independent reviewer (6 accepted, 79 refused).

```bash
cd engine-tests
cargo test --test vault -- --nocapture
cargo test --test review2 -- --nocapture
```

The test reads the contract from `VAULT_SRC` when set, else `../contracts/vault.sil`.

## 2. Mutation testing

`tools/mutate.py` breaks one check at a time (deletes each `require`, flips each comparison, deletes each lineage and
state check), recompiles, and reruns the engine test binary. A mutant that still passes every test marks a check that
is untested or redundant.

```bash
(cd engine-tests && cargo test --test vault --no-run)   # note the path of the test executable it prints
python tools/mutate.py <absolute path to the vault test executable>
```

Result on the current contract: 137 mutants; 128 killed, 2 do not compile (`>=` to `>` on the coin-age check, which
the compiler rejects), 7 survive the vault suite. Run against both suites, exactly five survive, each a deliberate
duplicate already enforced by the next line or by `validateOutputState`:

- `this.activeInputIndex == 0` in the solo-input check (covered by the input count of 1)
- `raw.length == 65` in the signature check (a wrong length already fails the hash-type compare or the signature check)
- the covenant input count in the solo-continuation check (covered by the solo-input check)
- the covenant output count in the solo-continuation check (covered by the output index check)
- the duplicate covenant input count in `topup`

Mutation testing has been repeated after every contract change.

## 3. Testnet runs (testnet-10)

The `testnet-client` command-line driver ran every entry against public testnet-10 nodes (node version 2.1.0), with a
watcher following each account and, where noted, stop and safe keys signing on an offline device. Testnet dials: limit
1 KAS per 1,200 DAA, notice delay 3,000 DAA, heir delay 3,000 DAA, fee allowance 0.05 KAS, remainder floor 1 KAS.

Key results:

- **Consensus enforces the notice delay.** The identical finish transaction was refused by the node ("one of the
  transaction sequence locks conditions was not met") when the coin was too young, and accepted later with the same
  transaction id.
- **Consensus enforces the limit.** A spend over the limit forced past the wallet's own check was refused by the node
  ("script ran, but verification failed").
- **The window resets** after a full window: a third spend opened a new window and the counter restarted.
- **Early inheritance is refused** by the node; inheritance after the silence pays the heir, and from a Pending coin it
  pays the waiting request first.
- **Stop keys 1, 2 and 3** each cancelled an account to the 2-of-3 safe, signed offline where noted; decline worked
  online and offline, and a request still finished normally after two declines.
- **Keyless inheritance** was sent by the watcher itself the moment the silence passed.

### Transaction ids, first contract versions

The early runs recorded the first eight hex digits of each id.

| Account | Step | Transaction (prefix) | Result |
|---|---|---|---|
| 1 | fund 20 TKAS | `f17da3bd` | accepted |
| 1 | spend 0.5 | `ed281a93` | accepted; fee counted toward the limit |
| 1 | request 10 | `1fa5f5c8` | accepted; Pending |
| 1 | cancel to the safe | `e4dade42` | accepted |
| 2 | fund | `b9d910b2` | accepted |
| 2 | request 10 | `17b0b346` | accepted |
| 2 | finish at coin age 191 DAA, then after 3,000 | `99b8650b` | refused by the node, then the same id accepted |
| 2 | cancel to the safe | `56573da9` | accepted |
| 3 | fund | `2f0892cd` | accepted |
| 3 | spend 2 over the 1 KAS limit, forced | `6eabc32c` | refused by the node |
| 3 | request 10 | `4c9b6cc3` | accepted; watcher alert within 2 seconds |
| 3 | cancel from Pending | `b765763e` | accepted |
| 4 | cancel signed offline | `1493af5b` | accepted |
| 5 | cancel signed offline (compact response) | `0fc9cdd3` | accepted |
| 6 | spend 0.6, then 0.6 after the window | `b97a022d`, `2de2d835` | accepted; counter reset |
| safe | send from the safe, signed offline | `8dd7d27c4678c1f0` | accepted |

### Transaction ids, current covenant design

Three stop keys, 2-of-3 safe, top-up, inheritance; then the hardened revisions, decline and keyless inheritance.

| Account | Step | Transaction | Result |
|---|---|---|---|
| 8 | fund 3 | `de646abbad167b865c800d8bd293f63191a69354018241ed0773ece8a670e132` | accepted |
| 8 | spend 0.5 | `3a3211aaf70b4ca91ffb269a2d4b4744d22e3d6e49e9f5a029237cc068597b0c` | accepted |
| 8 | top-up 1 (staging, top-up) | `595bae0aac077d3c7403622a1ff9e77cb91d8958b1d181a97f2a0e7db924d357`, `154fe8a2ed68244720c129f39280c69f58d40fd3c997caf174786fcb422cfb47` | accepted; state unchanged |
| 8 | inherit at coin age 250 DAA | `d477308fd8eb767ecf60b88fa25f7599d22eb1d705e99a62f4a9418fb00a8b0b` | refused by the node (sequence lock) |
| 8 | cancel by stop key 2, signed offline | `31a8d7b8f67fd219a715a12b543fd243753d7ace7e33c1f73eb4f6168c22d19d` | accepted |
| 9 | fund 2 | `d90d55613be50c3893941450acfef133f229dd6e435302cec0c08062c7d113de` | accepted |
| 9 | request 0.5 | `71bd6ea5fcaa985833973d25f5bfd198c52ff515e39fe27ae59357cd002f2523` | accepted |
| 9 | inherit after the silence | `e48fb5050afdf136b2880a0c0d4c1657f8e1db0e109ee7114ca2b5670291fae4` | accepted; on this earlier version the whole coin went to the heir |
| 10 | fund 1.5 | `2f253045ea04222675ce1db023fd485fc6894da083d619327465cbf861e2e0ee` | accepted |
| 10 | cancel by stop key 3, signed offline | `ab1bce5f93588b77a18e60770f7df5b21e943efbaa6fe18af77edad60d5b322d` | accepted |
| 12 | fund 3 | `56c01342d5b9a520371c9455f67eb4efd83e8d34d88148b1a4d0018dc8b64691` | accepted |
| 12 | spend 0.5 | `2e6471ff4f0a6e83f2a0f1b0530db336de04361d09929855583dfe4954dcbe7a` | accepted |
| 12 | spend 0.6 over the limit, forced | `5f958eefc0d75be729c774036a7c20516af17742ed3b38e00fbd9b67210df6e6` | refused by the node |
| 12 | top-up 1 (staging, top-up) | `7d6f827b70d9cee1782730a93adee8c6964c5b2753774a686fae9aa2b21225b2`, `1ec886b1cffdb7ba42a4750b2e3e41dd965d3656a6357a58b5a533404d75530c` | accepted |
| 12 | request 1 | `670a467e6c41a716f954c71dc01bb4bc1e77881ec689998820188420ac56403e` | accepted |
| 12 | cancel from Pending by stop key 2, signed offline | `899f33f366e4e74dd7aa931ec1560a537ec921a811719fa1dca0f4f78a6c8cb4` | accepted |
| 13 | fund 3 | `80d253e086e3b8a07d4bce761d6db900f69d1fa8109ba007c3256c27bc1666e7` | accepted |
| 13 | request 1 | `4648c0fa32491da3b901847503d1e19a631d8c9de0a3d11d7d2ec1cdf6d59c3a` | accepted |
| 13 | finish early, then after the delay | `ef058d34f63f9710ed08a3ba4e61ed50a1b95d99ea9e0a2f89be06e5915f9244` | refused by the node, then the same id accepted |
| 14 | fund 3 | `6980b2c139db38ec0d481aa32cff5c9b20df17381c0675e0042917d12138e331` | accepted |
| 14 | request 1 | `7c1daeaa2d10895fe298d3baebd74ad73b21bcb8a60d4fc4601a7bae92f96678` | accepted |
| 14 | inherit from Pending, early, then after the silence | `d7a9de722c18960b2a2d6dfe069f6827ac7f1541f6c1f327daf698309cb8d5b0` | refused by the node, then the same id accepted; the request paid first, the rest to the heir |
| 15 | cancel of an account at the floor, signed offline | `2b0bb37a9003029d3330319e72e84a62440e8738839154fd1ed10e484db7b366` | accepted; the account paid its own way out |
| 16 | top-up 0.1 (staging, top-up) | `aacddd09fa8bcba5001dd1b7ecb97cc579dc3bce613aebca9f1a4c0fa2fa7989`, `7be3b5158663e2c60518b5009ca5c01eea3151b6b760cdbab9092f528786a21f` | accepted |
| 16 | request 1 | `59aefb82d077f7204d50005924b8bbb6882d6d5884906ee4c9deb99690c992be` | accepted |
| 16 | finish | `a0ad5da9b4c15e67d291dcd13729de7ae6b21c8ba6dffd77ee496ddec41d28cb` | accepted; fee within the allowance |
| 17 | fund 3 | `9e828df0243a2b6ae28bbd1ad9c0b3c90ef72b5367c9013f33d96250d8b054f0` | accepted |
| 17 | request 1 | `19fd3afd0fd232efea37df3da9807f6ec68ac6c9cb80add1c7400dca96af5d4e` | accepted |
| 17 | decline by stop key 1 | `1b4878075caa4873759fb84616c9c02e63d2b15bcfcd25f618ad8197aa473886` | accepted; back to Locked, nothing left |
| 17 | request 1 | `78f8f7000c3c9c7a99730f9f35cac3447b1af7882001cbe3c6db7e16e4723b32` | accepted |
| 17 | decline by stop key 2, signed offline | `acb7234f94d9620d10aa7335cb9e5a7da86868c4746929cb5f2761d7b4f82c8a` | accepted |
| 17 | request 1 | `9216d25bf66e21991bc5b971eea9a4c4bb4e90936f4138a66a94e21002922c5c` | accepted |
| 17 | finish early, then after the delay | `0d476e39110f8f6dcebd5ca1052e233ae78b0f9bf45e203acca63dcb22d13bfb` | refused by the node, then the same id accepted |
| 17 | keyless inheritance, early, then sent by the watcher | `b143cd8f7e611b90a46048ef7ca1bf01e7e3ba7cb9459c3eca32cd3def1767f3` | refused by the node, then the same id accepted |

Account 9 ran on the version before inheritance from Pending was changed to honour the waiting payment first; account
14 shows the corrected behaviour. All ids can be looked up on any testnet-10 explorer.

Not yet on chain: the hard-limit dial (proven in the engine and by mutation testing) and the full proof round from the
phone on mainnet.

## 4. Wallet engine and page tests

**Engine unit tests** cover keys, transaction building, fee and dust rules, the guard, the offline signer's request and
response format, accounts, rule changes, heir changes, and the phone call interface (107 tests).

```bash
cd testnet-client
cargo test --lib
cargo clippy
```

**Page tests** (Node 18 or newer, from the repository root):

- `data.test.js` loads `app/web/data.js` in a sandbox with a fake Java bridge standing in for the shell and the engine;
  each test is one rule the phone relies on.
- `pagelint.js` enforces the page's house rules (nesting depth, function length, line length) and exits 1 on a break.
- `smoke.js` opens the page in a headless Chromium, visits every screen and taps every visible button; any script error
  fails. `--live` runs the phone page itself with a fake bridge injected, so the live branches run. Set `KV_CHROME` to
  a headless Chromium or Chrome executable.

```bash
node --test app/web-tests/data.test.js
node app/web-tests/pagelint.js
KV_CHROME=<path to headless Chromium> node app/web-tests/smoke.js
KV_CHROME=<path to headless Chromium> node app/web-tests/smoke.js --live
```
