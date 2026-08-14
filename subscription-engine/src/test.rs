#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, Symbol,
};

use crate::errors::ContractError;
use crate::types::{PaymentOutcome, SubscriptionStatus};
use crate::{SubscriptionEngine, SubscriptionEngineClient};

const INTERVAL: u64 = 2_592_000; // 30 days in seconds
const GRACE: u64 = 86_400; // 1 day
const RETRY_INTERVAL: u64 = 3_600; // 1 hour
const AMOUNT: i128 = 100_0000000; // 100 tokens (7 decimals)
const RETRY_LIMIT: u32 = 3;

/// Ledger at which subscriber approvals lapse. Comfortably beyond every
/// sequence number the tests warp to, except the one that tests expiry.
const APPROVAL_EXPIRY: u32 = 500_000;

struct Ctx {
    env: Env,
    client: SubscriptionEngineClient<'static>,
    contract_id: Address,
    merchant: Address,
    subscriber: Address,
    token: Address,
}

impl Ctx {
    fn token_client(&self) -> TokenClient<'_> {
        TokenClient::new(&self.env, &self.token)
    }

    /// Grant the contract an allowance to draw from the subscriber. This is the
    /// one-time authorization the whole delegated-billing model rests on.
    fn approve(&self, amount: i128, expiration_ledger: u32) {
        self.token_client().approve(
            &self.subscriber,
            &self.contract_id,
            &amount,
            &expiration_ledger,
        );
    }

    /// Move the ledger forward in both time and sequence.
    fn warp(&self, timestamp: u64, sequence_number: u32) {
        self.env.ledger().set(LedgerInfo {
            timestamp,
            protocol_version: 21,
            sequence_number,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1,
            min_persistent_entry_ttl: 1,
            max_entry_ttl: 6_312_000,
        });
    }

    /// Spend the subscriber down to `remaining` so the next charge fails on
    /// balance rather than on allowance.
    fn drain_balance_to(&self, remaining: i128) {
        let token = self.token_client();
        let balance = token.balance(&self.subscriber);
        if balance > remaining {
            let sink = Address::generate(&self.env);
            token.transfer(&self.subscriber, &sink, &(balance - remaining));
        }
    }

    fn subscription(&self, plan_id: u64) -> crate::types::Subscriber {
        self.client
            .get_subscriber(&self.subscriber, &plan_id)
            .unwrap()
    }
}

/// Base fixture. Auth is mocked here because registration, plan creation,
/// approval and signup are all operations a real user genuinely signs.
///
/// Billing tests deliberately drop back to enforcing mode — see
/// `keeper_can_bill_without_any_authorization`.
fn setup() -> Ctx {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, SubscriptionEngine);
    let client = SubscriptionEngineClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    StellarAssetClient::new(&env, &token).mint(&subscriber, &(AMOUNT * 10));

    Ctx {
        env,
        client,
        contract_id,
        merchant,
        subscriber,
        token,
    }
}

fn plan_with(ctx: &Ctx, retry_limit: u32, retry_interval: u64) -> u64 {
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "AcmeCorp"), &ctx.merchant);
    ctx.client.create_plan(
        &ctx.merchant,
        &Symbol::new(&ctx.env, "Pro"),
        &AMOUNT,
        &ctx.token,
        &INTERVAL,
        &GRACE,
        &retry_limit,
        &retry_interval,
    )
}

fn register_and_plan(ctx: &Ctx) -> u64 {
    plan_with(ctx, RETRY_LIMIT, RETRY_INTERVAL)
}

/// Fixture for the common case: an approved, active subscription.
fn subscribed(ctx: &Ctx) -> u64 {
    let plan_id = register_and_plan(ctx);
    ctx.approve(AMOUNT * 10, APPROVAL_EXPIRY);
    ctx.client.subscribe(&ctx.subscriber, &plan_id);
    plan_id
}

// ── Merchant ─────────────────────────────────────────────────────────────────

#[test]
fn registers_merchant() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    let m = ctx.client.get_merchant(&ctx.merchant).unwrap();
    assert!(m.active);
    assert_eq!(m.treasury_wallet, ctx.merchant);
}

