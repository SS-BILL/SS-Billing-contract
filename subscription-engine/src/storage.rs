use crate::types::{DataKey, Merchant, PaymentRecord, Subscriber, SubscriptionPlan};
use soroban_sdk::{Address, Env, Vec};

const LEDGER_BUMP: u32 = 535_000; // ~1 year at 5s/ledger

pub fn bump(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, LEDGER_BUMP, LEDGER_BUMP);
}

// Merchant
pub fn save_merchant(env: &Env, m: &Merchant) {
    let key = DataKey::Merchant(m.merchant_id.clone());
    env.storage().persistent().set(&key, m);
    bump(env, &key);
}
pub fn load_merchant(env: &Env, id: &Address) -> Option<Merchant> {
    env.storage()
        .persistent()
        .get(&DataKey::Merchant(id.clone()))
}

// Plan counter
pub fn next_plan_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PlanCounter)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&DataKey::PlanCounter, &id);
    id
}

// Payment counter
pub fn next_payment_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PaymentCounter)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&DataKey::PaymentCounter, &id);
    id
}

// Plan
pub fn save_plan(env: &Env, plan: &SubscriptionPlan) {
    let key = DataKey::Plan(plan.plan_id);
    env.storage().persistent().set(&key, plan);
    bump(env, &key);
}
pub fn load_plan(env: &Env, plan_id: u64) -> Option<SubscriptionPlan> {
    env.storage().persistent().get(&DataKey::Plan(plan_id))
}

// Merchant plan list
pub fn add_merchant_plan(env: &Env, merchant: &Address, plan_id: u64) {
    let key = DataKey::MerchantPlans(merchant.clone());
    let mut plans: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or(Vec::new(env));
    plans.push_back(plan_id);
    env.storage().persistent().set(&key, &plans);
    bump(env, &key);
}
pub fn get_merchant_plans(env: &Env, merchant: &Address) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::MerchantPlans(merchant.clone()))
        .unwrap_or(Vec::new(env))
}

// Subscriber
pub fn save_subscriber(env: &Env, sub: &Subscriber) {
    let key = DataKey::Subscriber(sub.subscriber.clone(), sub.plan_id);
    env.storage().persistent().set(&key, sub);
    bump(env, &key);
}
pub fn load_subscriber(env: &Env, addr: &Address, plan_id: u64) -> Option<Subscriber> {
    env.storage()
        .persistent()
        .get(&DataKey::Subscriber(addr.clone(), plan_id))
}

// Subscriber plan list
pub fn add_subscriber_plan(env: &Env, subscriber: &Address, plan_id: u64) {
    let key = DataKey::SubscriberPlans(subscriber.clone());
    let mut plans: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or(Vec::new(env));
    plans.push_back(plan_id);
    env.storage().persistent().set(&key, &plans);
    bump(env, &key);
}
pub fn get_subscriber_plans(env: &Env, subscriber: &Address) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::SubscriberPlans(subscriber.clone()))
        .unwrap_or(Vec::new(env))
}

// Payment record
pub fn save_payment(env: &Env, record: &PaymentRecord) {
    let key = DataKey::Payment(record.payment_id);
    env.storage().persistent().set(&key, record);
    bump(env, &key);
}
pub fn load_payment(env: &Env, payment_id: u64) -> Option<PaymentRecord> {
    env.storage()
        .persistent()
        .get(&DataKey::Payment(payment_id))
}
