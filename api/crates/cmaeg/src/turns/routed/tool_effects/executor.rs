use super::{
    gate::{Cancel, Gate},
    model::{Root, State},
    store::Store,
};
use crate::{
    auth::AgentCaller,
    delivery::{
        durable::receipt_for,
        intents::{DeliveryOutcome, ReplySource},
    },
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::{
        effects::{EffectDisposition, EffectExecutor},
        partition::PartitionLease,
    },
    turns::routed::{dispatcher::admission_error, ExecutionError, RoutedToolReplies},
};
use futures_util::future::BoxFuture;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

pub(super) struct Handler(pub Arc<RoutedToolReplies>);
impl Handler {
    async fn run(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        id: &str,
        token: &PartitionLease,
    ) -> Result<EffectDisposition, ExecutionError> {
        let store = Store(self.0.journal.clone());
        let mut root = store
            .read(id, plan, target)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        match &root.state {
            State::Settled { .. } | State::UnknownAbandoned { .. } => {
                return Ok(EffectDisposition::Completed)
            }
            State::Unknown { .. } => return Ok(EffectDisposition::Retained),
            State::Sending => {
                store
                    .transition(&mut root, token, State::Unknown { receipt: None })
                    .await?;
                tracing::warn!(
                    effect_sha256 = super::super::storage::hash(id.as_bytes()),
                    outcome = "unknown",
                    "Abandoned delayed send requires reconciliation"
                );
                return Ok(EffectDisposition::Retained);
            }
            State::Queued => {}
        }
        let payload = store.payload(&root).await?;
        let current = self
            .0
            .authorize_delayed(
                AgentCaller {
                    key_hash: payload.caller,
                },
                &payload.capability,
                plan,
                target,
            )
            .await;
        let current = match current {
            Ok(current) => current,
            Err(ExecutionError::Expired | ExecutionError::AuthorityChanged) => {
                let receipt = receipt_for(DeliveryOutcome::Rejected {
                    reason: "delayed_reply_authority_refused".into(),
                });
                return self.settle(&store, &mut root, token, receipt).await;
            }
            Err(error) => return Err(error),
        };
        store.transition(&mut root, token, State::Sending).await?;
        let gate = Arc::new(Gate {
            tools: self.0.clone(),
            plan: plan.clone(),
            target: target.clone(),
            payload: payload.clone(),
            root: root.clone(),
            token: token.clone(),
            channel_version: current.version,
            deadline: tokio::time::Instant::now() + Duration::from_secs(20),
            cancelled: AtomicBool::new(false),
        });
        let cancel = Cancel(gate.clone());
        let receipt = tokio::time::timeout(
            Duration::from_secs(20),
            self.0.coordinator.reply_from(
                current,
                payload.capability.event,
                payload.message,
                gate,
                ReplySource::Tool,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            receipt_for(DeliveryOutcome::Unknown {
                reason: "delayed_reply_deadline".into(),
            })
        });
        drop(cancel);
        self.settle(&store, &mut root, token, receipt).await
    }
    async fn settle(
        &self,
        store: &Store,
        root: &mut Root,
        token: &PartitionLease,
        receipt: crate::delivery::intents::IntentReceipt,
    ) -> Result<EffectDisposition, ExecutionError> {
        self.0.journal.guard_partition(&Some(token.clone())).await?;
        let stored = self
            .0
            .journal
            .receipts(&root.operation, &root.identity.owner, root.retained_until)
            .retain(&receipt)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let unknown = super::model::unresolved(&stored);
        let next = if unknown {
            State::Unknown {
                receipt: Some(stored),
            }
        } else {
            State::Settled { receipt: stored }
        };
        store.transition(root, token, next).await?;
        metrics::counter!("cmaeg_replica_delayed_reply_total", "outcome" => receipt.delivery.kind()).increment(1);
        if unknown {
            tracing::warn!(
                effect_sha256 = super::super::storage::hash(root.id.as_bytes()),
                "Delayed reply retains unresolved outcome; reconciliation required"
            );
            Ok(EffectDisposition::Retained)
        } else {
            Ok(EffectDisposition::Completed)
        }
    }
    async fn repair(&self, after: Option<&str>) -> Result<Option<String>, ExecutionError> {
        let rows = self.0.journal.effect_roots(after, 32).await?;
        if rows.len() > 32 {
            return Err(ExecutionError::Corrupt);
        }
        let next = (rows.len() == 32).then(|| rows.last().unwrap().0.clone());
        let mut previous = after.map(str::to_owned);
        for (id, value, version) in rows {
            if previous.as_ref().is_some_and(|previous| previous >= &id) {
                return Err(ExecutionError::Corrupt);
            }
            previous = Some(id.clone());
            let mut root: Root =
                serde_json::from_value(value).map_err(|_| ExecutionError::Corrupt)?;
            root.version = version;
            let plan = self
                .0
                .plans
                .load(&root.identity)
                .await
                .map_err(admission_error)?
                .ok_or(ExecutionError::Corrupt)?;
            let target = plan
                .targets
                .iter()
                .find(|t| t.id == root.target)
                .ok_or(ExecutionError::Corrupt)?;
            root.validate(&plan, target)?;
            if root.id != id {
                return Err(ExecutionError::Corrupt);
            }
            if root.active() {
                self.0
                    .journal
                    .enqueue_effect(&plan, target, &root.id)
                    .await?;
            }
        }
        Ok(next)
    }
}
impl EffectExecutor for Handler {
    fn execute<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        work_id: &'a str,
        token: &'a mut PartitionLease,
    ) -> BoxFuture<'a, Result<EffectDisposition, ExecutionError>> {
        Box::pin(self.run(plan, target, work_id, token))
    }
    fn recover<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>, ExecutionError>> {
        Box::pin(self.repair(after))
    }
}
