#![no_std]

mod errors;
mod events;
mod storage;
mod types;

#[cfg(test)]
mod test;

use errors::ContractError;
use soroban_sdk::{
    contract, contractimpl, token, Address, Env, Symbol, Vec,
};
use storage::*;
use types::*;

#[contract]
pub struct SubscriptionEngine;

#[contractimpl]
impl SubscriptionEngine {
    // ── Merchant Functions ────────────────────────────────────────────────────

    /// Register a new merchant. Caller becomes the merchant_id.
    pub fn register_merchant(
        env: Env,
        name: Symbol,
        treasury_wallet: Address,
    ) -> Result<(), ContractError> {
        // The treasury wallet doubles as the merchant's identity, so it must
        // prove control of the address it wants funds routed to.
        treasury_wallet.require_auth();

        if load_merchant(&env, &treasury_wallet).is_some() {
            return Err(ContractError::MerchantAlreadyExists);
        }

        let merchant = Merchant {
            merchant_id: treasury_wallet.clone(),
            name,
            treasury_wallet: treasury_wallet.clone(),
            active: true,
            created_at: env.ledger().timestamp(),
        };
        save_merchant(&env, &merchant);
        events::merchant_registered(&env, &treasury_wallet);
        Ok(())
    }

    /// Update treasury wallet for a merchant.
    pub fn update_treasury(
        env: Env,
        merchant_id: Address,
        new_treasury: Address,
    ) -> Result<(), ContractError> {
        merchant_id.require_auth();
        let mut merchant = load_merchant(&env, &merchant_id)
            .ok_or(ContractError::MerchantNotFound)?;
        merchant.treasury_wallet = new_treasury;
        save_merchant(&env, &merchant);
        Ok(())
    }

    // ── Plan Functions ────────────────────────────────────────────────────────

    /// Create a subscription plan under a merchant.
    pub fn create_plan(
        env: Env,
        merchant_id: Address,
        name: Symbol,
        amount: i128,
        token: Address,
        interval: u64,
        grace_period: u64,
        retry_limit: u32,
        retry_interval: u64,
    ) -> Result<u64, ContractError> {
        merchant_id.require_auth();

        let merchant = load_merchant(&env, &merchant_id)
            .ok_or(ContractError::MerchantNotFound)?;
        if !merchant.active {
            return Err(ContractError::MerchantInactive);
        }
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if interval == 0 {
            return Err(ContractError::InvalidInterval);
        }
        // A zero retry interval would let a keeper drain the retry budget in
        // consecutive ledgers, so require one whenever retries are enabled.
        if retry_limit > 0 && retry_interval == 0 {
            return Err(ContractError::InvalidRetryInterval);
        }

        let plan_id = next_plan_id(&env);
        let plan = SubscriptionPlan {
            plan_id,
            merchant_id: merchant_id.clone(),
            name,
            amount,
            token,
            interval,
            grace_period,
            retry_limit,
            retry_interval,
            active: true,
        };
        save_plan(&env, &plan);
        add_merchant_plan(&env, &merchant_id, plan_id);
        events::plan_created(&env, &merchant_id, plan_id);
        Ok(plan_id)
    }

    /// Update mutable plan fields (amount, interval, grace_period, retry_limit).
    pub fn update_plan(
        env: Env,
        merchant_id: Address,
        plan_id: u64,
        amount: i128,
        interval: u64,
        grace_period: u64,
        retry_limit: u32,
        retry_interval: u64,
    ) -> Result<(), ContractError> {
        merchant_id.require_auth();
        let mut plan = load_plan(&env, plan_id).ok_or(ContractError::PlanNotFound)?;
        if plan.merchant_id != merchant_id {
            return Err(ContractError::Unauthorized);
        }
        if amount <= 0 {
            return Err(ContractError::InvalidAmount);
        }
        if interval == 0 {
            return Err(ContractError::InvalidInterval);
        }
        if retry_limit > 0 && retry_interval == 0 {
            return Err(ContractError::InvalidRetryInterval);
        }
        plan.amount = amount;
        plan.interval = interval;
        plan.grace_period = grace_period;
        plan.retry_limit = retry_limit;
        plan.retry_interval = retry_interval;
        save_plan(&env, &plan);
        Ok(())
    }

