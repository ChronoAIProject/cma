//! Fresh lifecycle operations check majority control before the original
//! single-document insert. Recovery never repeats this admission check.

use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::SandboxStoreError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LifecycleCreateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("released runtime changed before lifecycle admission")]
    StaleReleasedRuntime,
}

impl MongoAgentLifecycleStore {
    pub(super) async fn finish_create(
        &self,
        mut record: LifecycleRecord,
        release: Option<(
            &layer3_adapter::ReleasedRuntime,
            layer2_core::released_runtime::ReleasedLane,
        )>,
    ) -> Result<CreateResult, LifecycleCreateError> {
        match super::super::admission_checks::require_fresh_read(&self.admission_settings).await {
            Ok(()) => {}
            Err(SandboxStoreError::AdmissionPaused) => {
                // A concurrent exact writer can commit after the outer replay
                // lookup but before the stop. Its operation keeps authority.
                return self
                    .replay(
                        &SubjectId::new(&record.owner),
                        &record.key,
                        &record.input.request,
                    )
                    .await?
                    .map(|existing| CreateResult::Existing(Box::new(existing)))
                    .ok_or(LifecycleCreateError::AdmissionPaused);
            }
            Err(error) => return Err(StoreError::Unavailable(error.to_string()).into()),
        }
        let inserted = if let (Some(store), Some((snapshot, lane))) =
            (&self.released_lanes, release)
        {
            let image = match lane {
                layer2_core::released_runtime::ReleasedLane::Agent => Some(snapshot.agent.as_str()),
                layer2_core::released_runtime::ReleasedLane::Machine => snapshot.machine.as_deref(),
            };
            if released::managed_lifecycle_release_lane(&record.input.request) != Some(lane)
                || image.is_none()
                || record.input.resolved_image.as_deref() != image
            {
                return Err(StoreError::Malformed(
                    "lifecycle image differs from its released runtime admission".into(),
                )
                .into());
            }
            record.released_lane = Some(lane);
            let mut session = store
                .session()
                .await
                .map_err(|error| StoreError::Unavailable(error.to_string()))?;
            match store.fence_snapshot(snapshot, lane, &mut session).await {
                Ok(()) => {}
                Err(SandboxStoreError::Conflict) => {
                    return Err(LifecycleCreateError::StaleReleasedRuntime)
                }
                Err(error) => return Err(StoreError::Unavailable(error.to_string()).into()),
            }
            match self
                .operations
                .insert_one(&record)
                .session(&mut session)
                .await
            {
                Ok(_) => session.commit_guarded().await,
                Err(error) => Err(error),
            }
        } else {
            self.operations.insert_one(&record).await.map(|_| ())
        };
        match inserted {
            Ok(_) => Ok(CreateResult::Created(record.id)),
            Err(error)
                if is_duplicate_key(&error)
                    || error.contains_label("UnknownTransactionCommitResult") =>
            {
                self.replay(
                    &SubjectId::new(&record.owner),
                    &record.key,
                    &record.input.request,
                )
                .await?
                .map(|existing| CreateResult::Existing(Box::new(existing)))
                .ok_or_else(|| {
                    StoreError::Unavailable("lifecycle operation raced out of view".into()).into()
                })
            }
            Err(error) if error.contains_label("TransientTransactionError") => {
                Err(LifecycleCreateError::StaleReleasedRuntime)
            }
            Err(error) => Err(driver(error).into()),
        }
    }
}