#[test]
fn rejects_duplicate_merchant() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    let result = ctx
        .client
        .try_register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    assert_eq!(result, Err(Ok(ContractError::MerchantAlreadyExists)));
}

#[test]
fn updates_treasury() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    let new_treasury = Address::generate(&ctx.env);
    ctx.client.update_treasury(&ctx.merchant, &new_treasury);
    assert_eq!(
        ctx.client
            .get_merchant(&ctx.merchant)
            .unwrap()
            .treasury_wallet,
        new_treasury
    );
}

// ── Plans ────────────────────────────────────────────────────────────────────

#[test]
fn creates_plan() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    assert_eq!(plan_id, 1);
    let plan = ctx.client.get_plan(&plan_id).unwrap();
    assert_eq!(plan.amount, AMOUNT);
    assert_eq!(plan.retry_interval, RETRY_INTERVAL);
    assert!(plan.active);
}

#[test]
fn rejects_plan_with_zero_amount() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    let result = ctx.client.try_create_plan(
        &ctx.merchant,
        &Symbol::new(&ctx.env, "Bad"),
        &0i128,
        &ctx.token,
        &INTERVAL,
        &GRACE,
        &RETRY_LIMIT,
        &RETRY_INTERVAL,
    );
    assert_eq!(result, Err(Ok(ContractError::InvalidAmount)));
}

#[test]
fn rejects_retries_without_a_retry_interval() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    let result = ctx.client.try_create_plan(
        &ctx.merchant,
        &Symbol::new(&ctx.env, "Bad"),
        &AMOUNT,
        &ctx.token,
        &INTERVAL,
        &GRACE,
        &RETRY_LIMIT,
        &0u64,
    );
    assert_eq!(result, Err(Ok(ContractError::InvalidRetryInterval)));
}

#[test]
fn disables_plan() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    ctx.client.disable_plan(&ctx.merchant, &plan_id);
    assert!(!ctx.client.get_plan(&plan_id).unwrap().active);
}

#[test]
fn rejects_plan_update_from_another_merchant() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    let impostor = Address::generate(&ctx.env);
    let result = ctx.client.try_update_plan(
        &impostor,
        &plan_id,
        &AMOUNT,
        &INTERVAL,
        &GRACE,
        &RETRY_LIMIT,
        &RETRY_INTERVAL,
    );
    assert_eq!(result, Err(Ok(ContractError::Unauthorized)));
}

// ── Signup ───────────────────────────────────────────────────────────────────

#[test]
fn subscribe_charges_first_cycle_and_schedules_next() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);

    let sub = ctx.subscription(plan_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.next_billing_at, INTERVAL); // ledger starts at t=0
    assert_eq!(
        ctx.token_client().balance(&ctx.merchant),
        AMOUNT,
        "first cycle should land in the treasury at signup"
    );
}

#[test]
fn subscribe_fails_without_an_allowance() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    // No approve() call — the subscriber never authorized the contract.
    let result = ctx.client.try_subscribe(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::InsufficientAllowance)));
    assert!(
        ctx.client
            .get_subscriber(&ctx.subscriber, &plan_id)
            .is_none(),
        "no subscription should exist when the first charge fails"
    );
}

#[test]
fn subscribe_fails_on_inactive_plan() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    ctx.approve(AMOUNT * 10, APPROVAL_EXPIRY);
    ctx.client.disable_plan(&ctx.merchant, &plan_id);
    let result = ctx.client.try_subscribe(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::PlanInactive)));
}

#[test]
fn rejects_duplicate_subscription() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    let result = ctx.client.try_subscribe(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::SubscriptionAlreadyExists)));
}

// ── Delegated billing ────────────────────────────────────────────────────────

/// The regression test for this contract's central design claim.
///
/// `process_payment` must succeed when the environment supplies *no*
/// authorization entries at all — that is what "the keeper bills you without
/// your signature" actually means on-chain.
///
/// The original implementation could not pass this. It called
/// `token.transfer(from = subscriber, ..)`, which requires the subscriber's
/// auth, and the failure was masked because every test ran under
/// `env.mock_all_auths()`.
#[test]
fn keeper_can_bill_without_any_authorization() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.warp(INTERVAL + 1, 100);

    // Drop out of auth-mocking into enforcing mode with an empty auth set.
    ctx.env.set_auths(&[]);

    let outcome = ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(outcome, PaymentOutcome::Paid);
    assert_eq!(
        ctx.token_client().balance(&ctx.merchant),
        AMOUNT * 2,
        "signup cycle plus one keeper-driven cycle"
    );
}