    /// Disable a plan (no new subscriptions, existing ones continue until cancelled).
    pub fn disable_plan(
        env: Env,
        merchant_id: Address,
        plan_id: u64,
    ) -> Result<(), ContractError> {
        merchant_id.require_auth();
        let mut plan = load_plan(&env, plan_id).ok_or(ContractError::PlanNotFound)?;
        if plan.merchant_id != merchant_id {
            return Err(ContractError::Unauthorized);
        }
        plan.active = false;
        save_plan(&env, &plan);
        Ok(())
    }

    // ── Subscription Functions ────────────────────────────────────────────────

    /// Subscribe to a plan and pay the first cycle immediately.
    ///
    /// Before calling this, the subscriber must grant the contract a token
    /// allowance:
    ///
    /// ```text
    /// token.approve(subscriber, <this contract>, total_budget, expiration_ledger)
    /// ```
    ///
    /// That approval is what lets later cycles be charged without the
    /// subscriber signing again. Note that Stellar allowances carry an
    /// expiration ledger, so the authorization is "sign once per approval
    /// window", not "sign once, forever" — the subscriber must re-approve
    /// before the window lapses or billing will halt.
    pub fn subscribe(
        env: Env,
        subscriber: Address,
        plan_id: u64,
    ) -> Result<(), ContractError> {
        subscriber.require_auth();

        let plan = load_plan(&env, plan_id).ok_or(ContractError::PlanNotFound)?;
        if !plan.active {
            return Err(ContractError::PlanInactive);
        }

        let merchant = load_merchant(&env, &plan.merchant_id)
            .ok_or(ContractError::MerchantNotFound)?;
        if !merchant.active {
            return Err(ContractError::MerchantInactive);
        }

        if load_subscriber(&env, &subscriber, plan_id).is_some() {
            return Err(ContractError::SubscriptionAlreadyExists);
        }

        let now = env.ledger().timestamp();
        // Charge the first cycle immediately. This also proves the allowance
        // is in place, so we never create a subscription that cannot be billed.
        Self::_charge(&env, &subscriber, &merchant.treasury_wallet, &plan.token, plan.amount)?;

        let record_id = next_payment_id(&env);
        save_payment(&env, &PaymentRecord {
            payment_id: record_id,
            subscriber: subscriber.clone(),
            merchant: plan.merchant_id.clone(),
            amount: plan.amount,
            timestamp: now,
            success: true,
        });

        let next_billing_at = now.saturating_add(plan.interval);
        let sub = Subscriber {
            subscriber: subscriber.clone(),
            plan_id,
            next_billing_at,
            next_retry_at: 0,
            status: SubscriptionStatus::Active,
            retries: 0,
            started_at: now,
        };
        save_subscriber(&env, &sub);
        add_subscriber_plan(&env, &subscriber, plan_id);
        events::subscribed(&env, &subscriber, plan_id, next_billing_at);
        events::payment_success(&env, &subscriber, plan.amount, now);
        Ok(())
    }

