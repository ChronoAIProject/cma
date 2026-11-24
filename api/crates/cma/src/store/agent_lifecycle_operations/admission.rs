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
    #[cfg(test)]
    pub(crate) async fn create(
        &self,
        owner: &SubjectId,
        key: &str,
        input: LifecycleInput,
    ) -> Result<CreateResult, LifecycleCreateError> {
        self.create_with_submission(owner, key, input, None, None)
            .await
    }

    pub(crate) fn supports_creation(
        &self,
        fence: &layer2_core::sandbox::store::CreationFence,
    ) -> bool {
        self.creation_writer_epoch.as_deref() == Some(fence.epoch.as_str())
    }

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
        if record
            .creation_fence
            .as_ref()
            .is_some_and(|fence| !self.supports_creation(fence))
        {
            return Err(
                StoreError::Unavailable("creation submission epoch is unsupported".into()).into(),
            );
        }
        let release = self.released_lanes.as_ref().zip(release);
        if let Some((_, (snapshot, lane))) = release {
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
        }

        let inserted = if release.is_some() || record.creation_fence.is_some() {
            use crate::store::sandbox_mongo as transaction;

            let database = self
                .operations
                .client()
                .database(&self.operations.namespace().db);
            let mut session = database.client().start_session().await.map_err(driver)?;
            transaction::start_transaction(&mut session)
                .await
                .map_err(driver)?;
            if let Some((store, (snapshot, lane))) = release {
                match store.fence_snapshot(snapshot, lane, &mut session).await {
                    Ok(()) => {}
                    Err(SandboxStoreError::Conflict) => {
                        return Err(LifecycleCreateError::StaleReleasedRuntime);
                    }
                    Err(error) => return Err(StoreError::Unavailable(error.to_string()).into()),
                }
            }
            if let Some(fence) = &record.creation_fence {
                let linked = super::super::creation_outcomes::link_operation(
                    &database,
                    &record.owner,
                    fence,
                    &record.id,
                    &mut session,
                )
                .await
                .map_err(driver)?;
                if !linked {
                    session.abort_guarded().await.map_err(driver)?;
                    return Err(StoreError::Unavailable(
                        "creation submission fence is no longer held".into(),
                    )
                    .into());
                }
            }
            match self
                .operations
                .insert_one(&record)
                .session(&mut session)
                .await
            {
                Ok(_) => match transaction::commit_transaction(&mut session).await {
                    transaction::CommitOutcome::Committed => Ok(()),
                    transaction::CommitOutcome::Retry(error)
                    | transaction::CommitOutcome::Unknown(error)
                    | transaction::CommitOutcome::Failed(error) => Err(error),
                },
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
            Err(error)
                if release.is_some() && error.contains_label("TransientTransactionError") =>
            {
                Err(LifecycleCreateError::StaleReleasedRuntime)
            }
            Err(error) => Err(driver(error).into()),
        }
    }

    pub(crate) async fn fail_fenced_creation(
        &self,
        owner: &SubjectId,
        lease: &LifecycleLease,
        fence: &layer2_core::sandbox::store::CreationFence,
        message: &str,
        failure: Option<layer1_public::rest::AgentLifecycleFailure>,
    ) -> Result<bool, StoreError> {
        if !self.supports_creation(fence) {
            return Err(StoreError::Unavailable(
                "creation submission epoch is unsupported".into(),
            ));
        }
        let database = self
            .operations
            .client()
            .database(&self.operations.namespace().db);
        super::super::creation_outcomes::refuse_with_operation(
            &database,
            owner,
            fence,
            &lease.id,
            &lease.token,
            super::writes::failure_update(message, failure)?,
        )
        .await
    }
}
