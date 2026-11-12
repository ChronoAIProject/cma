use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::ids::SandboxCommandId;
use crate::resource_access::{
    AccessError, AccessPurpose, CallerContext, ResourceAccessError, ResourceAccessService,
    ResourceCapability, ResourceTarget, MAX_MEMBERSHIP_AGE,
};
use crate::sandbox::agent_io::{
    command_key_digest, AgentCommandAdmissionAuthority, CommandAuthorizationError,
};
use crate::SandboxId;

use super::*;

#[derive(Debug, Clone)]
pub struct SandboxControlSubmission {
    pub sandbox_id: SandboxId,
    pub command: SandboxControl,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxControlServiceError {
    #[error(transparent)]
    Access(#[from] ResourceAccessError),
    #[error(transparent)]
    Authority(#[from] CommandAuthorizationError),
    #[error(transparent)]
    Store(#[from] SandboxControlError),
    #[error("model budget refused the revival: {0}")]
    Budget(crate::budget::BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
}

pub struct SandboxControlService {
    store: Arc<dyn SandboxControlStore>,
    access: Arc<ResourceAccessService>,
    authority: Arc<dyn AgentCommandAdmissionAuthority>,
    platform_spend: std::sync::OnceLock<Arc<dyn crate::budget::PlatformSpendLedger>>,
}

impl SandboxControlService {
    pub fn new(
        store: Arc<dyn SandboxControlStore>,
        access: Arc<ResourceAccessService>,
        authority: Arc<dyn AgentCommandAdmissionAuthority>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            access,
            authority,
            platform_spend: std::sync::OnceLock::new(),
        })
    }

    /// An explicit Revive reads the platform admission state at admission,
    /// as the owner's revive does (contract section 5.1); other controls
    /// carry no new commitment and stay available under a stop.
    pub fn attach_platform_spend(&self, spend: Arc<dyn crate::budget::PlatformSpendLedger>) {
        let _ = self.platform_spend.set(spend);
    }

    async fn require_platform_open(
        &self,
        cause: crate::admission::AdmissionCause,
    ) -> Result<(), crate::admission::AdmissionRefusal> {
        let Some(spend) = self.platform_spend.get() else {
            return Ok(());
        };
        let now = crate::seconds_from_rfc3339(&crate::now_rfc3339())
            .ok_or(crate::budget::BudgetError::Unavailable)?;
        match spend
            .admission_state(now)
            .await?
            .commitment_refusal_for(cause)
        {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    pub async fn admit(
        &self,
        context: &CallerContext,
        credential: &str,
        submission: SandboxControlSubmission,
    ) -> Result<SandboxControlRecord, SandboxControlServiceError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.admit_inner(context, credential, submission),
        )
        .await
        .map_err(|_| SandboxControlError::Unavailable)?
    }

    async fn admit_inner(
        &self,
        context: &CallerContext,
        credential: &str,
        submission: SandboxControlSubmission,
    ) -> Result<SandboxControlRecord, SandboxControlServiceError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(ResourceAccessError::Access(AccessError::NotFound).into());
        }
        if submission.expected_revision == 0
            || submission.expected_revision > i64::MAX as u64
            || submission.sandbox_id.as_str().is_empty()
            || submission.sandbox_id.as_str().len() > 256
            || submission.sandbox_id.as_str().trim() != submission.sandbox_id.as_str()
        {
            return Err(SandboxControlError::Invalid.into());
        }
        let key_sha256 = command_key_digest(&submission.idempotency_key)
            .map_err(|_| SandboxControlError::Invalid)?;
        let started = SystemTime::now();
        let deadline = tokio::time::Instant::now() + MAX_MEMBERSHIP_AGE;
        let target = ResourceTarget::Sandbox {
            sandbox_id: submission.sandbox_id.clone(),
        };
        let capability = submission.command.capability();
        let authorized = self
            .access
            .authorize(
                context,
                Some(credential),
                &target,
                capability,
                AccessPurpose::Mutation {
                    started_at: started,
                },
            )
            .await?;
        let owner = authorized
            .snapshot
            .scope(&target)
            .map_err(ResourceAccessError::from)?
            .owner;
        let authority = self
            .authority
            .command_admission_authority(context, credential, &owner)
            .await;
        // Slow custody failures cannot conceal a concurrent resource revocation.
        self.access
            .validate_read_result(&authorized, &target, capability)
            .await?;
        let envelope = SandboxControlEnvelope {
            command_id: SandboxCommandId::generate(),
            sandbox_id: submission.sandbox_id,
            sandbox_created_at: authorized.snapshot.lineage.sandbox.created_at.clone(),
            owner,
            actor: context.actor.clone(),
            authorization: authority?,
            command: submission.command,
            expected_revision: submission.expected_revision,
            request_fingerprint: submission.command.fingerprint(submission.expected_revision),
        };
        envelope.validate()?;
        let proof = SandboxControlAuthorization::bind(
            (*self.access).clone(),
            authorized,
            envelope.clone(),
            context,
            deadline,
        )?;
        let now = SystemTime::now();
        let record = SandboxControlRecord {
            schema_version: SANDBOX_CONTROL_SCHEMA_VERSION,
            envelope,
            key_sha256,
            admitted_at: crate::rfc3339_at(now),
            queue_deadline: crate::rfc3339_at(
                now + Duration::from_secs(SANDBOX_CONTROL_QUEUE_SECONDS),
            ),
            status: SandboxControlStatus::Queued,
            revision: 1,
            admission_audit: proof.audit()?,
            execution_audit: None,
        };
        if record.envelope.command == SandboxControl::Revive {
            match self
                .require_platform_open(crate::admission::AdmissionCause::Fresh)
                .await
            {
                Err(crate::admission::AdmissionRefusal::Paused) => {
                    if let Some(accepted) = self.store.lookup_sandbox_control(&record).await? {
                        return Ok(accepted);
                    }
                    return Err(SandboxControlServiceError::AdmissionPaused);
                }
                result => result.map_err(SandboxControlServiceError::from)?,
            }
        }
        Ok(self.store.reserve_sandbox_control(record, proof).await?)
    }

    /// The requesting actor can recover its receipt only while the exact target
    /// remains visible. A guessed ID never reveals another actor's command.
    pub async fn read(
        &self,
        context: &CallerContext,
        credential: &str,
        id: &SandboxCommandId,
    ) -> Result<SandboxControlRecord, SandboxControlServiceError> {
        tokio::time::timeout(Duration::from_secs(15), async {
            if !context.validate_principal() {
                return Err(ResourceAccessError::Access(AccessError::NotFound).into());
            }
            let record = self
                .store
                .sandbox_control(id)
                .await?
                .ok_or(SandboxControlError::NotFound)?;
            record.validate()?;
            if record.envelope.actor != context.actor {
                return Err(SandboxControlError::NotFound.into());
            }
            let target = record.envelope.target();
            let held = self
                .access
                .authorize(
                    context,
                    Some(credential),
                    &target,
                    ResourceCapability::ViewDetails,
                    AccessPurpose::Read,
                )
                .await?;
            let current = self
                .access
                .current_read_snapshot(&held, &target, ResourceCapability::ViewDetails)
                .await?;
            if current
                .scope(&target)
                .map_err(ResourceAccessError::from)?
                .owner
                != record.envelope.owner
                || current.lineage.sandbox.created_at != record.envelope.sandbox_created_at
            {
                return Err(SandboxControlError::NotFound.into());
            }
            Ok(record)
        })
        .await
        .map_err(|_| SandboxControlError::Unavailable)?
    }
}
