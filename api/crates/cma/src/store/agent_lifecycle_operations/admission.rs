//! Fresh lifecycle operations check majority control before the original
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
            canary_admission: None,
            canary_manifest_digest: None,
            canary_redemption_key: None,
            status: "queued".into(),
            phase: "queued".into(),
            events: Vec::new(),
            sandbox_id: None,
            agent_id: None,
            repository_commit: None,
            error: None,
            failure: None,
            connection: None,
            lease_expires_at: None,
            lease_token: None,
            attempts: 0,
            retry_at: None,
            created_at: now,
            updated_at: now,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
        })
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
        self.create_with_submission(owner, key, input, None).await
    }

    pub(crate) fn supports_creation(
        &self,
        fence: &layer2_core::sandbox::store::CreationFence,
    ) -> bool {
        self.creation_writer_epoch.as_deref() == Some(fence.epoch.as_str())
    }

    pub(super) async fn finish_create(
        &self,
        record: LifecycleRecord,
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
        if let Some(fence) = &record.creation_fence {
            if !self.supports_creation(fence) {
                return Err(StoreError::Unavailable(
                    "creation submission epoch is unsupported".into(),
                )
                .into());
            }
            let database = self
                .operations
                .client()
                .database(&self.operations.namespace().db);
            let mut session = database.client().start_session().await.map_err(driver)?;
            use crate::store::sandbox_mongo as transaction;
            transaction::start_transaction(&mut session)
                .await
                .map_err(driver)?;
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
            self.operations
                .insert_one(&record)
                .session(&mut session)
                .await
                .map_err(driver)?;
            return match transaction::commit_transaction(&mut session).await {
                transaction::CommitOutcome::Committed => Ok(CreateResult::Created(record.id)),
                transaction::CommitOutcome::Retry(error)
                | transaction::CommitOutcome::Unknown(error)
                | transaction::CommitOutcome::Failed(error) => Err(driver(error).into()),
            };
        }
        match self.operations.insert_one(&record).await {
            Ok(_) => Ok(CreateResult::Created(record.id)),
            Err(error) if is_duplicate_key(&error) => self
                .replay(
                    &SubjectId::new(&record.owner),
                    &record.key,
                    &record.input.request,
                )
                .await?
                .map(|existing| CreateResult::Existing(Box::new(existing)))
                .ok_or_else(|| {
                    StoreError::Unavailable("lifecycle operation raced out of view".into()).into()
                }),
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
