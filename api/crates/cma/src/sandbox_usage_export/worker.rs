use super::*;
use crate::{
    config::UsageExportConfig,
    store::sandbox_usage_export::{
        CaptureStore, CompletenessResult, DeliveryAttempt, DeliveryOutcome,
    },
};
use std::{sync::Arc, time::Duration};

pub(crate) struct UsageExporter {
    store: CaptureStore,
    transport: Arc<dyn UsageTransport>,
    destination: String,
}

impl UsageExporter {
    pub(crate) fn from_config(
        store: &crate::store::MongoSandboxStore,
        config: Option<&UsageExportConfig>,
    ) -> Result<Option<Self>, String> {
        config
            .map(|config| {
                let capture = store
                    .usage_capture_store()
                    .ok_or("usage export requires capture")?;
                Self::new(capture, config)
            })
            .transpose()
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        store: CaptureStore,
        destination: String,
        transport: Arc<dyn UsageTransport>,
    ) -> Self {
        Self {
            store,
            destination,
            transport,
        }
    }
    pub(crate) fn new(store: CaptureStore, config: &UsageExportConfig) -> Result<Self, String> {
        if store.namespace() != config.namespace {
            return Err("usage export namespace differs from capture".into());
        }
        Ok(Self {
            store,
            transport: Arc::new(nyxid::NyxIdUsageTransport::new(config)?),
            destination: config.destination(),
        })
    }

    pub(crate) fn spawn(
        self,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_secs(5));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut after = String::new();
            let mut completeness_after = String::new();
            let mut source_after = String::new();
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = ticks.tick() => {
                        tokio::select! {
                            _ = stop.changed() => break,
                            result = async {
                                let result = self.sweep(&mut after).await;
                                if self.store.completeness_enabled() {
                                    if let Err(error) = self.sweep_completeness_source(&mut source_after).await {
                                        tracing::warn!(%error, "usage completeness source remains pending");
                                    }
                                    if let Err(error) = self.store.rebuild_measured_completeness().await {
                                        tracing::warn!(%error, "usage completeness repair remains pending");
                                    }
                                    if let Err(error) = self.sweep_completeness(&mut completeness_after).await {
                                        tracing::warn!(%error, "usage completeness delivery remains pending");
                                    }
                                }
                                result
                            } => {
                                if result.is_err() { tracing::warn!("usage evidence delivery remains pending"); }
                            }
                        }
                    }
                }
            }
        })
    }

    pub(crate) async fn sweep(
        &self,
        after: &mut String,
    ) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        if self.store.producer_blocked(&self.destination).await? {
            return Ok(());
        }
        let streams = self.store.export_streams(after).await?;
        if streams.is_empty() {
            after.clear();
            return Ok(());
        }
        for stream in streams {
            *after = stream.clone();
            let attempt = match self.store.claim_delivery(&stream, &self.destination).await {
                Ok(Some(attempt)) => attempt,
                Ok(None) => continue,
                Err(_) => {
                    tracing::warn!("usage stream delivery held");
                    continue;
                }
            };
            let result = tokio::time::timeout(Duration::from_secs(60), self.deliver(&attempt))
                .await
                .unwrap_or(Err(TransportError::Unavailable));
            let result = result.and_then(|receipt| {
                receipt.validate(&attempt.event)?;
                if attempt
                    .retained_receipt
                    .as_ref()
                    .is_some_and(|old| old != &receipt)
                {
                    return Err(TransportError::Conflict);
                }
                Ok(receipt)
            });
            let forbidden = matches!(result, Err(TransportError::Forbidden));
            let outcome = match result {
                Ok(receipt) => DeliveryOutcome::Accepted(receipt),
                Err(TransportError::Forbidden) => DeliveryOutcome::ProducerForbidden,
                Err(TransportError::Invalid) => DeliveryOutcome::Hold("invalid_contract"),
                Err(TransportError::Conflict) => DeliveryOutcome::Hold("identity_conflict"),
                Err(error) => {
                    let (reason, delay) = match error {
                        TransportError::Unauthorized => ("producer_unauthorized", None),
                        TransportError::CursorChanged => ("cursor_changed", None),
                        TransportError::Prerequisite => ("prerequisite_missing", None),
                        TransportError::RateLimited(seconds) => ("rate_limited", Some(seconds)),
                        _ => ("outcome_unknown", None),
                    };
                    let seconds = reconcile::retry_seconds(
                        attempt.generation,
                        delay,
                        uuid::Uuid::new_v4().as_bytes()[0],
                    );
                    DeliveryOutcome::Retry { seconds, reason }
                }
            };
            self.store.finish_delivery(&attempt, outcome).await?;
            if forbidden {
                break;
            }
        }
        Ok(())
    }

    async fn sweep_completeness_source(
        &self,
        after: &mut String,
    ) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        let ids = self.store.completeness_source_page(after).await?;
        if ids.is_empty() {
            after.clear();
            return Ok(());
        }
        for id in ids {
            *after = id.clone();
            if self.store.backfill_completeness(&id).await? {
                break;
            }
        }
        Ok(())
    }

    async fn sweep_completeness(
        &self,
        after: &mut String,
    ) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        if self.store.producer_blocked(&self.destination).await? {
            return Ok(());
        }
        let ids = self.store.completeness_delivery_page(after).await?;
        if ids.is_empty() {
            after.clear();
            return Ok(());
        }
        for id in ids {
            *after = id.clone();
            let Some(attempt) = self
                .store
                .claim_completeness(&id, &self.destination)
                .await?
            else {
                continue;
            };
            let result = async {
                let receipt = match self.transport.completeness_receipt(&id).await? {
                    Some(receipt) => receipt,
                    None => self.transport.submit_completeness(&attempt).await?,
                };
                if !receipt.validates(&attempt) {
                    return Err(TransportError::Conflict);
                }
                Ok(receipt)
            }
            .await;
            let outcome = match result {
                Ok(receipt) => CompletenessResult::Accepted(receipt),
                Err(TransportError::Invalid | TransportError::Conflict) => CompletenessResult::Hold,
                Err(error) => CompletenessResult::Retry(reconcile::retry_seconds(
                    attempt.generation,
                    match error {
                        TransportError::RateLimited(seconds) => Some(seconds),
                        _ => None,
                    },
                    uuid::Uuid::new_v4().as_bytes()[0],
                )),
            };
            self.store.finish_completeness(&attempt, outcome).await?;
            // At most one additional HTTP delivery per sweep.
            break;
        }
        Ok(())
    }

    async fn deliver(&self, attempt: &DeliveryAttempt) -> Result<Receipt, TransportError> {
        let event = &attempt.event;
        match self.transport.receipt(&event.event_id).await? {
            Some(receipt) if !attempt.audit => {
                receipt.validate(event)?;
                return Ok(receipt);
            }
            Some(receipt) => {
                receipt.validate(event)?;
                let cursor = self
                    .transport
                    .cursor(&event.stream_id)
                    .await?
                    .ok_or(TransportError::Conflict)?;
                reconcile::inspect_cursor(&self.store, event, &cursor).await?;
                if contract::integer(&cursor.contiguous_sequence)? < event.sequence {
                    return Err(TransportError::Conflict);
                }
                return Ok(receipt);
            }
            None => {}
        }
        if let Some(cursor) = self.transport.cursor(&event.stream_id).await? {
            reconcile::inspect_cursor(&self.store, event, &cursor).await?;
        }
        // Absence and cursor observations never acknowledge an individual event.
        // A concurrent acceptance is recovered by the same event ID and bytes.
        self.transport.submit(event).await
    }
}
