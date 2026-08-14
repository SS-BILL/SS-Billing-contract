use soroban_sdk::{symbol_short, Address, Env};

pub fn merchant_registered(env: &Env, merchant: &Address) {
    env.events()
        .publish((symbol_short!("merch_reg"), merchant.clone()), ());
}

pub fn plan_created(env: &Env, merchant: &Address, plan_id: u64) {
    env.events()
        .publish((symbol_short!("plan_new"), merchant.clone()), plan_id);
}

pub fn subscribed(env: &Env, subscriber: &Address, plan_id: u64, next_billing: u64) {
    env.events().publish(
        (symbol_short!("sub_new"), subscriber.clone()),
        (plan_id, next_billing),
    );
}

pub fn payment_success(env: &Env, subscriber: &Address, amount: i128, timestamp: u64) {
    env.events().publish(
        (symbol_short!("pay_ok"), subscriber.clone()),
        (amount, timestamp),
    );
}

pub fn payment_failed(env: &Env, subscriber: &Address, plan_id: u64, retries: u32) {
    env.events().publish(
        (symbol_short!("pay_fail"), subscriber.clone()),
        (plan_id, retries),
    );
}

pub fn subscription_paused(env: &Env, subscriber: &Address, plan_id: u64) {
    env.events()
        .publish((symbol_short!("sub_pause"), subscriber.clone()), plan_id);
}

pub fn subscription_resumed(env: &Env, subscriber: &Address, plan_id: u64) {
    env.events()
        .publish((symbol_short!("sub_res"), subscriber.clone()), plan_id);
}

pub fn subscription_cancelled(env: &Env, subscriber: &Address, plan_id: u64) {
    env.events()
        .publish((symbol_short!("sub_canc"), subscriber.clone()), plan_id);
}

pub fn retry_attempted(env: &Env, subscriber: &Address, plan_id: u64, attempt: u32) {
    env.events().publish(
        (symbol_short!("retry"), subscriber.clone()),
        (plan_id, attempt),
    );
}
