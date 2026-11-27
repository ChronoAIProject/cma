//! Fresh lifecycle operations check majority control and capacity before the
//! single-document insert. Recovery never repeats this admission check.

use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::spec::BinarySubtype;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub(crate) enum LifecycleCreateError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
    #[error("retained home capacity is temporarily unavailable")]
    HomeCapacity,
    #[error("released runtime changed before lifecycle admission")]
    StaleReleasedRuntime,
}

impl LifecycleRecord {
    pub(super) fn for_admission(
        id: String,
        owner: &SubjectId,
        key: &str,
        input: LifecycleInput,
    ) -> Result<Self, StoreError> {
        let bytes = serde_json::to_vec(&input.request)
            .map_err(|error| StoreError::Malformed(format!("encode lifecycle input: {error}")))?;
        let fingerprint = Binary {
            subtype: BinarySubtype::Generic,
            bytes: Sha256::digest(&bytes).to_vec(),
        };
        let now = DateTime::now();
        Ok(Self {
            id,
            owner: owner.to_string(),
            key: key.to_owned(),
            fingerprint,
            creation_wire: None,
            creation_fence: None,
            input,
            released_lane: None,
            canary_admission: None,
            canary_manifest_digest: None,
            canary_redemption_key: None,
            status: "queued".into(),
            phase: "queued".into(),
            events: Vec::new(),
            sandbox_id: None,
            agent_id: None,
            repository_commit: None,
            repository_access_pin: None,
            public_repository: None,
            error: None,
            failure: None,
            connection: None,
            lease_expires_at: None,
            lease_token: None,
            attempts: 0,
            sandbox_quota_refusals: None,
            retry_at: None,
            created_at: now,
            updated_at: now,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
        })
    }

    /// The release selected by the ordinary creation of a new Machine Sandbox.
    pub(in crate::store) fn admitted_machine_image(&self) -> Option<&str> {
        let request = &self.input.request;
        if self.document_schema_version != DOCUMENT_SCHEMA_VERSION
            || self.canary_admission.is_some()
            || canary_key::is_reserved(&self.key)
            || !self.input.behavior.uses_repository_profile()
            || !request.machine_included
            || request.image.is_some()
            || request.workspace_id.is_some()
            || request.sandbox_id.is_some()
        {
            return None;
        }
        self.input
            .resolved_image
            .as_deref()
            .filter(|image| crate::agent_lifecycle::valid_immutable_image(image))
    }
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

    pub(crate) fn creation_terminal_settlement_enabled(&self) -> bool {
        self.creation_terminal_settlement_enabled
    }

    pub(crate) fn supports_creation(
        &self,
        fence: &layer2_core::sandbox::store::CreationFence,
    ) -> bool {
        self.creation_writer_epoch.as_deref() == Some(fence.epoch.as_str())
    }

    pub(crate) async fn create_with_submission(
        &self,
        owner: &SubjectId,
        key: &str,
        input: LifecycleInput,
        submission: Option<layer1_public::rest::creation_outcomes::Submission>,
        release: Option<(
            &layer3_adapter::ReleasedRuntime,
            layer2_core::released_runtime::ReleasedLane,
        )>,
    ) -> Result<CreateResult, LifecycleCreateError> {
        use layer1_public::rest::creation_outcomes::is_covered_key;

        if canary_key::is_reserved(key) {
            return Err(StoreError::Malformed("reserved lifecycle key".into()).into());
        }
        let (creation_wire, creation_fence) = match submission {
            Some(submission) => (
                Some(CreationWire {
                    deployment: submission.deployment,
                    digest: submission.wire_digest,
                }),
                submission.fence,
            ),
            None => (None, None),
        };
        if is_covered_key(key) != creation_fence.is_some() {
            return Err(StoreError::Malformed(
                "creation submission fence missing or unsupported".into(),
            )
            .into());
        }
        if let Some(existing) = self
            .operations
            .find_one(doc! { "owner": owner.as_str(), "key": key })
            .await
            .map_err(driver)?
        {
            schema::require_current(existing.document_schema_version)?;
            if !same_request(&existing.input.request, &input.request)? {
                return Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual: 1,
                }
                .into());
            }
            return Ok(CreateResult::Existing(Box::new(existing)));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut record = LifecycleRecord::for_admission(id, owner, key, input)?;
        record.creation_wire = creation_wire;
        record.creation_fence = creation_fence;
        self.finish_create(record, release).await
    }

    pub(super) async fn finish_create(
        &self,
        mut record: LifecycleRecord,
        release: Option<(
            &layer3_adapter::ReleasedRuntime,
            layer2_core::released_runtime::ReleasedLane,
        )>,
    ) -> Result<CreateResult, LifecycleCreateError> {
        let owner = SubjectId::new(&record.owner);
        let request = &record.input.request;
        let new_home = (request.sandbox_id.is_none() && request.workspace_id.is_none())
            .then_some((&owner, self.global_sandbox_limit));
        match super::super::admission_checks::require_fresh_lifecycle(
            &self.admission_settings,
            new_home,
        )
        .await
        {
            Ok(()) => {}
            Err(
                error @ (SandboxStoreError::AdmissionPaused
                | SandboxStoreError::GlobalCapacityReached),
            ) => {
                let refusal = if error == SandboxStoreError::AdmissionPaused {
                    LifecycleCreateError::AdmissionPaused
                } else {
                    LifecycleCreateError::HomeCapacity
                };
                // A concurrent exact writer may have committed after the outer replay read.
                return self
                    .replay(&owner, &record.key, request)
                    .await?
                    .map(|existing| CreateResult::Existing(Box::new(existing)))
                    .ok_or(refusal);
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
            if released::managed_lifecycle_release_lane(
                &record.input.request,
                record.input.behavior,
            ) != Some(lane)
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
        code: layer1_public::rest::creation_outcomes::RefusalCode,
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
            code,
        )
        .await
    }

    pub(crate) async fn cancel_fenced_creation(
        &self,
        owner: &SubjectId,
        record: &LifecycleRecord,
    ) -> Result<bool, StoreError> {
        let Some(fence) = record.creation_fence.as_ref() else {
            return Ok(false);
        };
        if !self.creation_terminal_settlement_enabled
            || !self.supports_creation(fence)
            || record.owner != owner.as_str()
        {
            return Ok(false);
        }
        let database = self
            .operations
            .client()
            .database(&self.operations.namespace().db);
        let mut failure =
            super::writes::failure_update("Creation cancelled before Sandbox allocation", None)?;
        failure
            .get_document_mut("$set")
            .expect("failure update is a set")
            .insert("connection", Bson::Null);
        let result = super::super::creation_outcomes::cancel_with_operation(
            &database, owner, fence, &record.id, failure,
        )
        .await;
        if super::super::creation_outcomes::cancellation_committed(
            &database, owner, fence, &record.id,
        )
        .await?
        {
            return Ok(true);
        }
        result.map(|_| false)
    }
}
