# Security

## Status

**Unaudited. Testnet only.** No third-party review has been performed. Do not
route real funds through this contract.

## Reporting a vulnerability

Open a private security advisory through GitHub's "Report a vulnerability" flow
rather than a public issue. Please do not disclose publicly until a fix ships.

## Trust model

| Party | Can | Cannot |
|---|---|---|
| Merchant | Set plan pricing and cadence, redirect their own treasury, disable plans, deactivate themselves | Charge a subscriber more than the plan amount per interval, charge early, touch another merchant's plans |
| Subscriber | Approve an allowance, subscribe, pause, resume, cancel, revoke the allowance at any time | Skip a billing cycle while retaining access |
| Keeper | Trigger `process_payment` for anyone | Redirect funds, charge before the due date, bypass the retry throttle |
| Contract | Draw from an approved allowance up to the plan amount on schedule | Custody funds — it never holds a balance |

The subscriber's real protection is the allowance. Revoking it via
`token.approve(spender, 0, ...)` stops all future collection immediately, with
no contract call required and no merchant cooperation needed.

## Known limitations

These are understood and unresolved. They are recorded here rather than left for
a reader to discover.

### Merchant can raise the price of an existing subscription

`update_plan` applies to everyone already subscribed, effective on their next
cycle. Subscribers are protected only by the size of the allowance they granted
— a subscriber who approved a large budget for convenience has implicitly
accepted repricing up to that budget.

A future version should either version plans so existing subscribers stay on
their signup terms, or emit a price-change event with a mandatory notice period.
Clients should not encourage subscribers to approve unbounded allowances.

### Plan and subscription lists grow without bound

`MerchantPlans` and `SubscriberPlans` are single `Vec<u64>` ledger entries that
only ever get appended to. A merchant who creates enough plans will eventually
push the entry past the ledger's size limit, at which point `create_plan` starts
failing permanently for that merchant. There is no compaction and no removal.

The fix is paginated storage keyed by index bucket. Until then this is a real
ceiling, not a theoretical one.

### Storage TTL is extended on write only

`bump` is called from the save helpers, so an entry that is read but never
written can reach its TTL and be archived. A long-paused subscription is the
realistic case. Restoring an archived entry is possible but requires an explicit
restore operation the current clients do not perform.

### Allowance expiry is silent

When an approval lapses, `process_payment` returns `Retrying` and the
subscription slides toward `Failed` with no distinct on-chain signal separating
"subscriber is broke" from "approval expired". Both surface as `pay_fail`.
Clients should poll `get_billing_allowance` to tell them apart.

### No reentrancy concern, but note the ordering

`_charge` dispatches to the token contract before subscription state is written.
Soroban does not permit reentrancy into the same contract, so this is safe today
and is called out only so a future refactor does not quietly rely on it.

## Auditing notes

If you are reviewing this contract, start with `_charge` and
`process_payment` in `src/lib.rs`, then read
`keeper_can_bill_without_any_authorization` in `src/test.rs`.

That test uses `env.set_auths(&[])` to run in enforcing mode with an empty
authorization set. It exists because an earlier version of this contract passed
a full test suite while being fundamentally unable to bill anyone: every test
ran under `env.mock_all_auths()`, which stubs out the exact mechanism the
payment path depends on.

**Any change to the payment path must be validated without blanket auth
mocking.** A broken authorization model looks perfectly healthy under
`mock_all_auths`.
