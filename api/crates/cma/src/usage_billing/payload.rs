use super::*;
use layer2_core::sandbox_usage::UsageEvent;

pub(super) const METRICS: [&str; 3] = [
    "cma_sandbox_cpu_millis_seconds",
    "cma_sandbox_memory_mib_seconds",
    "cma_sandbox_disk_gib_seconds",
];

pub(super) fn freeze(
    event: &UsageEvent,
    config: &UsageBillingConfig,
) -> Result<FrozenBatch, &'static str> {
    let subscription = format!("{}:{}", event.owner.as_str(), config.plan_code);
    let mut batch = FrozenBatch {
        version: 1,
        api_url: config.api_url.to_string(),
        plan_code: config.plan_code.clone(),
        owner: event.owner.to_string(),
        external_subscription_id: subscription.clone(),
        events: Vec::new(),
    };
    let elapsed = event
        .ended_at
        .checked_sub(event.started_at)
        .ok_or("invalid_usage_interval")?;
    if event
        .allocation
        .usage(elapsed)
        .map_err(|_| "usage_overflow")?
        != event.quantity
    {
        return Err("usage_quantity_mismatch");
    }
    let mut start = event.started_at;
    // Calendar billing uses UTC day boundaries. Splitting prevents a midnight
    // checkpoint assigning yesterday's resource-time to today's invoice period.
    while start < event.ended_at {
        let end = event.ended_at.min((start / 86_400 + 1) * 86_400);
        let quantity = event
            .allocation
            .usage(end - start)
            .map_err(|_| "usage_overflow")?;
        for (code, amount) in METRICS.into_iter().zip([
            quantity.cpu_millis_seconds,
            quantity.memory_mib_seconds,
            quantity.disk_gib_seconds,
        ]) {
            if amount == 0 {
                continue;
            }
            batch.events.push(json!({
                "transaction_id": format!("{}:{start}:{code}", event.transaction_id()),
                "external_subscription_id": subscription, "code": code, "timestamp": end - 1,
                "properties": { "quantity": amount.to_string(), "sandbox_id": event.sandbox_id.as_str(),
                    "service_code": "cma-sandbox", "interval_started_at": start, "interval_ended_at": end },
            }));
        }
        if batch.events.len() > 1000 {
            return Err("usage_interval_requires_manual_backfill");
        }
        start = end;
    }
    Ok(batch)
}
