use soroban_sdk::{contracttype, Address, Symbol};

// ── Enums ────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SubscriptionStatus {
    Active,
    Paused,
    Cancelled,
    GracePeriod,
    Failed,
}

/// Result of a billing attempt.
///
/// `process_payment` reports failure through this value rather than through
/// `Err`, because returning `Err` from a Soroban contract rolls back every
/// storage write made during the invocation — including the retry bookkeeping
/// we specifically need to survive a failed charge.
#[contracttype]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PaymentOutcome {
    /// Funds moved; the subscription advanced to its next cycle.
    Paid,
    /// Charge failed, retry budget remains. Subscription is in GracePeriod.
    Retrying,
    /// Charge failed and the retry budget or grace window is exhausted.
    Failed,
}

// ── Core Structs ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct Merchant {
    pub merchant_id: Address,
    pub name: Symbol,
    pub treasury_wallet: Address,
    pub active: bool,
    pub created_at: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SubscriptionPlan {
    pub plan_id: u64,
    pub merchant_id: Address,
    pub name: Symbol,
    pub amount: i128,
    pub token: Address,
    pub interval: u64, // seconds between billing cycles
    pub grace_period: u64,
    pub retry_limit: u32,
    /// Seconds to wait between retry attempts after a failed charge. Without
    /// this, a keeper polling every minute would burn the entire retry budget
    /// in minutes instead of spreading it across the grace window.
    pub retry_interval: u64,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Subscriber {
    pub subscriber: Address,
    pub plan_id: u64,
    /// Anchor for the billing cycle. Advances by exactly one `interval` per
    /// successful charge so a late keeper cannot make the schedule drift.
    pub next_billing_at: u64,
    /// Earliest timestamp at which a failed charge may be retried. Only
    /// meaningful while status is GracePeriod.
    pub next_retry_at: u64,
    pub status: SubscriptionStatus,
    pub retries: u32,
    pub started_at: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PaymentRecord {
    pub payment_id: u64,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub timestamp: u64,
    pub success: bool,
}

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Merchant(Address),
    Plan(u64),
    Subscriber(Address, u64),
    Payment(u64),
    MerchantPlans(Address),
    SubscriberPlans(Address),
    PlanCounter,
    PaymentCounter,
}
