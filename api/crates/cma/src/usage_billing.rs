//! Retryable export to NyxID's existing billing provider, outside lifecycle writes.
use crate::{
    config::UsageBillingConfig,
    store::{sandbox_usage::billing::Delivery, MongoSandboxStore},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

mod lease;
mod payload;

pub(crate) struct UsageWorkers {
    store: Arc<MongoSandboxStore>,
    exporter: Option<Exporter>,
    usage_exporter: Option<crate::sandbox_usage_export::UsageExporter>,
}
impl UsageWorkers {
    pub(crate) fn new(
        stores: &crate::store::MongoStores,
        config: &crate::config::Config,
    ) -> Result<Self, String> {
        let store = stores.sandbox_model.clone();
        let exporter = config
            .usage_billing
            .clone()
            .map(|config| Exporter::new(store.clone(), config))
            .transpose()?;
        let usage_exporter = crate::sandbox_usage_export::UsageExporter::from_config(
            &store,
            config.usage_export.as_ref(),
        )?;
        Ok(Self {
            store,
            exporter,
            usage_exporter,
        })
    }
    pub(crate) fn spawn(self, shutdown: &mut crate::shutdown::ShutdownCoordinator) {
        if let Some(projection) = self.store.spawn_usage_projection(shutdown.subscribe()) {
            shutdown.push(projection);
        }
        shutdown.push(self.store.spawn_usage_meter(shutdown.subscribe()));
        if let Some(exporter) = self.usage_exporter {
            shutdown.push(exporter.spawn(shutdown.subscribe()));
        }
        if let Some(exporter) = self.exporter {
            shutdown.push(exporter.spawn(shutdown.subscribe()));
        }
    }
}

struct Exporter {
    store: Arc<MongoSandboxStore>,
    config: UsageBillingConfig,
    http: reqwest::Client,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenBatch {
    version: u8,
    api_url: String,
    plan_code: String,
    owner: String,
    external_subscription_id: String,
    events: Vec<Value>,
}

impl Exporter {
    fn new(store: Arc<MongoSandboxStore>, config: UsageBillingConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("CMA-Sandbox-Usage/1")
            .build()
            .map_err(|_| "could not initialize usage billing transport".to_string())?;
        Ok(Self {
            store,
            config,
            http,
        })
    }
    fn spawn(self, mut stop: tokio::sync::watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_secs(15));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = ticks.tick() => {
                        // Cancellation leaves an expiring claim and the frozen request.
                        tokio::select! {
                            _ = stop.changed() => break,
                            _ = self.sweep() => {}
                        }
                    }
                }
            }
        })
    }
    async fn sweep(&self) {
        for _ in 0..30 {
            let delivery = match self.store.claim_usage_delivery(now()).await {
                Ok(Some(delivery)) => delivery,
                Ok(None) => break,
                Err(error) => {
                    tracing::error!(%error, "usage billing claim failed");
                    break;
                }
            };
            let result = self.deliver(&delivery).await;
            if let Err(error) = &result {
                tracing::warn!(transaction_id = %delivery.event.transaction_id(), error, "usage billing delivery deferred");
            }
            if let Err(error) = self
                .store
                .finish_usage_delivery(&delivery, now(), result.err())
                .await
            {
                tracing::error!(%error, "usage billing acknowledgement failed; stable provider IDs will be retried");
            }
        }
    }
    async fn deliver_claimed(
        &self,
        delivery: &Delivery,
        clock: &lease::Clock,
        post_started: &std::sync::atomic::AtomicBool,
    ) -> Result<(), &'static str> {
        let batch: FrozenBatch = match &delivery.payload {
            Some(payload) => {
                serde_json::from_value(payload.clone()).map_err(|_| "invalid_frozen_payload")?
            }
            None => {
                let batch = payload::freeze(&delivery.event, &self.config)?;
                self.store
                    .freeze_usage_delivery(delivery, &json!(batch), clock.now())
                    .await
                    .map_err(|_| "freeze_failed")?;
                batch
            }
        };
        if batch.version != 1 || batch.api_url != self.config.api_url.as_str() {
            return Err("provider_target_changed");
        }
        if batch.owner != delivery.event.owner.as_str() {
            return Err("provider_owner_mismatch");
        }
        if batch.events.is_empty() {
            return Ok(());
        }
        self.verify_subscription(&batch).await?;
        // Verification may take several HTTP round trips. Re-establish the exact
        // claim immediately before sending the immutable idempotent request.
        self.store
            .begin_usage_delivery(delivery, clock.now())
            .await
            .map_err(|_| "billing_claim_lost")?;
        post_started.store(true, std::sync::atomic::Ordering::Relaxed);
        let response = self
            .request(reqwest::Method::POST, &["events", "batch"])?
            .json(&json!({ "events": batch.events }))
            .send()
            .await
            .map_err(|_| "provider_outcome_unknown")?;
        if response.status().is_server_error() {
            return Err("provider_outcome_unknown");
        }
        if !response.status().is_success() {
            return Err("provider_rejected_events");
        }
        Ok(())
    }
    fn request(
        &self,
        method: reqwest::Method,
        path: &[&str],
    ) -> Result<reqwest::RequestBuilder, &'static str> {
        let mut url = self.config.api_url.clone();
        url.path_segments_mut()
            .map_err(|_| "invalid_provider_path")?
            .pop_if_empty()
            .extend(path);
        Ok(self
            .http
            .request(method, url)
            .bearer_auth(self.config.api_key.as_str())
            .header(reqwest::header::ACCEPT, "application/json"))
    }
    async fn read(&self, path: &[&str]) -> Result<Value, &'static str> {
        let mut response = self
            .request(reqwest::Method::GET, path)?
            .send()
            .await
            .map_err(|_| "provider_transport")?;
        if !response.status().is_success() {
            return Err("provider_read_rejected");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "provider_transport")? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err("provider_response_too_large");
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "invalid_provider_response")
    }
    async fn verify_subscription(&self, batch: &FrozenBatch) -> Result<(), &'static str> {
        let value = self
            .read(&["subscriptions", &batch.external_subscription_id])
            .await?;
        let subscription = &value["subscription"];
        if subscription["external_id"] != batch.external_subscription_id
            || subscription["external_customer_id"] != batch.owner
            || subscription["plan_code"] != batch.plan_code
            || subscription["status"] != "active"
            || subscription["billing_time"] != "calendar"
        {
            return Err("subscription_identity_or_status_mismatch");
        }
        let customer = self.read(&["customers", &batch.owner]).await?;
        if customer["customer"]["external_id"] != batch.owner
            || !matches!(
                customer["customer"]["timezone"].as_str(),
                Some("UTC" | "Etc/UTC")
            )
        {
            return Err("customer_identity_or_timezone_mismatch");
        }
        // Verify this subscription's effective plan, including its overrides.
        let charges = subscription["plan"]["charges"]
            .as_array()
            .ok_or("missing_plan_charges")?;
        for code in payload::METRICS {
            let definition = self.read(&["billable_metrics", code]).await?;
            if definition["billable_metric"]["code"] != code
                || definition["billable_metric"]["aggregation_type"] != "sum_agg"
                || definition["billable_metric"]["field_name"] != "quantity"
            {
                return Err("sandbox_metric_definition_mismatch");
            }
            if !charges.iter().any(|charge| {
                (charge["billable_metric"]["code"] == code
                    || charge["billable_metric_code"] == code)
                    && charge["charge_model"] == "standard"
            }) {
                return Err("missing_sandbox_plan_charge");
            }
        }
        Ok(())
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests;