    /// Process a recurring billing cycle.
    ///
    /// Callable by anyone — a keeper, the merchant, or the subscriber — because
    /// the funds move under the allowance the subscriber granted at signup, not
    /// under the caller's authority. No signature from the subscriber is needed
    /// or accepted here.
    ///
    /// A failed charge is reported as `Ok(PaymentOutcome::Retrying | Failed)`,
    /// never as `Err`. Returning `Err` would roll back the invocation, discarding
    /// the very retry bookkeeping the failure path exists to record.
    pub fn process_payment(
        env: Env,
        subscriber: Address,
        plan_id: u64,
    ) -> Result<PaymentOutcome, ContractError> {
        let mut sub = load_subscriber(&env, &subscriber, plan_id)
            .ok_or(ContractError::SubscriptionNotFound)?;

        match sub.status {
            SubscriptionStatus::Cancelled => return Err(ContractError::AlreadyCancelled),
            SubscriptionStatus::Paused => return Err(ContractError::SubscriptionNotActive),
            SubscriptionStatus::Failed => return Err(ContractError::SubscriptionFailed),
            SubscriptionStatus::Active | SubscriptionStatus::GracePeriod => {}
        }

        let now = env.ledger().timestamp();
        let plan = load_plan(&env, plan_id).ok_or(ContractError::PlanNotFound)?;
        let merchant = load_merchant(&env, &plan.merchant_id)
            .ok_or(ContractError::MerchantNotFound)?;

        // A disabled plan or deactivated merchant stops collecting. Existing
        // subscriptions are left untouched so they can be cancelled cleanly.
        if !plan.active {
            return Err(ContractError::PlanInactive);
        }
        if !merchant.active {
            return Err(ContractError::MerchantInactive);
        }

        if now < sub.next_billing_at {
            return Err(ContractError::BillingNotDue);
        }
        // While retrying, throttle to the plan's retry interval so a keeper
        // polling every ledger cannot exhaust the retry budget instantly.
        if sub.status == SubscriptionStatus::GracePeriod && now < sub.next_retry_at {
            return Err(ContractError::BillingNotDue);
        }

        let grace_deadline = sub.next_billing_at.saturating_add(plan.grace_period);

        match Self::_charge(
            &env,
            &subscriber,
            &merchant.treasury_wallet,
            &plan.token,
            plan.amount,
        ) {
            Ok(()) => {
                let record_id = next_payment_id(&env);
                save_payment(&env, &PaymentRecord {
                    payment_id: record_id,
                    subscriber: subscriber.clone(),
                    merchant: plan.merchant_id.clone(),
                    amount: plan.amount,
                    timestamp: now,
                    success: true,
                });

                // Advance from the previous anchor, not from `now`, so a keeper
                // that runs late does not permanently shift the billing date.
                // If the anchor has fallen too far behind to catch up, reset it
                // to `now` rather than emitting a due date in the past.
                let anchored = sub.next_billing_at.saturating_add(plan.interval);
                sub.next_billing_at = if anchored <= now {
                    now.saturating_add(plan.interval)
                } else {
                    anchored
                };
                sub.next_retry_at = 0;
                sub.retries = 0;
                sub.status = SubscriptionStatus::Active;
                save_subscriber(&env, &sub);
                events::payment_success(&env, &subscriber, plan.amount, now);
                Ok(PaymentOutcome::Paid)
            }
            Err(_) => {
                sub.retries = sub.retries.saturating_add(1);
                events::retry_attempted(&env, &subscriber, plan_id, sub.retries);

                let record_id = next_payment_id(&env);
                save_payment(&env, &PaymentRecord {
                    payment_id: record_id,
                    subscriber: subscriber.clone(),
                    merchant: plan.merchant_id.clone(),
                    amount: plan.amount,
                    timestamp: now,
                    success: false,
                });

                // The subscription dies when either budget runs out: the retry
                // count or the grace window. Checking only retries would let a
                // long retry_interval keep a delinquent subscription alive
                // indefinitely past its grace deadline.
                let retries_exhausted = sub.retries >= plan.retry_limit;
                let grace_exhausted = now >= grace_deadline;

                if retries_exhausted || grace_exhausted {
                    sub.status = SubscriptionStatus::Failed;
                    sub.next_retry_at = 0;
                    save_subscriber(&env, &sub);
                    events::payment_failed(&env, &subscriber, plan_id, sub.retries);
                    Ok(PaymentOutcome::Failed)
                } else {
                    sub.status = SubscriptionStatus::GracePeriod;
                    sub.next_retry_at = now.saturating_add(plan.retry_interval);
                    save_subscriber(&env, &sub);
                    events::payment_failed(&env, &subscriber, plan_id, sub.retries);
                    Ok(PaymentOutcome::Retrying)
                }
            }
        }
    }

    /// Pause an active subscription.
    pub fn pause_subscription(
        env: Env,
        subscriber: Address,
        plan_id: u64,
    ) -> Result<(), ContractError> {
        subscriber.require_auth();
        let mut sub = load_subscriber(&env, &subscriber, plan_id)
            .ok_or(ContractError::SubscriptionNotFound)?;
        if sub.status == SubscriptionStatus::Paused {
            return Err(ContractError::AlreadyPaused);
        }
        if sub.status == SubscriptionStatus::Cancelled {
            return Err(ContractError::AlreadyCancelled);
        }
        sub.status = SubscriptionStatus::Paused;
        save_subscriber(&env, &sub);
        events::subscription_paused(&env, &subscriber, plan_id);
        Ok(())
    }

