use super::super::tool_reply_store::{Admission, ReplyStore};
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
use std::{sync::Arc, time::Duration};

fn is_no_send(outcome: &DeliveryOutcome) -> bool {
    matches!(
        outcome,
        DeliveryOutcome::Unknown { reason }
            if reason == "delayed_reply_guard_uncertain"
                || reason == "delayed_reply_guard_wait_expired"
    )
}

pub(super) struct Handler(pub Arc<RoutedToolReplies>);
impl Handler {
    async fn run(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        id: &str,
        token: &mut PartitionLease,
    ) -> Result<EffectDisposition, ExecutionError> {
        let store = Store(self.0.journal.clone());
        let mut root = store
            .read(id, plan, target)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        if matches!(root.state, State::Unknown { .. } | State::Sending)
            && self.0.journal.quarantined(plan, target).await?
        {
            if matches!(root.state, State::Sending) {
                store
                    .transition(&mut root, token, State::Unknown { receipt: None })
                    .await?;
            }
            self.0
                .journal
                .write_effect_fenced(
                    &Some(token.clone()),
                    &root.id,
                    &root.identity.owner,
                    root.retained_until,
                    &root,
                    Some(root.version),
                    false,
                )
                .await?;
            return Ok(EffectDisposition::Completed);
        }
        match &root.state {
            State::Settled { .. } => return Ok(EffectDisposition::Completed),
            State::Unknown { receipt } => {
                // Old FIFO workers could save uncertainty without using the
                // shared guard. Publish their actual evidence before returning.
                // A no-send guard refusal has no IO evidence to add to a peer.
                let no_send = receipt.as_ref().is_some_and(|r| is_no_send(&r.delivery));
                if !no_send {
                    ReplyStore(self.0.journal.clone(), Some(token.clone()))
                        .recover_delayed(
                            plan,
                            target,
                            &root.operation,
                            &root.content_hash,
                            receipt.as_ref(),
                        )
                        .await?;
                }
                return Ok(if token.is_lane() || no_send {
                    EffectDisposition::Completed
                } else {
                    EffectDisposition::Retained
                });
            }
            State::Sending => {
                let receipt = ReplyStore(self.0.journal.clone(), Some(token.clone()))
                    .recover_delayed(plan, target, &root.operation, &root.content_hash, None)
                    .await?;
                return self.settle(&store, &mut root, token, receipt).await;
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
                root.answer.as_ref(),
            )
            .await;
        let current = match current {
            Ok(current) => current,
            Err(
                error @ (ExecutionError::Expired
                | ExecutionError::AuthorityChanged
                | ExecutionError::Abandoned),
            ) if root.answer.is_some() => {
                let reason = match error {
                    ExecutionError::Expired => "accepted_answer_target_expired",
                    ExecutionError::AuthorityChanged => "delayed_reply_authority_refused",
                    _ => "unknown_abandoned",
                };
                return self
                    .refuse_accepted_answer(&store, &mut root, (plan, target), token, reason)
                    .await;
            }
            Err(
                ExecutionError::Expired
                | ExecutionError::AuthorityChanged
                | ExecutionError::Abandoned,
            ) => {
                let receipt = receipt_for(DeliveryOutcome::Rejected {
                    reason: if self.0.journal.quarantined(plan, target).await? {
                        "unknown_abandoned"
                    } else {
                        "delayed_reply_authority_refused"
                    }
                    .into(),
                });
                return self.settle(&store, &mut root, token, receipt).await;
            }
            Err(error) => return Err(error),
        };
        let replies = ReplyStore(self.0.journal.clone(), Some(token.clone()));
        let admission = match &root.answer {
            Some(answer) => {
                replies
                    .admit_delayed_answer(
                        plan,
                        target,
                        root.operation.clone(),
                        root.content_hash.clone(),
                        answer,
                    )
                    .await
            }
            None => {
                replies
                    .admit_delayed(
                        plan,
                        target,
                        root.operation.clone(),
                        root.content_hash.clone(),
                    )
                    .await
            }
        };
        let claim = match admission {
            Ok(Admission::Claimed(claim)) => claim,
            Ok(Admission::Pending) => {
                if self
                    .0
                    .journal
                    .partitions
                    .as_ref()
                    .ok_or(ExecutionError::Unsupported)?
                    .delayed_pending_expired(token, root.retained_until)
                    .await?
                {
                    return self
                        .settle(
                            &store,
                            &mut root,
                            token,
                            receipt_for(DeliveryOutcome::Unknown {
                                reason: "delayed_reply_guard_wait_expired".into(),
                            }),
                        )
                        .await;
                }
                return Ok(EffectDisposition::Retry);
            }
            Err(ExecutionError::Busy) => return Ok(EffectDisposition::Retry),
            Ok(Admission::Replay(receipt)) => {
                return self.settle(&store, &mut root, token, receipt).await
            }
            Ok(Admission::Unknown) => {
                return self
                    .settle(
                        &store,
                        &mut root,
                        token,
                        receipt_for(DeliveryOutcome::Unknown {
                            reason: "delayed_reply_guard_uncertain".into(),
                        }),
                    )
                    .await
            }
            Err(ExecutionError::OperationLimit) => {
                if root.answer.is_some() {
                    return self
                        .refuse_accepted_answer(
                            &store,
                            &mut root,
                            (plan, target),
                            token,
                            "reply_operation_limit",
                        )
                        .await;
                }
                return self
                    .settle(
                        &store,
                        &mut root,
                        token,
                        receipt_for(DeliveryOutcome::Rejected {
                            reason: "reply_operation_limit".into(),
                        }),
                    )
                    .await;
            }
            Err(error) => return Err(error),
            Ok(_) => return Err(ExecutionError::Corrupt),
        };
        store.transition(&mut root, token, State::Sending).await?;
        let scope = super::super::live_scope::Scope::new(
            self.0.journal.clone(),
            super::super::live_scope::Capture::reply(*claim),
        );
        let gate = Arc::new(Gate {
            tools: self.0.clone(),
            plan: plan.clone(),
            target: target.clone(),
            payload: payload.clone(),
            root: root.clone(),
            scope: scope.clone(),
            channel_version: current.version,
            deadline: tokio::time::Instant::now() + Duration::from_secs(20),
            boundary: Default::default(),
        });
        let cancel = Cancel(gate.clone());
        let receipt = scope
            .run(
                "delayed_reply",
                tokio::time::timeout(
                    Duration::from_secs(20),
                    self.0.coordinator.reply_from(
                        current,
                        payload.capability.event,
                        payload.message,
                        gate.clone(),
                        ReplySource::Tool,
                    ),
                ),
            )
            .await?;
        let claim = scope.reply();
        *token = claim.partition.clone().ok_or(ExecutionError::LostLease)?;
        let receipt = receipt.unwrap_or_else(|_| {
            receipt_for(DeliveryOutcome::Unknown {
                reason: "delayed_reply_deadline".into(),
            })
        });
        let dispatch = gate.boundary.close();
        drop(cancel);
        super::super::live_scope::receipt(async {
            replies.finish_observed(&claim, &receipt, dispatch).await?;
            self.settle(&store, &mut root, token, receipt).await
        })
        .await
    }
    async fn refuse_accepted_answer(
        &self,
        store: &Store,
        root: &mut Root,
        (plan, target): (&AdmittedPlan, &StoredTarget),
        token: &PartitionLease,
        reason: &str,
    ) -> Result<EffectDisposition, ExecutionError> {
        let answer = root.answer.as_ref().ok_or(ExecutionError::Corrupt)?;
        let replies = ReplyStore(self.0.journal.clone(), Some(token.clone()));
        let receipt = match replies
            .refuse_delayed_answer(
                plan,
                target,
                &root.operation,
                &root.content_hash,
                answer,
                reason,
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(ExecutionError::Busy) => return Ok(EffectDisposition::Retry),
            Err(error) => return Err(error),
        };
        self.settle(store, root, token, receipt).await
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
        let no_send = is_no_send(&stored.delivery);
        let unknown = stored.requires_reconciliation();
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
            Ok(if token.is_lane() || no_send {
                EffectDisposition::Completed
            } else {
                EffectDisposition::Retained
            })
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
                match self
                    .0
                    .journal
                    .ensure_delayed_custody(&plan, target, &root.id)
                    .await
                {
                    // Expired root-only custody cannot be recreated and cannot
                    // keep the bounded repair cursor from reaching later work.
                    Ok(()) | Err(ExecutionError::Expired) => {}
                    Err(error) => return Err(error),
                }
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
