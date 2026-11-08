# The vault covenant

Source: [`contracts/vault.sil`](../../contracts/vault.sil) (SilverScript). This page describes the current version,
which takes 20 constructor arguments and includes the hard-limit dial.

## In plain terms

The savings account is a single coin on the Kaspa chain whose locking script carries its own rules and its own small
state. Every time the account moves, the script demands that the new coin carry the same rules and a correctly updated
state, so the rules travel with the money.

- The **card** (spend key) can pay out up to a limit per window. Fees count toward the limit.
- For anything larger, the card can only **request** a payment. The request records a destination and an amount;
  nothing leaves. After the **notice delay**, anyone may **finish** it, and the money can go only to that destination,
  only for that amount.
- Any of three **stop keys** can **decline** a waiting request (the coin goes back to normal, nothing leaves) or
  **cancel** the account, sending everything to the fixed **safe** address and nowhere else.
- The card can **top up** the account from outside coins; nothing can leave in a top-up.
- After the coin has sat untouched for the **heir delay**, the heir's key can take everything to the heir's address.
  If the owner chose so, the same transfer may be sent by anyone without a signature, but only to the heir.
- With the **hard limit** dial on, requests are switched off for good: only the limit, the stop and the heir remain.

## State

The script keeps five fields, rewritten on every continuation:

| Field | Meaning |
|---|---|
| `stage` | 0 = Locked (normal), 1 = Pending (a request is waiting) |
| `destHash` | blake2b of the waiting request's destination scriptPubKey; all zeros when Locked after a finish or decline |
| `amount` | the waiting request's amount in sompi; 0 when Locked |
| `windowStart` | DAA score at which the current spending window opened |
| `spentInWindow` | sompi already spent in the current window, fees included |

Amounts are in sompi (10^8 per KAS). Clocks (`delay`, `windowSize`, `inheritDelay`) are in DAA score units, about 10
per second.

## Constructor arguments

| Argument | Meaning |
|---|---|
| `spendPk` | The card: spends within the limit, requests, tops up |
| `cancelPk`, `cancelPk2`, `cancelPk3` | The three stop keys; unused slots repeat `cancelPk` |
| `coldSpkHash` | blake2b of the safe's serialized scriptPubKey (a 2-of-3 multisig) |
| `heirPk`, `heirSpkHash` | The heir's key and the blake2b of the heir's scriptPubKey |
| `inheritDelay` | Silence before inheritance, in DAA (must stay below 2^32, the relative-timelock domain) |
| `autoHeir` | 1 allows the keyless `inheritOpen`; 0 keeps inheritance to the heir's key |
| `hardLimit` | 1 makes `request` fail outright; 0 keeps the request/finish path |
| `delay` | Notice delay before `finish`, in DAA |
| `capPerWindow` | Spending limit per window, in sompi |
| `windowSize` | Length of the spending window, in DAA |
| `maxFee` | Most a keyless or stop path may leave as fee, in sompi |
| `minRemainder` | Smallest output any path may create, so every coin stays minable under Kaspa's storage mass rule |
| `initStage`, `initDestHash`, `initAmount`, `initWindowStart`, `initSpent` | The opening state |

With no heir, the deployer sets `heirPk = cancelPk`, `heirSpkHash = coldSpkHash`, `inheritDelay = 4000000000` and
`autoHeir = 0`, so inheritance can only ever pay the safe, and only after more than 12 years.

## Rules common to every entry

- **The vault coin is input 0.** This stops two vault coins from being satisfied by one output, even across
  different covenant ids.
- **Signatures are SIGHASH_ALL only.** Each signature must be 65 bytes ending in hash type `0x01`, so no unsigned part
  of the transaction (outputs, arguments) can be changed by someone relaying it.
- **Lineage.** Entries that continue the account require exactly one input and exactly one output carrying this
  covenant id, at a fixed output index, and `validateOutputState` checks that output's script is the same contract
  with the expected new state. A coin sent straight to the vault address has no covenant id; such coins can leave only
  through `cancel`, `inherit` or `inheritOpen`.
- **Stop key selection.** `which` (0, 1 or 2) picks the stop key first; the signature is then checked against that key.
  Any other value fails.

## Entries

### `spend(sig)` — pay within the limit
Requires: a SIGHASH_ALL signature by `spendPk`; Locked; `0 <= spentInWindow <= capPerWindow`; the vault coin as the
only input; exactly two outputs, the continuation at output 1; input sequence 0 (so consensus enforces the lock time)
and a lock time below 500,000,000,000 (DAA domain, not a timestamp).
Accounting: outflow = vault input value - continuation value, so the fee counts. If the lock time is at least
`windowStart + windowSize` **and** at least the spent coin's DAA score + `windowSize`, a new window opens at the lock
time and the counter restarts at this outflow; otherwise the outflow is added to the counter.
Then: counter <= `capPerWindow`; continuation >= `minRemainder`; new state Locked with the updated window fields.

### `request(sig, newDestSpk, newAmount)` — start a larger payment
Requires: `hardLimit == 0`; a SIGHASH_ALL signature by `spendPk`; Locked; the vault coin as the only input; exactly
one output, the continuation; `newAmount >= minRemainder`; `newDestSpk` 36 or 37 bytes (a standard pay-to-pubkey or
pay-to-script-hash scriptPubKey); continuation >= input - `maxFee`; `newAmount + maxFee <= continuation`; and what
`finish` will leave behind is either at most `maxFee` or at least `maxFee + minRemainder`.
New state: Pending, `destHash = blake2b(newDestSpk)`, `amount = newAmount`, window fields unchanged. The destination is
passed in full so a watcher reading the chain can show where the money is headed.