#[test]
fn successful_charge_resets_retry_state() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.warp(INTERVAL + 1, 100);

    assert_eq!(
        ctx.client.process_payment(&ctx.subscriber, &plan_id),
        PaymentOutcome::Paid
    );
    let sub = ctx.subscription(plan_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.retries, 0);
    assert_eq!(sub.next_retry_at, 0);
}

#[test]
fn billing_before_the_due_date_is_rejected() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    let result = ctx.client.try_process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::BillingNotDue)));
}

/// A late keeper must not push the billing date forward. Charging at
/// `INTERVAL + 10_000` should still schedule the next cycle at `2 * INTERVAL`,
/// not at `INTERVAL + 10_000 + INTERVAL`.
#[test]
fn billing_schedule_does_not_drift_when_the_keeper_runs_late() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.warp(INTERVAL + 10_000, 100);

    ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(ctx.subscription(plan_id).next_billing_at, INTERVAL * 2);
}

/// If the anchor has fallen so far behind that advancing by one interval would
/// still leave it in the past, it resets to `now` rather than emitting a due
/// date that is instantly overdue.
#[test]
fn billing_anchor_recovers_from_a_long_outage() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    let now = INTERVAL * 5;
    ctx.warp(now, 100);

    ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(ctx.subscription(plan_id).next_billing_at, now + INTERVAL);
}

// ── Failure handling ─────────────────────────────────────────────────────────

#[test]
fn insufficient_balance_moves_subscription_to_grace() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + 1, 100);

    let outcome = ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(outcome, PaymentOutcome::Retrying);
    let sub = ctx.subscription(plan_id);
    assert_eq!(sub.status, SubscriptionStatus::GracePeriod);
    assert_eq!(
        sub.retries, 1,
        "retry bookkeeping must survive the failed charge"
    );
}

#[test]
fn lapsed_allowance_moves_subscription_to_grace() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    // Approve exactly one cycle: signup succeeds, the next charge cannot.
    ctx.approve(AMOUNT, APPROVAL_EXPIRY);
    ctx.client.subscribe(&ctx.subscriber, &plan_id);
    ctx.warp(INTERVAL + 1, 100);

    let outcome = ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(outcome, PaymentOutcome::Retrying);
    assert_eq!(
        ctx.subscription(plan_id).status,
        SubscriptionStatus::GracePeriod
    );
}

/// Allowances expire by ledger sequence. Past the expiration ledger the
/// approval reads as zero and billing stops — the reason "sign once, forever"
/// is not achievable on Stellar.
#[test]
fn allowance_expiry_stops_billing() {
    let ctx = setup();
    let plan_id = register_and_plan(&ctx);
    ctx.approve(AMOUNT * 10, 1_000);
    ctx.client.subscribe(&ctx.subscriber, &plan_id);

    ctx.warp(INTERVAL + 1, 1_001); // past the approval's expiration ledger

    assert_eq!(
        ctx.client.get_billing_allowance(&ctx.subscriber, &plan_id),
        0
    );
    assert_eq!(
        ctx.client.process_payment(&ctx.subscriber, &plan_id),
        PaymentOutcome::Retrying
    );
}

/// A keeper polling every ledger must not burn the whole retry budget at once.
#[test]
fn retries_are_throttled_by_the_retry_interval() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + 1, 100);

    ctx.client.process_payment(&ctx.subscriber, &plan_id);

    // Immediate re-poll: throttled.
    let result = ctx.client.try_process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::BillingNotDue)));
    assert_eq!(ctx.subscription(plan_id).retries, 1);

    // After the retry interval elapses, the attempt is allowed through.
    ctx.warp(INTERVAL + 1 + RETRY_INTERVAL, 101);
    ctx.client.process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(ctx.subscription(plan_id).retries, 2);
}

