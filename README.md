# SS-Billing — Subscription Engine

Soroban smart contract for recurring on-chain billing on Stellar. Merchants
publish plans; subscribers approve a spending allowance once; a keeper collects
each cycle without further signatures.

This repository contains **only the contract**. The
[backend](https://github.com/SS-BILL/SS-Billing-backend) and
[frontend](https://github.com/SS-BILL/SS-Billing-frontend) live in their own
repositories.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Soroban SDK](https://img.shields.io/badge/soroban--sdk-21.7-orange)](https://developers.stellar.org/docs/build/smart-contracts)

> **Status: testnet only.** This contract has not been audited and has not been
> deployed to mainnet. Do not route real funds through it.

---

## How billing works

The contract never holds funds. It moves them directly from subscriber to
merchant treasury using the SAC allowance mechanism.

```
1. Merchant   register_merchant(name, treasury)      signs
2. Merchant   create_plan(amount, interval, ...)     signs
3. Subscriber token.approve(spender = contract, ...) signs   <-- the one signature
4. Subscriber subscribe(plan_id)                     signs   <-- charges cycle 1
5. Keeper     process_payment(subscriber, plan_id)   no signature needed
   ...repeated every interval
```

Step 5 is the point of the contract. `process_payment` is callable by anyone —
a keeper, the merchant, a cron job, a stranger — because the funds move under
the allowance granted in step 3, not under the caller's authority. The contract
authorizes the `transfer_from` as itself, which Soroban permits automatically
for sub-invocations a contract makes directly.

### The allowance is not permanent

Stellar allowances carry an **expiration ledger**. The accurate description of
this model is *"sign once per approval window"*, not *"sign once, forever"*.
When the approval lapses or its balance runs down, `process_payment` returns
`Retrying` and the subscription drifts into `GracePeriod` and then `Failed`.

Clients must surface `get_billing_allowance` and prompt for re-approval before
the window closes. A dashboard that shows an active subscription without showing
a lapsing allowance is showing a subscription that is about to stop paying.

---

## Lifecycle

```
                subscribe
                    │
                    ▼
              ┌──────────┐  pause    ┌────────┐
              │  Active  │──────────▶│ Paused │
              │          │◀──────────│        │
              └────┬─────┘  resume   └────────┘
                   │
       charge fails│  ┌──────────────┐
                   └─▶│ GracePeriod  │──┐ charge succeeds
                      └──────┬───────┘◀─┘  (returns to Active)
                             │
        retries exhausted OR │
        grace window closed  ▼
                      ┌──────────┐
                      │  Failed  │  terminal — resubscribe required
                      └──────────┘
```

`cancel_subscription` moves to `Cancelled` from any non-terminal state.

**Pause does not shift the billing anchor.** A subscription paused before its
due date and resumed after it is immediately billable for the cycle it kept
access through.

---

## Contract interface

### Merchant

| Function | Auth | Notes |
|---|---|---|
| `register_merchant(name, treasury_wallet)` | treasury | The treasury address is the merchant identity |
| `update_treasury(merchant_id, new_treasury)` | merchant | Routes future collections elsewhere |
| `create_plan(merchant_id, name, amount, token, interval, grace_period, retry_limit, retry_interval) -> plan_id` | merchant | |
| `update_plan(merchant_id, plan_id, amount, interval, grace_period, retry_limit, retry_interval)` | merchant | Applies to existing subscribers on their next cycle |
| `disable_plan(merchant_id, plan_id)` | merchant | Halts new signups **and** further collection |

### Subscriber

| Function | Auth | Notes |
|---|---|---|
| `subscribe(subscriber, plan_id)` | subscriber | Charges cycle 1; fails without a sufficient allowance |
| `pause_subscription(subscriber, plan_id)` | subscriber | |
| `resume_subscription(subscriber, plan_id)` | subscriber | Preserves the original due date |
| `cancel_subscription(subscriber, plan_id)` | subscriber | Terminal |

### Keeper

| Function | Auth | Returns |
|---|---|---|
| `process_payment(subscriber, plan_id)` | **none** | `Paid` \| `Retrying` \| `Failed` |

A failed charge returns `Ok(Retrying)` or `Ok(Failed)` — never `Err`. Returning
`Err` from a Soroban contract rolls back the invocation, which would discard the
retry bookkeeping the failure path exists to record. Reserve `Err` handling for
genuine caller errors such as `BillingNotDue`.

### Queries

| Function | Notes |
|---|---|
| `get_merchant`, `get_plan`, `get_subscriber`, `get_payment` | Raw record lookups |
| `get_merchant_plans`, `get_subscriber_plans` | Plan id lists |
| `get_billing_allowance(subscriber, plan_id)` | Remaining drawable amount; `0` once expired |
| `is_billable(subscriber, plan_id)` | Whether `process_payment` would attempt a charge now |

Keepers should filter on `is_billable` rather than comparing `next_billing_at`
themselves — the latter ignores retry throttling and terminal states, and each
wrong guess costs a transaction fee.

---

## Errors

| Code | Error | Meaning |
|---|---|---|
| 1–3 | `MerchantNotFound`, `MerchantAlreadyExists`, `MerchantInactive` | |
| 4–5 | `PlanNotFound`, `PlanInactive` | |
| 6–8 | `SubscriptionNotFound`, `SubscriptionNotActive`, `SubscriptionAlreadyExists` | |
| 9 | `BillingNotDue` | Not yet due, or the retry throttle has not elapsed |
| 10 | `InsufficientBalance` | Subscriber cannot cover the charge |
| 11 | `RetryLimitExceeded` | Reserved; failures now report via `PaymentOutcome` |
| 12–14 | `Unauthorized`, `InvalidAmount`, `InvalidInterval` | |
| 15–17 | `AlreadyPaused`, `NotPaused`, `AlreadyCancelled` | |
| 18 | `InsufficientAllowance` | Approval too small or expired |
| 19 | `SubscriptionFailed` | Terminal; resubscribe required |
| 20 | `InvalidRetryInterval` | `retry_interval` of 0 with retries enabled |

---

## Events

| Topic | Payload |
|---|---|
| `merch_reg` | `()` |
| `plan_new` | `plan_id` |
| `sub_new` | `(plan_id, next_billing_at)` |
| `pay_ok` | `(amount, timestamp)` |
| `pay_fail` | `(plan_id, retries)` |
| `retry` | `(plan_id, attempt)` |
| `sub_pause`, `sub_res`, `sub_canc` | `plan_id` |

All topics are `symbol_short!`, which caps at 9 characters — a limit worth
remembering, since exceeding it is a compile error rather than a runtime one.

---

## Development

```bash
make test        # cargo test
make lint        # clippy, warnings denied
make fmt         # rustfmt
make check       # everything CI runs
make build       # compile to wasm32-unknown-unknown
```

Requires the [Stellar CLI](https://developers.stellar.org/docs/tools/stellar-cli)
for `make optimize` and `make deploy`. The toolchain is pinned in
`rust-toolchain.toml` and dependencies in `Cargo.lock` — both are needed for the
deployed WASM hash to be reproducible from this source.

### Deploying

```bash
stellar keys generate --global alice --network testnet --fund
make deploy SOURCE=alice NETWORK=testnet
```

Pass the resulting contract id to the backend as `CONTRACT_ID`.

---

## Testing

```bash
cd subscription-engine && cargo test
```

37 tests covering the state machine, billing schedule, failure handling and
queries.

The suite deliberately avoids a blanket `env.mock_all_auths()`. Mocking is
confined to the fixture, where it substitutes for signatures a user genuinely
provides. `keeper_can_bill_without_any_authorization` runs under
`env.set_auths(&[])` — real enforcing mode with an empty authorization set — and
is the test that actually proves the delegated-billing claim.

Any change to the payment path should be validated against that test
specifically. Blanket auth mocking will make a broken authorization model look
perfectly healthy.

---

## License

MIT — see [LICENSE](LICENSE).