### `finish()` — complete a request after the notice delay (no signature)
Requires: Pending; the vault coin as the only input; coin age >= `delay` (a relative timelock, enforced by consensus);
output 0 pays exactly `amount` to the script whose hash is `destHash`.
Then either one output only, no output carrying the covenant, and remainder <= `maxFee` (the account ends); or exactly
two outputs, the continuation at output 1 with at least remainder - `maxFee` and at least `minRemainder`, new state
Locked with `destHash` zeroed, `amount = 0`, window fields unchanged.

### `decline(sig, which)` — refuse a waiting request
Requires: a SIGHASH_ALL signature by the chosen stop key; Pending; the vault coin as the only input; exactly one
output, the continuation, holding at least input - `maxFee` and at least `minRemainder`.
New state: Locked, `destHash` zeroed, `amount = 0`, window fields unchanged (a declined request neither widens nor
resets the limit). A stop key, not the card, because the card may be the thief's.

### `topup(sig)` — add money
Requires: a SIGHASH_ALL signature by `spendPk`; Locked; the vault coin at input 0 and the only covenant input (other
plain inputs pay the top-up and the fee); exactly one output, the continuation, holding at least the vault coin's value.
State unchanged.

### `cancel(sig, which)` — emergency stop to the safe
Requires: a SIGHASH_ALL signature by the chosen stop key; any stage; the vault coin at input 0; no output carrying the
covenant (the account ends); output 0 pays the safe (`blake2b(scriptPubKey) == coldSpkHash`) at least input - `maxFee`
and at least `minRemainder`. Extra inputs and outputs are allowed, so a fee spike can be paid from outside the account.

### `inherit(sig)` — the heir, after the silence
Requires: a SIGHASH_ALL signature by `heirPk`, then the shared heir payout below.

### `inheritOpen()` — the same payout with no signature
Requires: `autoHeir == 1`; exactly one output when Locked, exactly two when Pending (so whoever submits it can pay the
fee or add an outside coin, never take a cut); then the shared heir payout.

**Heir payout (both entries):** the vault coin at input 0; no output carrying the covenant; coin age >= `inheritDelay`.
If Pending, the owner's last signed instruction is honoured first: output 0 pays exactly `amount` to the committed
destination and the heir output is output 1. The heir output pays the script whose hash is `heirSpkHash` at least
(everything owed) - `maxFee`, and at least `minRemainder`.

Every entry that continues the account creates a new coin, so the silence clock restarts on every spend, request,
finish, decline and top-up.

## The dials

| Dial | Argument | Testnet proof value | Mainnet default |
|---|---|---|---|
| Daily limit | `capPerWindow` | 1 KAS | 100 KAS |
| Spending window | `windowSize` | 1,200 DAA (about 2 minutes) | 1 day |
| Notice delay | `delay` | 3,000 DAA (about 5 minutes) | 1 day |
| Hard limit | `hardLimit` | off | off unless chosen |
| Heir delay | `inheritDelay` | 3,000 DAA | 180 days |
| Keyless inheritance | `autoHeir` | proven both ways | off unless chosen |
| Fee allowance | `maxFee` | 0.05 KAS | 0.05 KAS |
| Remainder floor | `minRemainder` | 1 KAS | 1 KAS |

The wallet refuses to open an account with dials that cannot work: every clock between 1 and 2^32 - 1 DAA, every amount
above zero, `maxFee` below `minRemainder`, and (outside the testnet proof preset) an heir delay of at least ten times
the notice delay, so a withdrawal that is ready to leave can never be inherited at the same moment. Dials are fixed in
the coin's script; changing them means opening a new account and moving the money across.

Practical consequences of the floors: a request must be at least `minRemainder` (1 KAS), and an account must keep
either nothing or at least `maxFee + minRemainder` after a payment. The wallet checks these before building anything
and explains a refusal in words.

## What cannot happen

- Nothing leaves above the limit without first being requested, recorded on chain, and left waiting for the notice
  delay.
- A waiting payment cannot be redirected or resized: the destination hash and amount are fixed in the state, and
  signatures cover the whole transaction.
- A stop key cannot pay anyone but the safe. The worst a stolen stop key does is freeze the account into the safe or
  decline a request.
- The card cannot reach the safe, change the heir, or move money with a top-up.
- The heir cannot take anything before the coin has been silent for the full heir delay, and from a Pending coin the
  waiting payment is honoured first.
- A keyless `finish` or `inheritOpen` cannot send money anywhere except the recorded destination or the heir; a
  stranger submitting one can only pay the fee.
- No path can leave a coin too small to be mined: every output is either zero or at least `minRemainder`.
- With `hardLimit` set, no large withdrawal can ever start, whoever signs.
- A coin sent to the vault address from outside cannot be spent by the card or finished by a stranger.

## What the covenant does not do

- It does not stop payments within the limit. That is the price of being able to pay at a shop.
- It does not hide anything. Balances, requests and destinations are public, like every Kaspa address.
- It does not stop the card from keeping the coin alive. A thief holding the card can keep renewing the coin with
  small spends, which restarts the heir clock; the stop keys remain the answer to a stolen card.
- It does not count coins sent to the address without lineage as part of the account; the wallet must follow the
  covenant id, not the address.