#[test]
fn exhausting_the_retry_budget_fails_the_subscription() {
    let ctx = setup();
    let plan_id = plan_with(&ctx, 1, RETRY_INTERVAL);
    ctx.approve(AMOUNT * 10, APPROVAL_EXPIRY);
    ctx.client.subscribe(&ctx.subscriber, &plan_id);
    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + 1, 100);

    let outcome = ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(outcome, PaymentOutcome::Failed);
    assert_eq!(ctx.subscription(plan_id).status, SubscriptionStatus::Failed);
}

/// Retry budget alone is not enough to keep a delinquent subscription alive:
/// once the grace window closes it fails regardless of remaining retries.
#[test]
fn exhausting_the_grace_window_fails_the_subscription() {
    let ctx = setup();
    let plan_id = subscribed(&ctx); // retry_limit 3, plenty remaining
    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + GRACE + 1, 100);

    let outcome = ctx.client.process_payment(&ctx.subscriber, &plan_id);

    assert_eq!(outcome, PaymentOutcome::Failed);
    let sub = ctx.subscription(plan_id);
    assert_eq!(sub.status, SubscriptionStatus::Failed);
    assert!(sub.retries < RETRY_LIMIT, "grace, not retries, ended it");
}

#[test]
fn a_failed_subscription_cannot_be_billed_again() {
    let ctx = setup();
    let plan_id = plan_with(&ctx, 1, RETRY_INTERVAL);
    ctx.approve(AMOUNT * 10, APPROVAL_EXPIRY);
    ctx.client.subscribe(&ctx.subscriber, &plan_id);
    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + 1, 100);
    ctx.client.process_payment(&ctx.subscriber, &plan_id);

    let result = ctx.client.try_process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::SubscriptionFailed)));
}

#[test]
fn a_recovered_subscriber_is_charged_and_restored() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + 1, 100);
    ctx.client.process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(
        ctx.subscription(plan_id).status,
        SubscriptionStatus::GracePeriod
    );

    // Subscriber tops up and the next retry window opens.
    StellarAssetClient::new(&ctx.env, &ctx.token).mint(&ctx.subscriber, &(AMOUNT * 2));
    ctx.warp(INTERVAL + 1 + RETRY_INTERVAL, 101);

    assert_eq!(
        ctx.client.process_payment(&ctx.subscriber, &plan_id),
        PaymentOutcome::Paid
    );
    let sub = ctx.subscription(plan_id);
    assert_eq!(sub.status, SubscriptionStatus::Active);
    assert_eq!(sub.retries, 0);
}

// ── Merchant/plan deactivation ───────────────────────────────────────────────

#[test]
fn a_disabled_plan_stops_collecting() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.client.disable_plan(&ctx.merchant, &plan_id);
    ctx.warp(INTERVAL + 1, 100);

    let result = ctx.client.try_process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::PlanInactive)));
}

// ── Pause / resume / cancel ──────────────────────────────────────────────────

#[test]
fn a_paused_subscription_cannot_be_billed() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.client.pause_subscription(&ctx.subscriber, &plan_id);
    ctx.warp(INTERVAL + 1, 100);

    let result = ctx.client.try_process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::SubscriptionNotActive)));
}

/// Regression test for a free-service exploit: pausing just before the due date
/// and resuming just after used to reset the billing anchor to
/// `now + interval`, which a subscriber could repeat forever.
#[test]
fn pause_and_resume_does_not_skip_a_billing_cycle() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    let due_at = ctx.subscription(plan_id).next_billing_at;

    ctx.warp(INTERVAL - 1, 50); // one second before the charge
    ctx.client.pause_subscription(&ctx.subscriber, &plan_id);
    ctx.warp(INTERVAL + 1, 51); // and back, one second after
    ctx.client.resume_subscription(&ctx.subscriber, &plan_id);

    assert_eq!(
        ctx.subscription(plan_id).next_billing_at,
        due_at,
        "the billing anchor must survive a pause/resume round trip"
    );
    assert_eq!(
        ctx.client.process_payment(&ctx.subscriber, &plan_id),
        PaymentOutcome::Paid,
        "the skipped cycle is immediately collectable on resume"
    );
}

