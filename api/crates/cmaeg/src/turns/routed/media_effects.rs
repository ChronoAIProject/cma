//! Continuations use a distinct work identity in the original conversation FIFO.
use super::{media_work, ExecutionError, MediaWorker};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::{
        effects::{EffectDisposition, EffectExecutor},
        partition::PartitionLease,
    },
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
const KIND: &str = "replica_media_completion_v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completed {
    kind: String,
    root: String,
    target: String,
    work: String,
    request_hash: String,
    retained_until: i64,
}
impl Completed {
    fn expected(plan: &AdmittedPlan, target: &StoredTarget) -> Self {
        Self {
            kind: KIND.into(),
            root: plan.admission_id.clone(),
            target: target.id.clone(),
            work: media_work::id(target),
            request_hash: plan.request_hash.clone(),
            retained_until: plan.retained_until,
        }
    }
    fn id(target: &StoredTarget) -> String {
        super::storage::identifier(&(KIND, &target.id))
    }
}
impl MediaWorker {
    pub(super) async fn completed_media(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<bool, ExecutionError> {
        let Some((value, version)) = self
            .store
            .0
            .read::<Completed>(&Completed::id(target))
            .await?
        else {
            return Ok(false);
        };
        if version != 1
            || super::storage::bytes(&value)?
                != super::storage::bytes(&Completed::expected(plan, target))?
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(true)
    }
    pub(super) async fn complete_media(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        token: &PartitionLease,
    ) -> Result<(), ExecutionError> {
        self.store
            .0
            .write_fenced(
                &Some(token.clone()),
                &Completed::id(target),
                &plan.identity.owner,
                plan.retained_until,
                &Completed::expected(plan, target),
                None,
            )
            .await?;
        Ok(())
    }
    pub(super) async fn admit_media(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), ExecutionError> {
        let Some(head) = self.index.load(plan, target).await? else {
            return Ok(());
        };
        if self.completed_media(plan, target).await? {
            self.index.finish(&head).await?;
        } else {
            self.store
                .0
                .enqueue_effect(plan, target, &media_work::id(target))
                .await?;
        }
        Ok(())
    }
}
impl super::RoutedToolReplies {
    pub fn replica_effect_executor_with_media(
        self: &Arc<Self>,
        media: Arc<dyn EffectExecutor>,
    ) -> Arc<dyn EffectExecutor> {
        Arc::new(Handler {
            media,
            other: self.replica_effect_executor(),
        })
    }
}
struct Handler {
    media: Arc<dyn EffectExecutor>,
    other: Arc<dyn EffectExecutor>,
}
impl EffectExecutor for Handler {
    fn execute<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        work_id: &'a str,
        token: &'a mut PartitionLease,
    ) -> BoxFuture<'a, Result<EffectDisposition, ExecutionError>> {
        if work_id == media_work::id(target) {
            self.media.execute(plan, target, work_id, token)
        } else {
            self.other.execute(plan, target, work_id, token)
        }
    }
    fn recover<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>, ExecutionError>> {
        self.other.recover(after)
    }
}
impl EffectExecutor for MediaWorker {
    fn execute<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        work_id: &'a str,
        token: &'a mut PartitionLease,
    ) -> BoxFuture<'a, Result<EffectDisposition, ExecutionError>> {
        Box::pin(async move {
            if work_id != media_work::id(target) {
                return Err(ExecutionError::Invalid);
            }
            let mut captured = Some(token.clone());
            let result = self.run_owned(plan, target, &mut captured).await;
            *token = captured.ok_or(ExecutionError::LostLease)?;
            match result {
                Ok(true) => Ok(EffectDisposition::Completed),
                Ok(false) => Ok(EffectDisposition::Retry),
                Err(ExecutionError::Uncertain) => {
                    tracing::warn!(
                        effect_sha256 = super::storage::hash(work_id.as_bytes()),
                        "Media effect remains at FIFO front pending reconciliation"
                    );
                    Ok(EffectDisposition::Retained)
                }
                Err(error) => Err(error),
            }
        })
    }
    fn recover<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>, ExecutionError>> {
        // The bounded MediaWorkIndex scheduler repairs media admission. This
        // cursor belongs only to the ordinary indexed tool-effect root stream.
        Box::pin(async move {
            if after.is_some() {
                return Err(ExecutionError::Invalid);
            }
            Ok(None)
        })
    }
}

#[cfg(test)]
#[path = "media_replica_tests.rs"]
mod tests;