    /// Resume a paused subscription.
    pub fn resume_subscription(
        env: Env,
        subscriber: Address,
        plan_id: u64,
    ) -> Result<(), ContractError> {
        subscriber.require_auth();
        let mut sub = load_subscriber(&env, &subscriber, plan_id)
            .ok_or(ContractError::SubscriptionNotFound)?;
        if sub.status != SubscriptionStatus::Paused {
            return Err(ContractError::NotPaused);
        }
        // Reset next billing to now + interval on resume
        sub.next_billing_at = env.ledger().timestamp() + load_plan(&env, plan_id)
            .ok_or(ContractError::PlanNotFound)?.interval;
        sub.status = SubscriptionStatus::Active;
        save_subscriber(&env, &sub);
        events::subscription_resumed(&env, &subscriber, plan_id);
        Ok(())
    }

    /// Cancel a subscription permanently.
    pub fn cancel_subscription(
        env: Env,
        subscriber: Address,
        plan_id: u64,
    ) -> Result<(), ContractError> {
        subscriber.require_auth();
        let mut sub = load_subscriber(&env, &subscriber, plan_id)
            .ok_or(ContractError::SubscriptionNotFound)?;
        if sub.status == SubscriptionStatus::Cancelled {
            return Err(ContractError::AlreadyCancelled);
        }
        sub.status = SubscriptionStatus::Cancelled;
        save_subscriber(&env, &sub);
        events::subscription_cancelled(&env, &subscriber, plan_id);
        Ok(())
    }

    // ── Query Functions ───────────────────────────────────────────────────────

    pub fn get_merchant(env: Env, merchant_id: Address) -> Option<Merchant> {
        load_merchant(&env, &merchant_id)
    }

    pub fn get_plan(env: Env, plan_id: u64) -> Option<SubscriptionPlan> {
        load_plan(&env, plan_id)
    }

    pub fn get_subscriber(env: Env, subscriber: Address, plan_id: u64) -> Option<Subscriber> {
        load_subscriber(&env, &subscriber, plan_id)
    }

    pub fn get_payment(env: Env, payment_id: u64) -> Option<PaymentRecord> {
        load_payment(&env, payment_id)
    }

    pub fn get_merchant_plans(env: Env, merchant_id: Address) -> Vec<u64> {
        storage::get_merchant_plans(&env, &merchant_id)
    }

    pub fn get_subscriber_plans(env: Env, subscriber: Address) -> Vec<u64> {
        storage::get_subscriber_plans(&env, &subscriber)
    }

    // ── Internal Helpers ──────────────────────────────────────────────────────

    /// Move `amount` from the subscriber to the merchant treasury using the
    /// allowance the subscriber granted this contract.
    ///
    /// `transfer_from` requires auth from the *spender*, which is this contract.
    /// A contract's own address is authorized automatically for sub-invocations
    /// it makes directly, so no signature is needed at call time — that is what
    /// makes keeper-driven billing possible.
    ///
    /// The previous implementation called `transfer(from = subscriber, ..)`,
    /// which requires auth from the subscriber and therefore could only ever
    /// succeed inside a transaction the subscriber personally signed. That
    /// contradicted the entire delegated-billing design.
    ///
    /// Both preconditions are checked before dispatching, because a trapped
    /// token call would abort the whole invocation and lose the retry
    /// bookkeeping the caller needs to persist.
    fn _charge(
        env: &Env,
        from: &Address,
        to: &Address,
        token: &Address,
        amount: i128,
    ) -> Result<(), ContractError> {
        let client = token::Client::new(env, token);
        let spender = env.current_contract_address();

        if client.balance(from) < amount {
            return Err(ContractError::InsufficientBalance);
        }
        // Returns 0 once the approval's expiration ledger has passed.
        if client.allowance(from, &spender) < amount {
            return Err(ContractError::InsufficientAllowance);
        }

        client.transfer_from(&spender, from, to, &amount);
        Ok(())
    }
}