#[test]
fn resume_requires_a_paused_subscription() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    let result = ctx
        .client
        .try_resume_subscription(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::NotPaused)));
}

#[test]
fn cancels_subscription_once() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.client.cancel_subscription(&ctx.subscriber, &plan_id);
    assert_eq!(
        ctx.subscription(plan_id).status,
        SubscriptionStatus::Cancelled
    );

    let result = ctx
        .client
        .try_cancel_subscription(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::AlreadyCancelled)));
}

#[test]
fn a_cancelled_subscription_cannot_be_billed() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.client.cancel_subscription(&ctx.subscriber, &plan_id);
    ctx.warp(INTERVAL + 1, 100);

    let result = ctx.client.try_process_payment(&ctx.subscriber, &plan_id);
    assert_eq!(result, Err(Ok(ContractError::AlreadyCancelled)));
}

// ── Queries ──────────────────────────────────────────────────────────────────

#[test]
fn reports_remaining_billing_allowance() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    // Approved 10 cycles, one consumed at signup.
    assert_eq!(
        ctx.client.get_billing_allowance(&ctx.subscriber, &plan_id),
        AMOUNT * 9
    );
}

#[test]
fn is_billable_tracks_due_date_and_retry_throttle() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    assert!(!ctx.client.is_billable(&ctx.subscriber, &plan_id));

    ctx.warp(INTERVAL + 1, 100);
    assert!(ctx.client.is_billable(&ctx.subscriber, &plan_id));

    // Fail the charge to enter the throttled grace state.
    ctx.drain_balance_to(0);
    ctx.client.process_payment(&ctx.subscriber, &plan_id);
    assert!(
        !ctx.client.is_billable(&ctx.subscriber, &plan_id),
        "a throttled retry is not billable yet"
    );

    ctx.warp(INTERVAL + 1 + RETRY_INTERVAL, 101);
    assert!(ctx.client.is_billable(&ctx.subscriber, &plan_id));
}

#[test]
fn is_billable_is_false_for_terminal_states() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    ctx.client.cancel_subscription(&ctx.subscriber, &plan_id);
    ctx.warp(INTERVAL + 1, 100);
    assert!(!ctx.client.is_billable(&ctx.subscriber, &plan_id));
}

#[test]
fn lists_merchant_plans() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    for name in ["Basic", "Pro"] {
        ctx.client.create_plan(
            &ctx.merchant,
            &Symbol::new(&ctx.env, name),
            &AMOUNT,
            &ctx.token,
            &INTERVAL,
            &GRACE,
            &RETRY_LIMIT,
            &RETRY_INTERVAL,
        );
    }
    assert_eq!(ctx.client.get_merchant_plans(&ctx.merchant).len(), 2);
}

#[test]
fn lists_subscriber_plans() {
    let ctx = setup();
    ctx.client
        .register_merchant(&Symbol::new(&ctx.env, "Acme"), &ctx.merchant);
    ctx.approve(AMOUNT * 10, APPROVAL_EXPIRY);
    for name in ["Basic", "Pro"] {
        let plan_id = ctx.client.create_plan(
            &ctx.merchant,
            &Symbol::new(&ctx.env, name),
            &AMOUNT,
            &ctx.token,
            &INTERVAL,
            &GRACE,
            &RETRY_LIMIT,
            &RETRY_INTERVAL,
        );
        ctx.client.subscribe(&ctx.subscriber, &plan_id);
    }
    assert_eq!(ctx.client.get_subscriber_plans(&ctx.subscriber).len(), 2);
}

#[test]
fn records_payment_history() {
    let ctx = setup();
    let plan_id = subscribed(&ctx);
    let signup = ctx.client.get_payment(&1).unwrap();
    assert_eq!(signup.amount, AMOUNT);
    assert!(signup.success);

    ctx.drain_balance_to(0);
    ctx.warp(INTERVAL + 1, 100);
    ctx.client.process_payment(&ctx.subscriber, &plan_id);

    let failure = ctx.client.get_payment(&2).unwrap();
    assert!(
        !failure.success,
        "failed charges must be recorded, not rolled back"
    );
}
