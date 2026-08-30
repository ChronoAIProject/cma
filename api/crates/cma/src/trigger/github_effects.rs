//! Durable Trigger effects backed by the typed NyxID/GitHub broker.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    effect_requires_authority_admission, rfc3339_at, seconds_from_rfc3339, trigger_session_id,
    ActivateSponsoredGrant, AuthorityAdmissionSnapshot, AuthorityProvenance, Authorization,
    BaselineImportBlob, BaselineImportChunk, BaselineImportOutcome, BaselineObjectStore,
    BillingMode, BranchRecord, Caller, Capability, CapabilityAuthorityAdmission,
    CapabilityOperation, CapabilityRevocationState, CheckpointIntegrity, CheckpointObjectStore,
    CleanupReceipt, CleanupTarget, ConnectionSnapshot, CoreError, DelegationState, DiffArtifact,
    DiffArtifactObjectStore, DiffValidationOutcome, EffectClaim, EffectClaimMode, FailureClass,
    InertGrantCapability, Launch, LaunchAuthority, LaunchCapability, LaunchEffectKind,
    LaunchEffectResult, LaunchFailure, PullRequestRecord, PullRequestState, RecoveryState,
    RepositoryIdentity, Session, SessionCredentialAuthority, SessionId, SessionLink,
    SessionService, SessionStatus, SponsoredModelAuthorityError, SponsorshipError,
    SponsorshipService, SponsorshipStoreError, SubjectId, TriggerAuthorityError,
    TriggerAuthorityVerifier, TriggerServiceError, TriggerStoreError, WorkspaceCheckpoint,
    WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest, WorkspaceRecord,
    BASELINE_IMPORT_CHUNK_BYTES, CAPABILITY_MAX_LIFETIME_SECONDS, CAPABILITY_POLICY_VERSION,
    PULL_REQUEST_STATUS_INTERVAL_SECONDS,
};
use sha2::{Digest as _, Sha256};

use super::capability_signing::{
    CapabilitySigner, RuntimePolicyTombstone, POLICY_AUDIENCE, POLICY_VERSION,
};
#[cfg(test)]
use super::github::WorkspaceSourceEntry;
use super::github::{
    build_baseline_archive_plan, observe_provider_requests, validate_repository_rebinding,
    workspace_blob_metadata, BranchContext, BranchReconciliation, ForkContext, ForkReconciliation,
    GithubBroker, GithubBrokerError, PullRequestContext, WorkspaceBlobMetadata, WorkspaceContext,
    WorkspaceSourcePlan, WorkspaceUnsupported,
};
use super::pull_request_status::PullRequestStatusDisposition;
use super::worker::{EffectExecution, EffectExecutor};
use super::workspace_diff::{
    build_diff_artifact, build_invalid_diff_artifact, deterministic_diff_id, DiffBuildError,
};
#[cfg(test)]
use layer2_core::BaselineArchivePlan;

#[cfg(test)]
const DEFAULT_FORK_READINESS_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PROVIDER_RETRY: Duration = Duration::from_secs(300);
const MAX_RETRY_AFTER: u64 = 3_600;
const STAGING_IMPORT_RETENTION_POLICY_ID: &str = "trigger-workspace-import-v1";
#[cfg(test)]
const TEST_CHANGED_FILE_LIMIT: u32 = 1_000;

impl EffectExecution {
    fn with_provider_request_id(mut self, provider_request_id: Option<String>) -> Self {
        let Some(provider_request_id) = provider_request_id else {
            return self;
        };
        let resources = match &mut self {
            Self::Completed {
                external_resource_ids,
                ..
            }
            | Self::Progressed {
                external_resource_ids,
                ..
            }
            | Self::RetryLater {
                external_resource_ids,
                ..
            }
            | Self::ReconcileLater {
                external_resource_ids,
                ..
            }
            | Self::WaitLater {
                external_resource_ids,
                ..
            }
            | Self::RetryWithProgress {
                external_resource_ids,
                ..
            }
            | Self::ReconcileWithProgress {
                external_resource_ids,
                ..
            } => external_resource_ids,
            Self::FailedWithProgress { .. } | Self::Failed { .. } => return self,
        };
        resources.insert("provider_request_id".into(), provider_request_id);
        self
    }
}

#[derive(Debug, Clone)]
struct CapabilityAuthorityEvidence {
    connection: ConnectionSnapshot,
    authority: AuthorityProvenance,
    upstream: RepositoryIdentity,
    fork: RepositoryIdentity,
    branch: BranchRecord,
}

#[derive(Debug, Clone)]
struct EffectAuthorityEvidence {
    connection: ConnectionSnapshot,
    authority: AuthorityProvenance,
    upstream: RepositoryIdentity,
    fork: Option<RepositoryIdentity>,
    branch: Option<BranchRecord>,
}

#[derive(Debug, Clone)]
enum RetainedCleanupObject {
    Baseline { reference: String },
    BaselineImport,
    Checkpoint(WorkspaceCheckpoint),
    CheckpointIntent(layer2_core::WorkspaceCheckpointIntent),
    DiffArtifact(DiffArtifact),
}

#[async_trait]
trait GithubEffectPort: Send + Sync {
    async fn preflight(&self, launch: &Launch) -> Result<LaunchAuthority, TriggerServiceError>;

    async fn create_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError>;

    async fn reconcile_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError>;

    async fn create_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError>;

    async fn reconcile_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError>;

    async fn import_workspace(
        &self,
        context: &WorkspaceContext,
    ) -> Result<WorkspaceSourcePlan, GithubBrokerError>;

    async fn read_workspace_blob(
        &self,
        context: &WorkspaceContext,
        blob: &BaselineImportBlob,
    ) -> Result<Vec<u8>, GithubBrokerError>;

    async fn preflight_capability(
        &self,
        launch: &Launch,
    ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError>;

    async fn preflight_effect_authority(
        &self,
        launch: &Launch,
        kind: LaunchEffectKind,
    ) -> Result<EffectAuthorityEvidence, GithubBrokerError> {
        if !matches!(
            kind,
            LaunchEffectKind::Reach
                | LaunchEffectKind::BranchPublication
                | LaunchEffectKind::PullRequest
        ) {
            return Err(GithubBrokerError::Malformed(
                "the adapter has no authority preflight for this effect".into(),
            ));
        }
        let evidence = self.preflight_capability(launch).await?;
        Ok(EffectAuthorityEvidence {
            connection: evidence.connection,
            authority: evidence.authority,
            upstream: evidence.upstream,
            fork: Some(evidence.fork),
            branch: Some(evidence.branch),
        })
    }

    async fn publish_branch(
        &self,
        context: &super::github::PublicationContext,
        plan: &super::workspace_diff::PublicationPlan,
    ) -> Result<super::github::PublicationAdvance, GithubBrokerError> {
        let _ = (context, plan);
        Err(GithubBrokerError::Malformed(
            "the Git Data publication adapter is unavailable".into(),
        ))
    }

    async fn reconcile_branch_publication(
        &self,
        context: &super::github::PublicationContext,
        plan: &super::workspace_diff::PublicationPlan,
    ) -> Result<super::github::PublicationAdvance, GithubBrokerError> {
        let _ = (context, plan);
        Err(GithubBrokerError::Malformed(
            "the read-only Git Data reconciliation adapter is unavailable".into(),
        ))
    }

    async fn write_pull_request(
        &self,
        context: &super::github::PullRequestContext,
    ) -> Result<super::github::PullRequestAdvance, GithubBrokerError> {
        let _ = context;
        Err(GithubBrokerError::Malformed(
            "the pull-request adapter is unavailable".into(),
        ))
    }

    async fn reconcile_pull_request(
        &self,
        context: &super::github::PullRequestContext,
    ) -> Result<super::github::PullRequestAdvance, GithubBrokerError> {
        let _ = context;
        Err(GithubBrokerError::Malformed(
            "the pull-request adapter is unavailable".into(),
        ))
    }

    async fn observe_pull_request(
        &self,
        context: &super::github::PullRequestContext,
    ) -> Result<PullRequestRecord, GithubBrokerError> {
        let _ = context;
        Err(GithubBrokerError::Malformed(
            "the pull-request status adapter is unavailable".into(),
        ))
    }
}

#[derive(Debug, Clone)]
struct TriggerSessionEvidence {
    session: Session,
    runtime_generation: Option<u64>,
    credential_authority: Option<SessionCredentialAuthority>,
}

#[async_trait]
trait TriggerSessionEffectPort: Send + Sync {
    async fn ensure_session(&self, launch: &Launch) -> Result<TriggerSessionEvidence, CoreError>;

    async fn credential_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionCredentialAuthority>, CoreError>;

    async fn install_runtime_policy_and_probe(
        &self,
        session_id: &SessionId,
        expected_credential_authority: &SessionCredentialAuthority,
        capability: LaunchCapability,
        signer: Arc<CapabilitySigner>,
        probe: layer3_adapter::RuntimeGatewayProbeRequest,
    ) -> Result<
        layer2_core::RuntimeGatewayReachabilityReceipt,
        layer2_core::RuntimeGatewayReachabilityError,
    >;

    async fn reconcile_runtime_policy_and_probe(
        &self,
        session_id: &SessionId,
        expected_credential_authority: &SessionCredentialAuthority,
        probe: layer3_adapter::RuntimeGatewayProbeRequest,
        expected_launch_id: &str,
        expected_generation: u64,
        expected_fence: u64,
    ) -> Result<
        layer2_core::RuntimeGatewayReconciliationReceipt,
        layer2_core::RuntimeGatewayReachabilityError,
    > {
        let _ = (
            session_id,
            expected_credential_authority,
            probe,
            expected_launch_id,
            expected_generation,
            expected_fence,
        );
        Err(layer2_core::RuntimeGatewayReachabilityError::ProtocolUnsupported)
    }

    async fn install_sponsored_model_authority(
        &self,
        session_id: &SessionId,
        expected_credential_authority: &SessionCredentialAuthority,
        capability: &InertGrantCapability,
        maximum_output_tokens_per_request: u64,
        gateway_origin: &layer3_adapter::SponsoredModelGatewayOrigin,
    ) -> Result<layer3_adapter::SponsoredModelAuthorityReceipt, SponsoredModelAuthorityError> {
        let _ = (
            session_id,
            expected_credential_authority,
            capability,
            maximum_output_tokens_per_request,
            gateway_origin,
        );
        Err(SponsoredModelAuthorityError::ProtocolUnsupported)
    }

    async fn runtime_policy_audience(&self, session_id: &SessionId) -> Result<String, CoreError>;

    async fn runtime_policy_observation(
        &self,
        session_id: &SessionId,
    ) -> Result<layer2_core::RuntimePolicyObservation, CoreError>;

    async fn install_runtime_tombstone(
        &self,
        session_id: &SessionId,
        tombstone: RuntimePolicyTombstone,
        signer: Arc<CapabilitySigner>,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError>;

    async fn retire_runtime_credential(&self, session_id: &SessionId) -> Result<(), CoreError>;

    async fn runtime_credential_retirement_settled(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, CoreError>;

    async fn dispatch_initial_prompt(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<layer2_core::PromptDelivery, CoreError>;

    async fn checkpoint_workspace(
        &self,
        launch: &Launch,
        request: &layer3_adapter::WorkspaceCheckpointRequest,
    ) -> Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError> {
        let _ = (launch, request);
        Err(CoreError::CapabilityUnsupported(
            "workspace_checkpoint_v2".into(),
        ))
    }
}

#[async_trait]
trait TriggerSponsorshipEffectPort: Send + Sync {
    async fn activate_grant(
        &self,
        contributor: &SubjectId,
        command: ActivateSponsoredGrant,
    ) -> Result<ActivatedSponsoredModelGrant, SponsorshipError>;
}

#[derive(Clone)]
struct ActivatedSponsoredModelGrant {
    capability: InertGrantCapability,
    maximum_output_tokens_per_request: u64,
}

struct SponsorshipServiceEffectPort(Arc<SponsorshipService>);

#[async_trait]
impl TriggerSponsorshipEffectPort for SponsorshipServiceEffectPort {
    async fn activate_grant(
        &self,
        contributor: &SubjectId,
        command: ActivateSponsoredGrant,
    ) -> Result<ActivatedSponsoredModelGrant, SponsorshipError> {
        let caller = Caller::new(
            contributor.clone(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        let grant = self.0.activate_grant(&caller, command).await?;
        grant.validate()?;
        let maximum_output_tokens_per_request = (grant.allocation.quota.output_tokens
            / grant.allocation.quota.requests)
            .max(1)
            .min(u64::from(u32::MAX));
        Ok(ActivatedSponsoredModelGrant {
            capability: grant.capability,
            maximum_output_tokens_per_request,
        })
    }
}

struct SponsoredEffectRuntime {
    sponsorships: Arc<dyn TriggerSponsorshipEffectPort>,
    gateway_origin: layer3_adapter::SponsoredModelGatewayOrigin,
}

struct SessionServiceEffectPort(Arc<SessionService>);

#[async_trait]
impl TriggerSessionEffectPort for SessionServiceEffectPort {
    async fn ensure_session(&self, launch: &Launch) -> Result<TriggerSessionEvidence, CoreError> {
        let session = self.0.ensure_trigger_session(launch).await?;
        let runtime_generation = self
            .0
            .trigger_runtime_generation(&session.id)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))?;
        let credential_authority = self
            .0
            .trigger_credential_authority(&session.id)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))?;
        Ok(TriggerSessionEvidence {
            session,
            runtime_generation,
            credential_authority,
        })
    }

    async fn credential_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionCredentialAuthority>, CoreError> {
        self.0
            .trigger_credential_authority(session_id)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }

    async fn install_runtime_policy_and_probe(
        &self,
        session_id: &SessionId,
        expected_credential_authority: &SessionCredentialAuthority,
        mut capability: LaunchCapability,
        signer: Arc<CapabilitySigner>,
        probe: layer3_adapter::RuntimeGatewayProbeRequest,
    ) -> Result<
        layer2_core::RuntimeGatewayReachabilityReceipt,
        layer2_core::RuntimeGatewayReachabilityError,
    > {
        self.0
            .install_runtime_policy_and_probe_with_credential_authority(
                session_id,
                probe,
                expected_credential_authority,
                move |runtime_audience| {
                    capability.runtime_audience = runtime_audience.to_string();
                    signer
                        .sign_policy(&capability)
                        .map_err(CoreError::InvalidRequest)
                },
            )
            .await
    }

    async fn reconcile_runtime_policy_and_probe(
        &self,
        session_id: &SessionId,
        expected_credential_authority: &SessionCredentialAuthority,
        probe: layer3_adapter::RuntimeGatewayProbeRequest,
        expected_launch_id: &str,
        expected_generation: u64,
        expected_fence: u64,
    ) -> Result<
        layer2_core::RuntimeGatewayReconciliationReceipt,
        layer2_core::RuntimeGatewayReachabilityError,
    > {
        self.0
            .reconcile_runtime_policy_and_probe_with_credential_authority(
                session_id,
                probe,
                expected_credential_authority,
                expected_launch_id,
                expected_generation,
                expected_fence,
            )
            .await
    }

    async fn install_sponsored_model_authority(
        &self,
        session_id: &SessionId,
        expected_credential_authority: &SessionCredentialAuthority,
        capability: &InertGrantCapability,
        maximum_output_tokens_per_request: u64,
        gateway_origin: &layer3_adapter::SponsoredModelGatewayOrigin,
    ) -> Result<layer3_adapter::SponsoredModelAuthorityReceipt, SponsoredModelAuthorityError> {
        self.0
            .install_sponsored_model_authority(
                session_id,
                expected_credential_authority,
                capability,
                maximum_output_tokens_per_request,
                gateway_origin,
            )
            .await
    }

    async fn runtime_policy_audience(&self, session_id: &SessionId) -> Result<String, CoreError> {
        self.0.runtime_policy_audience(session_id).await
    }

    async fn runtime_policy_observation(
        &self,
        session_id: &SessionId,
    ) -> Result<layer2_core::RuntimePolicyObservation, CoreError> {
        self.0.runtime_policy_observation(session_id).await
    }

    async fn install_runtime_tombstone(
        &self,
        session_id: &SessionId,
        mut tombstone: RuntimePolicyTombstone,
        signer: Arc<CapabilitySigner>,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError> {
        self.0
            .install_runtime_policy(session_id, move |runtime_audience| {
                tombstone.runtime_audience = runtime_audience.to_string();
                signer
                    .sign_tombstone(tombstone)
                    .map_err(CoreError::InvalidRequest)
            })
            .await
    }

    async fn retire_runtime_credential(&self, session_id: &SessionId) -> Result<(), CoreError> {
        self.0.retire_runtime_credential(session_id).await
    }

    async fn runtime_credential_retirement_settled(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, CoreError> {
        self.0
            .runtime_credential_retirement_settled(session_id)
            .await
    }

    async fn dispatch_initial_prompt(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<layer2_core::PromptDelivery, CoreError> {
        self.0.dispatch_trigger_prompt(launch, claim).await
    }

    async fn checkpoint_workspace(
        &self,
        launch: &Launch,
        request: &layer3_adapter::WorkspaceCheckpointRequest,
    ) -> Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError> {
        let session = launch.session.as_ref().ok_or(CoreError::InvalidState)?;
        self.0
            .checkpoint_trigger_workspace(
                &session.session_id,
                &launch.id,
                &launch.owner,
                session.runtime_generation,
                request,
            )
            .await
    }
}

async fn preflight_github_authority(
    github: &GithubBroker,
    launch: &Launch,
    kind: LaunchEffectKind,
    require_retained_branch_head: bool,
) -> Result<EffectAuthorityEvidence, GithubBrokerError> {
    let expected_connection = launch.connection.as_ref().ok_or_else(|| {
        GithubBrokerError::Malformed("the Launch has no selected connection".into())
    })?;
    let expected_upstream = &launch.revision.content.upstream.repository;
    let mut upstream = github
        .repository_by_id_with_connection(
            &launch.owner,
            expected_connection,
            expected_upstream.repository_id,
        )
        .await?;
    upstream.identity.selected_ref = expected_upstream.selected_ref.clone();
    validate_capability_upstream(expected_upstream, &upstream)?;
    if kind == LaunchEffectKind::Fork && !upstream.identity.allow_forking {
        return Err(GithubBrokerError::ForkingDisabled);
    }

    let fork_required = kind != LaunchEffectKind::Fork;
    let branch_required = matches!(
        kind,
        LaunchEffectKind::Reach
            | LaunchEffectKind::BranchPublication
            | LaunchEffectKind::PullRequest
    );
    let fork = if fork_required {
        let expected_fork = &launch
            .fork
            .as_ref()
            .ok_or_else(|| GithubBrokerError::Malformed("the Launch has no fork".into()))?
            .repository;
        let mut fork = github
            .repository_by_id_with_connection(
                &launch.owner,
                expected_connection,
                expected_fork.repository_id,
            )
            .await?;
        fork.identity.selected_ref = expected_fork.selected_ref.clone();
        validate_capability_fork(expected_fork, &fork, expected_connection)?;
        Some(fork.identity)
    } else {
        None
    };

    let branch = if branch_required {
        let branch_context = BranchContext::try_from(launch)?;
        let mut observed_branch = github.observe_branch(&branch_context).await?;
        let expected_branch = launch.branch.as_ref().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no fenced branch".into())
        })?;
        if observed_branch.name != expected_branch.name
            || observed_branch.expected_head != expected_branch.expected_head
            || (require_retained_branch_head
                && observed_branch.current_head != expected_branch.current_head)
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        observed_branch.created_at = expected_branch.created_at.clone();
        Some(observed_branch)
    } else {
        None
    };

    // Last provider read before admission: this proves the exact UserService,
    // account, backing key, and scope still match the Launch.
    let current = github
        .preflight_connection(&launch.owner, &expected_connection.service_id)
        .await?;
    if !same_connection_authority(expected_connection, &current.snapshot) {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    let observed_at = rfc3339_at(SystemTime::now());
    Ok(EffectAuthorityEvidence {
        connection: current.snapshot.clone(),
        authority: AuthorityProvenance {
            subject: launch.owner.clone(),
            subject_token_generation: current.subject_token_generation,
            service_id: current.snapshot.service_id.clone(),
            github_catalog_service_id: current.snapshot.github_catalog_service_id.clone(),
            github_user_service_slug: current.snapshot.github_user_service_slug.clone(),
            service_state_version: current.snapshot.service_state_version,
            backing_key_id: current.snapshot.backing_key_id.clone(),
            session_credential_authority: None,
            state: DelegationState::Available,
            observed_at: observed_at.clone(),
        },
        upstream: upstream.identity,
        fork,
        branch,
    })
}

#[cfg(test)]
pub(super) async fn preflight_github_authority_for_test(
    github: &GithubBroker,
    launch: &Launch,
    kind: LaunchEffectKind,
    require_retained_branch_head: bool,
) -> Result<(), GithubBrokerError> {
    preflight_github_authority(github, launch, kind, require_retained_branch_head)
        .await
        .map(|_| ())
}

#[async_trait]
impl GithubEffectPort for GithubBroker {
    async fn preflight(&self, launch: &Launch) -> Result<LaunchAuthority, TriggerServiceError> {
        let connection = launch.connection.as_ref().ok_or_else(|| {
            layer2_core::TriggerValidationError::field(
                "connection",
                "the Launch has no selected connection",
            )
        })?;
        self.verify_launch(
            &launch.owner,
            &launch.revision,
            &connection.service_id,
            Some(connection),
        )
        .await
    }

    async fn create_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        GithubBroker::create_fork(self, context).await
    }

    async fn reconcile_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        GithubBroker::reconcile_fork(self, context).await
    }

    async fn create_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError> {
        GithubBroker::create_branch(self, context).await
    }

    async fn reconcile_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError> {
        GithubBroker::reconcile_branch(self, context).await
    }

    async fn import_workspace(
        &self,
        context: &WorkspaceContext,
    ) -> Result<WorkspaceSourcePlan, GithubBrokerError> {
        GithubBroker::import_workspace(self, context).await
    }

    async fn read_workspace_blob(
        &self,
        context: &WorkspaceContext,
        blob: &BaselineImportBlob,
    ) -> Result<Vec<u8>, GithubBrokerError> {
        GithubBroker::read_workspace_blob(self, context, blob).await
    }

    async fn observe_pull_request(
        &self,
        context: &PullRequestContext,
    ) -> Result<PullRequestRecord, GithubBrokerError> {
        GithubBroker::observe_pull_request(self, context).await
    }

    async fn preflight_capability(
        &self,
        launch: &Launch,
    ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError> {
        let evidence =
            preflight_github_authority(self, launch, LaunchEffectKind::Reach, false).await?;
        Ok(CapabilityAuthorityEvidence {
            connection: evidence.connection,
            authority: evidence.authority,
            upstream: evidence.upstream,
            fork: evidence.fork.ok_or_else(|| {
                GithubBrokerError::Malformed("the capability preflight has no fork".into())
            })?,
            branch: evidence.branch.ok_or_else(|| {
                GithubBrokerError::Malformed("the capability preflight has no branch".into())
            })?,
        })
    }

    async fn preflight_effect_authority(
        &self,
        launch: &Launch,
        kind: LaunchEffectKind,
    ) -> Result<EffectAuthorityEvidence, GithubBrokerError> {
        if !matches!(
            kind,
            LaunchEffectKind::Fork
                | LaunchEffectKind::Branch
                | LaunchEffectKind::Reach
                | LaunchEffectKind::BranchPublication
                | LaunchEffectKind::PullRequest
        ) {
            return Err(GithubBrokerError::Malformed(
                "the effect is not an irreversible GitHub authority boundary".into(),
            ));
        }
        preflight_github_authority(self, launch, kind, true).await
    }

    async fn publish_branch(
        &self,
        context: &super::github::PublicationContext,
        plan: &super::workspace_diff::PublicationPlan,
    ) -> Result<super::github::PublicationAdvance, GithubBrokerError> {
        GithubBroker::publish_branch(self, context, plan).await
    }

    async fn reconcile_branch_publication(
        &self,
        context: &super::github::PublicationContext,
        plan: &super::workspace_diff::PublicationPlan,
    ) -> Result<super::github::PublicationAdvance, GithubBrokerError> {
        GithubBroker::reconcile_branch_publication(self, context, plan).await
    }

    async fn write_pull_request(
        &self,
        context: &super::github::PullRequestContext,
    ) -> Result<super::github::PullRequestAdvance, GithubBrokerError> {
        GithubBroker::write_pull_request(self, context).await
    }

    async fn reconcile_pull_request(
        &self,
        context: &super::github::PullRequestContext,
    ) -> Result<super::github::PullRequestAdvance, GithubBrokerError> {
        GithubBroker::reconcile_pull_request(self, context).await
    }
}

fn same_connection_authority(expected: &ConnectionSnapshot, current: &ConnectionSnapshot) -> bool {
    expected.service_id == current.service_id
        && expected.github_catalog_service_id == current.github_catalog_service_id
        && expected.github_user_service_slug == current.github_user_service_slug
        && expected.service_state_version == current.service_state_version
        && expected.backing_key_id == current.backing_key_id
        && expected.account.account_id == current.account.account_id
        && expected.account.account_node_id == current.account.account_node_id
        && expected.granted_scopes.iter().collect::<BTreeSet<_>>()
            == current.granted_scopes.iter().collect::<BTreeSet<_>>()
        && expected
            .satisfied_scope_classes
            .iter()
            .collect::<BTreeSet<_>>()
            == current
                .satisfied_scope_classes
                .iter()
                .collect::<BTreeSet<_>>()
        && current.state == layer2_core::ConnectionState::Active
}

fn validate_capability_upstream(
    expected: &RepositoryIdentity,
    observed: &super::github::RepositoryEvidence,
) -> Result<(), GithubBrokerError> {
    validate_repository_rebinding(expected, observed)?;
    if !observed.permissions.pull {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok(())
}

fn validate_capability_fork(
    expected: &RepositoryIdentity,
    observed: &super::github::RepositoryEvidence,
    connection: &ConnectionSnapshot,
) -> Result<(), GithubBrokerError> {
    validate_repository_rebinding(expected, observed)?;
    if observed.identity.owner_id != connection.account.account_id
        || observed.identity.owner_node_id != connection.account.account_node_id
        || !observed.permissions.pull
        || !observed.permissions.push
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok(())
}

/// GitHub-backed part of the closed Trigger effect vocabulary.
///
/// Later runtime/workspace adapters compose beside this type. Unsupported
/// kinds fail closed so a configuration mistake cannot silently skip a step.
pub(crate) struct GithubEffectExecutor {
    github: Arc<dyn GithubEffectPort>,
    workspace_store: Option<Arc<dyn BaselineObjectStore>>,
    checkpoint_store: Option<Arc<dyn CheckpointObjectStore>>,
    diff_store: Option<Arc<dyn DiffArtifactObjectStore>>,
    sessions: Option<Arc<dyn TriggerSessionEffectPort>>,
    sponsorship: Option<SponsoredEffectRuntime>,
    capability_signer: Option<Arc<CapabilitySigner>>,
    fork_readiness_timeout: Duration,
    changed_file_limit: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GithubEffectRuntimeLimits {
    pub(crate) fork_readiness_timeout: Duration,
    pub(crate) changed_file_limit: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct CapabilityRenewal {
    pub(crate) reach: layer2_core::ReachStatus,
    pub(crate) connection: ConnectionSnapshot,
    pub(crate) authority: AuthorityProvenance,
    pub(crate) branch: BranchRecord,
}

#[derive(Debug, Clone)]
pub(crate) struct CapabilityRevocation {
    pub(crate) generation: u64,
    pub(crate) fence: u64,
    pub(crate) runtime_audience: String,
    pub(crate) tombstone_acknowledged: bool,
    pub(crate) agent_key_revocation_confirmed: bool,
    pub(crate) credential_session_id: SessionId,
}

impl GithubEffectExecutor {
    #[cfg(test)]
    pub(crate) fn new(github: Arc<GithubBroker>) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: None,
            diff_store: None,
            sessions: None,
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    pub(crate) fn with_runtime(
        github: Arc<GithubBroker>,
        workspace_store: Arc<dyn BaselineObjectStore>,
        checkpoint_store: Arc<dyn CheckpointObjectStore>,
        diff_store: Arc<dyn DiffArtifactObjectStore>,
        sessions: Arc<SessionService>,
        capability_signer: Arc<CapabilitySigner>,
        limits: GithubEffectRuntimeLimits,
    ) -> Self {
        Self {
            github,
            workspace_store: Some(workspace_store),
            checkpoint_store: Some(checkpoint_store),
            diff_store: Some(diff_store),
            sessions: Some(Arc::new(SessionServiceEffectPort(sessions))),
            sponsorship: None,
            capability_signer: Some(capability_signer),
            fork_readiness_timeout: limits.fork_readiness_timeout,
            changed_file_limit: limits.changed_file_limit,
        }
    }

    pub(crate) fn with_sponsorship(
        mut self,
        sponsorships: Arc<SponsorshipService>,
        gateway_origin: layer3_adapter::SponsoredModelGatewayOrigin,
    ) -> Self {
        self.sponsorship = Some(SponsoredEffectRuntime {
            sponsorships: Arc::new(SponsorshipServiceEffectPort(sponsorships)),
            gateway_origin,
        });
        self
    }

    #[cfg(test)]
    fn with_port(github: Arc<dyn GithubEffectPort>, fork_readiness_timeout: Duration) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: None,
            diff_store: None,
            sessions: None,
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    #[cfg(test)]
    fn with_session_port(
        github: Arc<dyn GithubEffectPort>,
        sessions: Arc<dyn TriggerSessionEffectPort>,
    ) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: None,
            diff_store: None,
            sessions: Some(sessions),
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    #[cfg(test)]
    fn with_capability_ports(
        github: Arc<dyn GithubEffectPort>,
        sessions: Arc<dyn TriggerSessionEffectPort>,
        capability_signer: Arc<CapabilitySigner>,
    ) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: None,
            diff_store: None,
            sessions: Some(sessions),
            sponsorship: None,
            capability_signer: Some(capability_signer),
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    #[cfg(test)]
    fn with_checkpoint_ports(
        github: Arc<dyn GithubEffectPort>,
        sessions: Arc<dyn TriggerSessionEffectPort>,
        checkpoint_store: Arc<dyn CheckpointObjectStore>,
    ) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: Some(checkpoint_store),
            diff_store: None,
            sessions: Some(sessions),
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    async fn fresh_session_credential_authority(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<SessionCredentialAuthority, EffectExecution> {
        let session = launch.session.as_ref().ok_or_else(|| {
            authority_admission_failure(
                claim,
                "trigger_session_credential_session_missing",
                "The Launch has no deterministic session for credential authority admission.",
            )
        })?;
        let sessions = self
            .sessions
            .as_ref()
            .ok_or_else(|| unsupported_effect(claim))?;
        match sessions.credential_authority(&session.session_id).await {
            Ok(Some(authority)) if authority.session_id == session.session_id => Ok(authority),
            Ok(_) => Err(transient_failure(
                claim,
                false,
                "trigger_session_credential_authority_pending",
                "The runtime has not acknowledged an exact current session credential yet.",
                None,
            )),
            Err(error) => Err(session_failure(error, claim)),
        }
    }

    pub(crate) async fn capability_runtime_observation(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<layer2_core::RuntimePolicyObservation, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        let session = launch.session.as_ref().ok_or_else(|| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_reach_session_missing",
                "The Launch has no deterministic session for delegated access.",
                "Contact support; CMA will not infer runtime policy state.",
                &claim,
                None,
            )
        })?;
        let sessions = self
            .sessions
            .as_ref()
            .ok_or_else(|| execution_failure(unsupported_effect(&claim)))?;
        sessions
            .runtime_policy_observation(&session.session_id)
            .await
            .map_err(|error| execution_failure(session_failure(error, &claim)))
    }

    pub(crate) async fn renew_capability(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRenewal, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        match self.run_reach(launch, &claim, true).await {
            EffectExecution::Completed {
                result:
                    LaunchEffectResult::Reach {
                        reach,
                        connection,
                        authority,
                        branch,
                    },
                ..
            } => Ok(CapabilityRenewal {
                reach,
                connection,
                authority,
                branch,
            }),
            EffectExecution::Completed { .. } => Err(failure(
                FailureClass::TerminalSecurity,
                "trigger_reach_result_mismatch",
                "The capability adapter returned a different effect result.",
                "Contact support; delegated access remains fail-closed.",
                &claim,
                None,
            )),
            execution => Err(execution_failure(execution)),
        }
    }

    pub(crate) async fn admit_capability_renewal(
        &self,
        launch: &Launch,
        fence: u64,
        target_policy_generation: u64,
        target_policy_expires_at_epoch_seconds: u64,
    ) -> Result<CapabilityAuthorityAdmission, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        let mut evidence = self
            .github
            .preflight_capability(launch)
            .await
            .map_err(|error| execution_failure(broker_failure(error, &claim, false)))?;
        evidence.authority.session_credential_authority = Some(
            self.fresh_session_credential_authority(launch, &claim)
                .await
                .map_err(execution_failure)?,
        );
        let observed_at = evidence.authority.observed_at.clone();
        let snapshot = AuthorityAdmissionSnapshot::for_capability_renewal(
            launch,
            fence,
            target_policy_generation,
            evidence.connection,
            evidence.authority,
            evidence.upstream,
            evidence.fork,
            evidence.branch,
            observed_at,
        )
        .map_err(|_| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_capability_authority_admission_invalid",
                "Fresh provider evidence did not satisfy the policy-renewal authority contract.",
                "Contact support; CMA did not sign or install another policy.",
                &claim,
                None,
            )
        })?;
        Ok(CapabilityAuthorityAdmission {
            snapshot,
            target_policy_generation,
            target_policy_expires_at_epoch_seconds,
        })
    }

    pub(crate) async fn revoke_capability(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRevocation, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        let credential_session_id = trigger_session_id(&launch.id).map_err(|_| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_credential_session_identity_invalid",
                "The Launch does not identify its reserved credential custody record.",
                "Contact support; CMA will not retire unrelated session authority.",
                &claim,
                None,
            )
        })?;
        let reach = launch.reach.as_ref().ok_or_else(|| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_reach_missing",
                "The Launch has no admitted capability record to revoke.",
                "Contact support; CMA cannot infer prior delegated access.",
                &claim,
                None,
            )
        })?;
        let generation = reach.policy_generation.checked_add(1).ok_or_else(|| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_reach_generation_exhausted",
                "The delegated-access generation cannot advance safely.",
                "Contact support; the session remains fail-closed at policy expiry.",
                &claim,
                None,
            )
        })?;
        let sessions = self
            .sessions
            .as_ref()
            .ok_or_else(|| execution_failure(unsupported_effect(&claim)))?;

        let mut first_failure = None;
        let mut runtime_audience = reach.runtime_audience.clone();
        let mut tombstone_acknowledged = false;
        match (launch.session.as_ref(), self.capability_signer.as_ref()) {
            (Some(session), Some(signer)) if session.session_id == credential_session_id => {
                let tombstone = RuntimePolicyTombstone {
                    version: POLICY_VERSION,
                    launch_id: launch.id.0.clone(),
                    session_id: session.session_id.0.clone(),
                    runtime_audience: String::new(),
                    generation,
                    fence,
                    issued_at_epoch_seconds: SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    issuer: "cma-trigger".into(),
                    audience: POLICY_AUDIENCE.into(),
                    signature_key_id: signer.key_id().into(),
                };
                match sessions
                    .install_runtime_tombstone(&session.session_id, tombstone, Arc::clone(signer))
                    .await
                {
                    Ok((audience, receipt))
                        if receipt.generation == generation
                            && receipt.fence == fence
                            && receipt.revoked =>
                    {
                        runtime_audience = audience;
                        tombstone_acknowledged = true;
                    }
                    Ok(_) => {
                        first_failure = Some(failure(
                            FailureClass::TerminalSecurity,
                            "trigger_revoke_acknowledgement_mismatch",
                            "The runtime acknowledged different revocation data than CMA issued.",
                            "Contact support; provider credential revocation remains the backstop.",
                            &claim,
                            None,
                        ));
                    }
                    Err(error) => {
                        first_failure = Some(execution_failure(session_failure(error, &claim)));
                    }
                }
            }
            (Some(_), _) => {
                first_failure = Some(failure(
                    FailureClass::TerminalSecurity,
                    "trigger_reach_session_mismatch",
                    "The retained session does not match this Launch's reserved identity.",
                    "Contact support; CMA skipped runtime mutation and is retiring only exact custody.",
                    &claim,
                    None,
                ));
            }
            (None, _) => {
                first_failure = Some(failure(
                    FailureClass::RetryableTransient,
                    "trigger_reach_session_missing",
                    "The runtime session was not committed before revocation began.",
                    "CMA is retiring exact credential custody without inventing runtime evidence.",
                    &claim,
                    None,
                ));
            }
        }

        let agent_key_revocation_confirmed = match retire_session_credential(
            sessions.as_ref(),
            &credential_session_id,
            &claim,
        )
        .await
        {
            Ok(()) => true,
            Err(error) => {
                first_failure.get_or_insert(error);
                false
            }
        };

        if !tombstone_acknowledged && !agent_key_revocation_confirmed {
            return Err(first_failure.unwrap_or_else(|| {
                failure(
                    FailureClass::RetryableTransient,
                    "trigger_revocation_pending",
                    "Neither runtime nor provider has acknowledged revocation yet.",
                    "CMA will retry both enforcement boundaries within the policy TTL.",
                    &claim,
                    None,
                )
            }));
        }
        Ok(CapabilityRevocation {
            generation,
            fence,
            runtime_audience,
            tombstone_acknowledged,
            agent_key_revocation_confirmed,
            credential_session_id,
        })
    }

    /// Retire exact session credential custody without claiming Reach existed.
    pub(crate) async fn retire_session_credential(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<SessionId, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        let session_id = trigger_session_id(&launch.id).map_err(|_| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_credential_session_identity_invalid",
                "The Launch does not identify its reserved credential custody record.",
                "Contact support; CMA will not retire unrelated session authority.",
                &claim,
                None,
            )
        })?;
        let sessions = self
            .sessions
            .as_ref()
            .ok_or_else(|| execution_failure(unsupported_effect(&claim)))?;
        retire_session_credential(sessions.as_ref(), &session_id, &claim).await?;
        Ok(session_id)
    }

    async fn run_preflight(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        match self.github.preflight(launch).await {
            Ok(authority) => {
                let mut resources = BTreeMap::new();
                resources.insert(
                    "repository_id".into(),
                    launch
                        .revision
                        .content
                        .upstream
                        .repository
                        .repository_id
                        .to_string(),
                );
                resources.insert("service_id".into(), authority.connection.service_id.clone());
                resources.insert("source_sha".into(), authority.source_sha.clone());
                EffectExecution::Completed {
                    result: LaunchEffectResult::Preflight {
                        connection: authority.connection,
                        authority: authority.provenance,
                        source_sha: authority.source_sha,
                        workflow_paths: authority.workflow_paths,
                    },
                    safe_detail: "GitHub authority and source revision verified.".into(),
                    external_resource_ids: resources,
                }
            }
            Err(error) => service_failure(error, claim),
        }
    }

    async fn run_fork(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        reconcile_only: bool,
    ) -> EffectExecution {
        let context = match ForkContext::try_from(launch) {
            Ok(context) => context,
            Err(error) => return broker_failure(error, claim, reconcile_only),
        };
        if !reconcile_only {
            if let Err(execution) = require_live_effect_authority_admission(launch, claim) {
                return *execution;
            }
        }
        let result = if reconcile_only {
            self.github.reconcile_fork(&context).await
        } else {
            self.github.create_fork(&context).await
        };
        match result {
            Ok(ForkReconciliation::Ready(fork)) => {
                let mut resources = retained_resources(launch, claim);
                resources.insert(
                    "fork_repository_id".into(),
                    fork.repository.repository_id.to_string(),
                );
                resources.insert(
                    "fork_repository_node_id".into(),
                    fork.repository.repository_node_id.clone(),
                );
                EffectExecution::Completed {
                    result: LaunchEffectResult::Fork { fork: *fork },
                    safe_detail: "Your GitHub fork is ready.".into(),
                    external_resource_ids: resources,
                }
            }
            Ok(ForkReconciliation::Pending { repository_id }) => self.defer_fork(
                launch,
                claim,
                repository_id,
                "fork_readiness_pending",
                "GitHub is still preparing the fork.",
            ),
            Ok(ForkReconciliation::Absent) if reconcile_only => self.defer_fork(
                launch,
                claim,
                None,
                "fork_outcome_pending",
                "Checking whether GitHub accepted the earlier fork request.",
            ),
            Ok(ForkReconciliation::Absent) => EffectExecution::Failed {
                failure: failure(
                    FailureClass::RecoveryRequired,
                    "fork_write_not_observed",
                    "GitHub did not expose the fork after the create request.",
                    "Review the retained provider evidence before authorizing another fork request.",
                    claim,
                    None,
                ),
            },
            Err(error) => broker_failure(error, claim, reconcile_only),
        }
    }

    fn defer_fork(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        repository_id: Option<u64>,
        code: &'static str,
        detail: &'static str,
    ) -> EffectExecution {
        let now = SystemTime::now();
        let now_seconds = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut resources = retained_resources(launch, claim);
        if let Some(repository_id) = repository_id {
            resources.insert("fork_repository_id".into(), repository_id.to_string());
        }
        let deadline = resources
            .get("fork_readiness_deadline")
            .and_then(|value| seconds_from_rfc3339(value))
            .unwrap_or_else(|| {
                now_seconds.saturating_add(self.fork_readiness_timeout.as_secs().max(1))
            });
        resources.insert(
            "fork_readiness_deadline".into(),
            rfc3339_at(SystemTime::UNIX_EPOCH + Duration::from_secs(deadline)),
        );
        if now_seconds >= deadline {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::RecoveryRequired,
                    "fork_readiness_deadline_exceeded",
                    "The fork outcome could not be confirmed within the readiness window.",
                    "Review the retained GitHub evidence before authorizing another write.",
                    claim,
                    None,
                ),
            };
        }

        let remaining = deadline.saturating_sub(now_seconds);
        let delay = fork_reconciliation_delay(launch, claim).min(remaining.max(1));
        let reconcile_at = rfc3339_at(now + Duration::from_secs(delay));
        EffectExecution::ReconcileLater {
            failure: failure(
                FailureClass::AmbiguousExternal,
                code,
                detail,
                "CMA will reconcile the same fork intent without repeating the write.",
                claim,
                Some(reconcile_at.clone()),
            ),
            reconcile_at,
            external_resource_ids: resources,
        }
    }

    async fn run_branch(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        reconcile_only: bool,
    ) -> EffectExecution {
        let context = match BranchContext::try_from(launch) {
            Ok(context) => context,
            Err(error) => return broker_failure(error, claim, reconcile_only),
        };
        if !reconcile_only {
            if let Err(execution) = require_live_effect_authority_admission(launch, claim) {
                return *execution;
            }
        }
        let result = if reconcile_only {
            self.github.reconcile_branch(&context).await
        } else {
            self.github.create_branch(&context).await
        };
        match result {
            Ok(BranchReconciliation::Ready(branch)) => {
                let mut resources = retained_resources(launch, claim);
                resources.insert("branch_ref".into(), format!("refs/heads/{}", branch.name));
                resources.insert("branch_head".into(), branch.current_head.clone());
                resources.insert(
                    "fork_repository_id".into(),
                    context.fork.repository.repository_id.to_string(),
                );
                EffectExecution::Completed {
                    result: LaunchEffectResult::Branch { branch },
                    safe_detail: "Your launch branch is pinned to the reviewed source.".into(),
                    external_resource_ids: resources,
                }
            }
            Ok(BranchReconciliation::Absent) if reconcile_only => {
                let mut resources = retained_resources(launch, claim);
                resources.insert(
                    "branch_ref".into(),
                    format!("refs/heads/{}", context.branch_name()),
                );
                reconcile_failure(
                    claim,
                    "branch_outcome_pending",
                    "Checking whether GitHub accepted the earlier branch request.",
                    resources,
                )
            }
            Ok(BranchReconciliation::Absent) => EffectExecution::Failed {
                failure: failure(
                    FailureClass::RecoveryRequired,
                    "branch_write_not_observed",
                    "GitHub did not expose the branch after the create request.",
                    "Review the retained provider evidence before authorizing another branch request.",
                    claim,
                    None,
                ),
            },
            Err(error) => broker_failure(error, claim, reconcile_only),
        }
    }

    async fn run_workspace(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let Some(store) = self.workspace_store.as_ref() else {
            return unsupported_effect(claim);
        };
        let context = match WorkspaceContext::try_from(launch) {
            Ok(context) => context,
            Err(error) => return broker_failure(error, claim, false),
        };
        match store
            .reconcile_baseline_import(&launch.owner, &launch.id, &launch.source_sha)
            .await
        {
            Ok(Some(completed)) => {
                if let Err(error) = store
                    .retire_baseline_import_chunks(&launch.owner, &launch.id)
                    .await
                {
                    return workspace_store_failure(error, claim);
                }
                return workspace_import_completion(completed, launch, claim);
            }
            Ok(None) => {}
            Err(error) => return workspace_store_failure(error, claim),
        }

        let source = match self.github.import_workspace(&context).await {
            Ok(source) => source,
            Err(error) => return broker_failure(error, claim, false),
        };
        if let Err(error) = store
            .begin_baseline_import(
                &launch.owner,
                &launch.id,
                &source.source_revision,
                &source.tree_digest,
                &source.blobs,
            )
            .await
        {
            return workspace_store_failure(error, claim);
        }

        let mut imported = BTreeMap::<String, WorkspaceBlobMetadata>::new();
        for blob in &source.blobs {
            let mut bytes = Vec::with_capacity(blob.bytes as usize);
            let mut chunks = Vec::with_capacity(blob.chunk_count as usize);
            let mut complete = true;
            for ordinal in 0..blob.chunk_count {
                let retained = match store
                    .get_baseline_import_chunk(
                        &launch.owner,
                        &launch.id,
                        &source.source_revision,
                        blob,
                        ordinal,
                    )
                    .await
                {
                    Ok(retained) => retained,
                    Err(error) => return workspace_store_failure(error, claim),
                };
                let Some(retained) = retained else {
                    complete = false;
                    break;
                };
                let expected = import_chunk_bytes(blob, ordinal);
                if u64::try_from(retained.len()).unwrap_or(u64::MAX) != expected {
                    return workspace_store_failure(
                        TriggerStoreError::Malformed(
                            "a staged baseline chunk has the wrong size".into(),
                        ),
                        claim,
                    );
                }
                chunks.push(BaselineImportChunk {
                    provider_object_id: blob.provider_object_id.clone(),
                    ordinal,
                    bytes: expected,
                    sha256: format!("{:x}", Sha256::digest(&retained)),
                });
                bytes.extend_from_slice(&retained);
            }

            if !complete {
                bytes = match self.github.read_workspace_blob(&context, blob).await {
                    Ok(bytes) => bytes,
                    Err(error) => return broker_failure(error, claim, false),
                };
                chunks.clear();
                for (ordinal, part) in bytes
                    .chunks(BASELINE_IMPORT_CHUNK_BYTES as usize)
                    .enumerate()
                {
                    let chunk = BaselineImportChunk {
                        provider_object_id: blob.provider_object_id.clone(),
                        ordinal: ordinal as u32,
                        bytes: part.len() as u64,
                        sha256: format!("{:x}", Sha256::digest(part)),
                    };
                    if let Err(error) = store
                        .put_baseline_import_chunk(
                            &launch.owner,
                            &launch.id,
                            &source.source_revision,
                            blob,
                            &chunk,
                            part,
                        )
                        .await
                    {
                        return workspace_store_failure(error, claim);
                    }
                    chunks.push(chunk);
                }
            }
            let needs_symlink_target = source.entries.iter().any(|entry| {
                entry.provider_object_id == blob.provider_object_id && entry.mode == 0o120000
            });
            let needs_gitmodules = source.entries.iter().any(|entry| {
                entry.provider_object_id == blob.provider_object_id && entry.path == ".gitmodules"
            });
            let metadata = match workspace_blob_metadata(
                blob,
                &bytes,
                chunks,
                needs_symlink_target,
                needs_gitmodules,
            ) {
                Ok(metadata) => metadata,
                Err(error) => return broker_failure(error, claim, false),
            };
            imported.insert(blob.provider_object_id.clone(), metadata);
        }

        let (manifest, plan) = match build_baseline_archive_plan(&source, &imported) {
            Ok(result) => result,
            Err(error) => return broker_failure(error, claim, false),
        };
        let completed = match store
            .finalize_baseline_import(&launch.owner, &launch.id, &manifest, plan)
            .await
        {
            Ok(completed) => completed,
            Err(error) => return workspace_store_failure(error, claim),
        };
        if let Err(error) = store
            .retire_baseline_import_chunks(&launch.owner, &launch.id)
            .await
        {
            return workspace_store_failure(error, claim);
        }
        workspace_import_completion(completed, launch, claim)
    }
}

fn workspace_import_completion(
    imported: BaselineImportOutcome,
    launch: &Launch,
    claim: &EffectClaim,
) -> EffectExecution {
    let now = SystemTime::now();
    let mut resources = retained_resources(launch, claim);
    resources.insert(
        "workspace_object_reference".into(),
        imported.object_reference.clone(),
    );
    resources.insert(
        "workspace_manifest_digest".into(),
        imported.manifest.digest.clone(),
    );
    resources.insert(
        "workspace_file_count".into(),
        imported.manifest.entries.len().to_string(),
    );
    EffectExecution::Completed {
        result: LaunchEffectResult::WorkspaceImport {
            workspace: WorkspaceRecord {
                baseline: imported.manifest,
                baseline_object_reference: imported.object_reference,
                baseline_archive_sha256: imported.archive_sha256,
                latest_checkpoint: None,
                previous_checkpoint: None,
                retired_checkpoint: None,
                sealed_contribution_checkpoint: None,
                checkpoint_intent: None,
                lifecycle_checkpoint: Default::default(),
                checkpoint_version: 0,
                workspace_epoch: 1,
                retention_policy_id: "trigger-workspace-v1".into(),
                delete_after: rfc3339_at(now + Duration::from_secs(30 * 24 * 60 * 60)),
            },
        },
        safe_detail: "The exact repository baseline is stored and verified.".into(),
        external_resource_ids: resources,
    }
}

fn import_chunk_bytes(blob: &BaselineImportBlob, ordinal: u32) -> u64 {
    blob.bytes
        .saturating_sub(u64::from(ordinal).saturating_mul(BASELINE_IMPORT_CHUNK_BYTES))
        .min(BASELINE_IMPORT_CHUNK_BYTES)
}

fn workspace_store_failure(error: TriggerStoreError, claim: &EffectClaim) -> EffectExecution {
    match error {
        TriggerStoreError::Unavailable(_) => transient_failure(
            claim,
            false,
            "workspace_storage_temporarily_unavailable",
            "The repository baseline could not be confirmed in durable storage.",
            None,
        ),
        _ => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "workspace_storage_integrity_failed",
                "The durable repository baseline failed its integrity contract.",
                "Contact support with the Launch correlation ID; no partial baseline was accepted.",
                claim,
                None,
            ),
        },
    }
}

impl GithubEffectExecutor {
    async fn run_sandbox(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let evidence = match self.ensure_trigger_session(launch, claim).await {
            Ok(evidence) => evidence,
            Err(execution) => return execution,
        };
        let mut resources = session_resources(launch, claim, &evidence.session);
        if evidence.session.status == SessionStatus::Rotating {
            return session_wait_later(
                "The repository runtime is rotating and is not ready for Launch work.",
                resources,
            );
        }
        match evidence.session.recovery_state {
            RecoveryState::Normal => {}
            RecoveryState::Recovering | RecoveryState::Unknown => {
                return session_wait_later(
                    "The repository runtime is recovering and is not ready for Launch work.",
                    resources,
                );
            }
            RecoveryState::Idle
            | RecoveryState::Degraded
            | RecoveryState::Retired
            | RecoveryState::Invalid => {
                return session_recovery_failure(claim);
            }
        }
        match evidence.session.status {
            SessionStatus::Ready | SessionStatus::Running
                if evidence.credential_authority.is_some() =>
            {
                let Some(sessions) = self.sessions.as_ref() else {
                    return unsupported_effect(claim);
                };
                let sandbox_id = match sessions.runtime_policy_audience(&evidence.session.id).await
                {
                    Ok(sandbox_id) => sandbox_id,
                    Err(error) => return session_failure(error, claim),
                };
                resources.insert("sandbox_id".into(), sandbox_id);
                EffectExecution::Completed {
                    result: LaunchEffectResult::Sandbox,
                    safe_detail: "The repository runtime is ready with scoped delegated access."
                        .into(),
                    external_resource_ids: resources,
                }
            }
            SessionStatus::Provisioning
            | SessionStatus::Rotating
            | SessionStatus::Ready
            | SessionStatus::Running => session_wait_later(
                "The repository runtime is still establishing its scoped access.",
                resources,
            ),
            SessionStatus::Idle | SessionStatus::Terminated => EffectExecution::Failed {
                failure: failure(
                    FailureClass::RecoveryRequired,
                    "trigger_runtime_not_live",
                    "The deterministic Trigger session stopped before it became ready.",
                    "Review the retained session failure before authorizing recovery.",
                    claim,
                    None,
                ),
            },
        }
    }

    async fn run_session(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let evidence = match self.ensure_trigger_session(launch, claim).await {
            Ok(evidence) => evidence,
            Err(execution) => return execution,
        };
        let resources = session_resources(launch, claim, &evidence.session);
        if evidence.session.status == SessionStatus::Rotating {
            return session_wait_later(
                "The deterministic session is rotating and cannot be attached yet.",
                resources,
            );
        }
        match evidence.session.recovery_state {
            RecoveryState::Normal => {}
            RecoveryState::Recovering | RecoveryState::Unknown => {
                return session_wait_later(
                    "The deterministic session is recovering and cannot be attached yet.",
                    resources,
                );
            }
            RecoveryState::Idle
            | RecoveryState::Degraded
            | RecoveryState::Retired
            | RecoveryState::Invalid => {
                return session_recovery_failure(claim);
            }
        }
        if !matches!(
            evidence.session.status,
            SessionStatus::Ready | SessionStatus::Running
        ) || evidence.credential_authority.is_none()
            || evidence.runtime_generation.is_none()
        {
            return session_wait_later(
                "The deterministic session is not yet safe to attach to this Launch.",
                resources,
            );
        }
        let Some(environment_profile_id) = evidence.session.environment_profile.as_ref() else {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_session_profile_missing",
                    "The deterministic session has no frozen Trigger environment profile.",
                    "Contact support; CMA will not dispatch a prompt into an unscoped session.",
                    claim,
                    None,
                ),
            };
        };
        EffectExecution::Completed {
            result: LaunchEffectResult::Session {
                session: SessionLink {
                    session_id: evidence.session.id,
                    environment_profile_id: environment_profile_id.0.clone(),
                    runtime_generation: evidence
                        .runtime_generation
                        .expect("validated as live above"),
                    created_at: evidence.session.created_at,
                },
            },
            safe_detail: "The Codex session is attached to this Launch.".into(),
            external_resource_ids: resources,
        }
    }

    async fn run_reach(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        capability_renewal: bool,
    ) -> EffectExecution {
        let sponsored_authority_write = launch.revision.content.commercial_policy.billing_mode
            == BillingMode::Sponsored
            && (capability_renewal || claim.mode == EffectClaimMode::Execute);
        let sponsored_runtime = if sponsored_authority_write {
            match self.sponsorship.as_ref() {
                Some(runtime) => Some(runtime),
                None => {
                    return authority_admission_failure(
                        claim,
                        "trigger_sponsorship_runtime_missing",
                        "This sponsored Launch has no configured grant activation runtime.",
                    )
                }
            }
        } else {
            None
        };
        let capability_admission = if capability_renewal {
            launch
                .capability_authority_admission
                .as_ref()
                .filter(|admission| admission.snapshot.fence == claim.fence)
        } else {
            None
        };
        if capability_renewal && capability_admission.is_none() {
            return authority_admission_failure(
                claim,
                "trigger_capability_authority_admission_missing",
                "The exact renewal fence has no committed capability authority admission.",
            );
        }
        if claim.mode == EffectClaimMode::Reconcile
            && !sponsored_authority_write
            && !capability_renewal
        {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::RecoveryRequired,
                    "trigger_reach_install_outcome_unknown",
                    "The runtime policy installation outcome is unknown.",
                    "Wait for the bounded policy TTL and review runtime evidence before authorizing another installation.",
                    claim,
                    None,
                ),
            };
        }
        let Some(sessions) = self.sessions.as_ref() else {
            return unsupported_effect(claim);
        };
        let Some(signer) = self.capability_signer.as_ref() else {
            return unsupported_effect(claim);
        };
        let (authority, reserved_generation, reserved_expiry, policy_fence) =
            if let Some(admission) = capability_admission {
                if admission
                    .snapshot
                    .validate_capability_for(
                        launch,
                        claim.fence,
                        admission.target_policy_generation,
                    )
                    .is_err()
                {
                    return authority_admission_failure(
                        claim,
                        "trigger_capability_authority_admission_invalid",
                        "The committed capability authority does not match this fence.",
                    );
                }
                (
                    authority_from_snapshot(&admission.snapshot, claim),
                    Some(admission.target_policy_generation),
                    Some(admission.target_policy_expires_at_epoch_seconds),
                    admission.snapshot.fence,
                )
            } else if claim.mode == EffectClaimMode::Reconcile {
                let admission =
                    match launch.historical_effect_authority_admission(claim) {
                        Ok(admission) => admission,
                        Err(_) => return authority_admission_failure(
                            claim,
                            "trigger_reach_reconciliation_admission_missing",
                            "The retained Reach intent has no valid original authority admission.",
                        ),
                    };
                (
                    authority_from_snapshot(admission, claim),
                    None,
                    None,
                    admission.fence,
                )
            } else {
                (
                    admitted_effect_authority(launch, claim),
                    None,
                    None,
                    claim.fence,
                )
            };
        let authority = match authority {
            Ok(authority) => authority,
            Err(execution) => return *execution,
        };
        let Some(session) = launch.session.as_ref() else {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_reach_session_missing",
                    "The Launch has no deterministic session to receive delegated access.",
                    "Contact support; CMA will not issue policy without an exact session binding.",
                    claim,
                    None,
                ),
            };
        };
        let Some(branch) = launch.branch.as_ref() else {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_reach_branch_missing",
                    "The Launch has no fenced branch for delegated GitHub access.",
                    "Contact support; CMA will not issue an unbound repository policy.",
                    claim,
                    None,
                ),
            };
        };
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let generation = reserved_generation.unwrap_or_else(|| {
            launch
                .reach
                .as_ref()
                .map_or(1, |reach| reach.policy_generation.saturating_add(1))
        });
        let (github_catalog_service_id, github_user_service_slug) =
            match authority.connection.github_binding() {
                Ok((catalog_service_id, user_service_slug)) => (
                    catalog_service_id.to_string(),
                    user_service_slug.to_string(),
                ),
                Err(_) => {
                    return authority_admission_failure(
                        claim,
                        "trigger_github_binding_missing",
                        "The committed Reach admission has no exact GitHub catalog route binding.",
                    )
                }
            };
        let Some(admitted_credential_authority) = authority
            .authority
            .session_credential_authority
            .as_ref()
            .cloned()
        else {
            return authority_admission_failure(
                claim,
                "trigger_session_credential_authority_missing",
                "The committed Reach admission has no exact session credential authority.",
            );
        };
        let current_credential_authority =
            match self.fresh_session_credential_authority(launch, claim).await {
                Ok(authority) => authority,
                Err(execution) => return execution,
            };
        if current_credential_authority != admitted_credential_authority {
            return authority_admission_failure(
                claim,
                "trigger_session_credential_authority_drift",
                "The session credential changed after the committed Reach admission.",
            );
        }
        let probe = layer3_adapter::RuntimeGatewayProbeRequest {
            version: layer3_adapter::RUNTIME_GATEWAY_PROBE_VERSION,
            service_id: authority.connection.service_id.clone(),
            catalog_service_id: github_catalog_service_id.clone(),
            user_service_slug: github_user_service_slug.clone(),
            resource_id: authority.fork.repository_id,
        };
        let (admitted, expires_at) = if claim.mode == EffectClaimMode::Reconcile
            && !capability_renewal
        {
            match sessions
                .reconcile_runtime_policy_and_probe(
                    &session.session_id,
                    &admitted_credential_authority,
                    probe,
                    &launch.id.0,
                    generation,
                    policy_fence,
                )
                .await
            {
                Ok(reconciled) => (reconciled.reachability, reconciled.expires_at_epoch_seconds),
                Err(error) => {
                    return runtime_gateway_reconciliation_failure(
                        error,
                        claim,
                        retained_resources(launch, claim),
                    )
                }
            }
        } else {
            let expires_at = reserved_expiry
                .unwrap_or_else(|| now.saturating_add(CAPABILITY_MAX_LIFETIME_SECONDS));
            if expires_at <= now {
                return EffectExecution::Failed {
                    failure: failure(
                        FailureClass::RecoveryRequired,
                        "trigger_capability_admission_expired",
                        "The admitted policy generation expired before installation completed.",
                        "CMA will reserve a later generation only after the prior TTL is safely past.",
                        claim,
                        None,
                    ),
                };
            }
            let capability = LaunchCapability {
                version: CAPABILITY_POLICY_VERSION,
                launch_id: launch.id.clone(),
                session_id: session.session_id.clone(),
                // Layer 2 fills this from the exact runtime handle while holding
                // the session lifecycle fence.
                runtime_audience: String::new(),
                service_id: authority.connection.service_id.clone(),
                github_catalog_service_id: github_catalog_service_id.clone(),
                github_user_service_slug: github_user_service_slug.clone(),
                upstream: authority.upstream.clone(),
                fork: authority.fork.clone(),
                allowed_ref: format!("refs/heads/{}", branch.name),
                operations: launch
                    .revision
                    .content
                    .repository_access
                    .allowed_operation_classes
                    .iter()
                    .copied()
                    .map(CapabilityOperation::Repository)
                    .collect(),
                generation,
                fence: policy_fence,
                not_before_epoch_seconds: now,
                expires_at_epoch_seconds: expires_at,
                issuer: "cma-trigger".into(),
                audience: POLICY_AUDIENCE.into(),
                signature_key_id: signer.key_id().to_string(),
                numeric_binding_observed_at: authority.authority.observed_at.clone(),
                revocation: CapabilityRevocationState::Active,
            };
            if let Err(execution) =
                require_live_policy_install_authority(launch, claim, capability_renewal)
            {
                return *execution;
            }
            match sessions
                .install_runtime_policy_and_probe(
                    &session.session_id,
                    &admitted_credential_authority,
                    capability,
                    Arc::clone(signer),
                    probe,
                )
                .await
            {
                Ok(installed) => (installed, expires_at),
                Err(error) => return runtime_gateway_reachability_failure(error, claim),
            }
        };
        let runtime_audience = admitted.runtime_audience;
        let receipt = admitted.policy;
        if receipt.generation != generation || receipt.fence != policy_fence || receipt.revoked {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_reach_acknowledgement_mismatch",
                    "The runtime acknowledged different delegated access than CMA issued.",
                    "Contact support; GitHub access remains fail-closed for this Launch.",
                    claim,
                    None,
                ),
            };
        }
        let mut resources = retained_resources(launch, claim);
        resources.insert("service_id".into(), authority.connection.service_id.clone());
        resources.insert("catalog_service_id".into(), github_catalog_service_id);
        resources.insert("user_service_slug".into(), github_user_service_slug);
        resources.insert("policy_generation".into(), generation.to_string());
        resources.insert("policy_fence".into(), policy_fence.to_string());
        resources.insert("runtime_audience".into(), runtime_audience.clone());
        resources.insert("policy_expires_at".into(), expires_at.to_string());
        resources.insert(
            "runtime_gateway_probe_version".into(),
            admitted.probe_version.to_string(),
        );
        resources.insert("runtime_gateway_probe_outcome".into(), "allowed".into());
        if let Some(sponsored_runtime) = sponsored_runtime {
            if let Err(execution) = self
                .activate_and_install_sponsored_model_authority(
                    launch,
                    claim,
                    sessions.as_ref(),
                    sponsored_runtime,
                    &admitted_credential_authority,
                    &runtime_audience,
                    &mut resources,
                )
                .await
            {
                return execution;
            }
        }
        EffectExecution::Completed {
            result: LaunchEffectResult::Reach {
                reach: layer2_core::ReachStatus {
                    service_id: authority.connection.service_id.clone(),
                    state: CapabilityRevocationState::Active,
                    policy_generation: generation,
                    policy_fence,
                    runtime_audience: runtime_audience.clone(),
                    expires_at_epoch_seconds: expires_at,
                    numeric_binding_observed_at: authority.authority.observed_at.clone(),
                    refreshed_at: authority.authority.observed_at.clone(),
                    revocation_requested_at: None,
                    revocation_effective_at: None,
                    last_refusal: None,
                    tombstone_acknowledged: false,
                    agent_key_revocation_confirmed: false,
                },
                connection: authority.connection,
                authority: authority.authority,
                branch: authority.branch,
            },
            safe_detail:
                "The runtime acknowledged its scoped policy and proved the restricted GitHub path."
                    .into(),
            external_resource_ids: resources,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn activate_and_install_sponsored_model_authority(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        sessions: &dyn TriggerSessionEffectPort,
        runtime: &SponsoredEffectRuntime,
        credential: &SessionCredentialAuthority,
        expected_runtime_audience: &str,
        resources: &mut BTreeMap<String, String>,
    ) -> Result<(), EffectExecution> {
        let session = launch.session.as_ref().ok_or_else(|| {
            authority_admission_failure(
                claim,
                "trigger_sponsored_session_missing",
                "The sponsored Launch has no exact session for grant activation.",
            )
        })?;
        let stable_request_id = sponsored_activation_request_id(launch, &session.session_id);
        let activated = runtime
            .sponsorships
            .activate_grant(
                &launch.owner,
                ActivateSponsoredGrant {
                    stable_request_id,
                    launch_id: launch.id.clone(),
                },
            )
            .await
            .map_err(|error| sponsorship_activation_failure(error, claim, resources.clone()))?;
        let capability = activated.capability;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if capability.validate().is_err()
            || capability.launch_id != launch.id
            || capability.session_id != session.session_id
            || seconds_from_rfc3339(&capability.expires_at).is_none_or(|expires| expires <= now)
        {
            return Err(EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_sponsored_capability_invalid",
                    "The activated grant did not match this exact live Launch and session.",
                    "Contact support; CMA did not install model authority or dispatch the prompt.",
                    claim,
                    None,
                ),
            });
        }
        let receipt = sessions
            .install_sponsored_model_authority(
                &session.session_id,
                credential,
                &capability,
                activated.maximum_output_tokens_per_request,
                &runtime.gateway_origin,
            )
            .await
            .map_err(|error| sponsored_model_authority_failure(error, claim, resources.clone()))?;
        if receipt.runtime_audience != expected_runtime_audience
            || receipt.session_id != session.session_id.0
            || receipt.session_credential_generation != credential.custody_generation
            || receipt.launch_id != launch.id.0
            || receipt.grant_id != capability.grant_id.0
            || receipt.grant_generation != capability.grant_generation
            || receipt.policy_generation != capability.policy_generation
            || receipt.maximum_output_tokens_per_request
                != activated.maximum_output_tokens_per_request
            || receipt.capability_fingerprint != capability.fingerprint
        {
            return Err(EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_sponsored_authority_acknowledgement_mismatch",
                    "The runtime acknowledged different sponsored model authority than CMA issued.",
                    "Contact support; model access and initial prompt dispatch remain fail-closed.",
                    claim,
                    None,
                ),
            });
        }
        resources.insert("sponsored_grant_id".into(), capability.grant_id.0);
        resources.insert(
            "sponsored_grant_generation".into(),
            capability.grant_generation.to_string(),
        );
        resources.insert(
            "sponsored_policy_generation".into(),
            capability.policy_generation.to_string(),
        );
        resources.insert("sponsored_model_authority".into(), "acknowledged".into());
        Ok(())
    }

    async fn ensure_trigger_session(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<TriggerSessionEvidence, EffectExecution> {
        let Some(sessions) = self.sessions.as_ref() else {
            return Err(unsupported_effect(claim));
        };
        sessions
            .ensure_session(launch)
            .await
            .map_err(|error| session_failure(error, claim))
    }

    async fn run_initial_dispatch(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let Some(sessions) = self.sessions.as_ref() else {
            return unsupported_effect(claim);
        };
        if launch
            .reach
            .as_ref()
            .is_none_or(|reach| reach.state != CapabilityRevocationState::Active)
        {
            return EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalSecurity,
                    "trigger_initial_prompt_reach_unproven",
                    "The exact Trigger runtime has no active restricted-gateway proof.",
                    "Run the Reach step successfully before dispatching the initial task.",
                    claim,
                    None,
                ),
            };
        }
        let evidence = match self.ensure_trigger_session(launch, claim).await {
            Ok(evidence) => evidence,
            Err(execution) => return execution,
        };
        let resources = session_resources(launch, claim, &evidence.session);
        if let Some(execution) =
            initial_dispatch_lifecycle_wait(claim, &evidence.session, &resources)
        {
            return execution;
        }
        let delivery = match sessions.dispatch_initial_prompt(launch, claim).await {
            Ok(delivery) => delivery,
            Err(CoreError::SessionRotating) => {
                return session_wait_later(
                    "The Trigger session began rotating before initial-task dispatch.",
                    resources,
                );
            }
            Err(error) => {
                // Lifecycle can change after the observation above but before
                // the session input fence is acquired. Re-observe before
                // classifying the error so rotation/recovery remains a
                // budget-neutral wait rather than a failed dispatch.
                if let Ok(evidence) = self.ensure_trigger_session(launch, claim).await {
                    let observed_resources = session_resources(launch, claim, &evidence.session);
                    if let Some(execution) = initial_dispatch_lifecycle_wait(
                        claim,
                        &evidence.session,
                        &observed_resources,
                    ) {
                        return execution;
                    }
                }
                return session_failure(error, claim);
            }
        };
        let result = LaunchEffectResult::InitialDispatch {
            delivery: delivery.clone(),
        };
        let mut resources = resources;
        resources.insert("prompt_origin_key".into(), delivery.origin_key.clone());
        resources.insert("prompt_payload_hash".into(), delivery.payload_hash.clone());
        if let Some(sequence) = delivery.dispatch_fence_sequence {
            resources.insert(
                "prompt_dispatch_fence_sequence".into(),
                sequence.to_string(),
            );
        }
        match delivery.state {
            layer2_core::PromptDeliveryState::Completed => EffectExecution::Completed {
                result,
                safe_detail: "The initial Trigger task reached a terminal runtime response.".into(),
                external_resource_ids: resources,
            },
            layer2_core::PromptDeliveryState::Queued => {
                let retry_at = rfc3339_at(SystemTime::now() + retry_delay(claim, None));
                EffectExecution::RetryWithProgress {
                    result,
                    failure: failure(
                        FailureClass::RetryableTransient,
                        "initial_prompt_pre_write_failure",
                        "The runtime input queue refused the initial task before any write.",
                        "CMA will retry the same immutable task after a bounded backoff.",
                        claim,
                        Some(retry_at.clone()),
                    ),
                    retry_at,
                    external_resource_ids: resources,
                }
            }
            layer2_core::PromptDeliveryState::WriteConfirmed
            | layer2_core::PromptDeliveryState::ExecutionObserved => {
                let reconcile_at = rfc3339_at(
                    SystemTime::now()
                        + Duration::from_secs(reconciliation_delay(claim.reconciliation_attempt)),
                );
                EffectExecution::ReconcileWithProgress {
                    result,
                    failure: failure(
                        FailureClass::RetryableTransient,
                        "initial_prompt_turn_pending",
                        "The initial Trigger task is waiting for its terminal runtime result.",
                        "CMA will observe the same session turn without sending the task again.",
                        claim,
                        Some(reconcile_at.clone()),
                    ),
                    reconcile_at,
                    external_resource_ids: resources,
                }
            }
            layer2_core::PromptDeliveryState::RecoveryRequired
            | layer2_core::PromptDeliveryState::DispatchFenced => {
                EffectExecution::FailedWithProgress {
                    result,
                    failure: failure(
                        FailureClass::RecoveryRequired,
                        "initial_prompt_recovery_required",
                        "CMA cannot prove whether the initial task reached the runtime.",
                        "Review the retained session evidence before explicitly authorizing a replacement dispatch; it may execute twice.",
                        claim,
                        None,
                    ),
                }
            }
            layer2_core::PromptDeliveryState::Failed => EffectExecution::FailedWithProgress {
                result,
                failure: failure(
                    FailureClass::PolicyUserAction,
                    "initial_prompt_execution_failed",
                    "The runtime reported that the initial Trigger task failed.",
                    "Open the retained session to review the failure before deciding how to continue.",
                    claim,
                    None,
                ),
            },
        }
    }

    async fn run_checkpoint(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let Some(sessions) = self.sessions.as_ref() else {
            return unsupported_effect(claim);
        };
        let Some(store) = self.checkpoint_store.as_ref() else {
            return unsupported_effect(claim);
        };
        let Some(workspace) = launch.workspace.as_ref() else {
            return checkpoint_failure(
                claim,
                FailureClass::TerminalSecurity,
                "checkpoint_workspace_missing",
                "The Launch has no imported workspace authority.",
                "Contact support; CMA will not infer checkpoint ownership.",
            );
        };
        let Some(session) = launch.session.as_ref() else {
            return checkpoint_failure(
                claim,
                FailureClass::RecoveryRequired,
                "checkpoint_session_missing",
                "The Launch has no runtime generation to checkpoint.",
                "Recover the retained Trigger session before contributing.",
            );
        };
        let Some(intent) = workspace.checkpoint_intent.as_ref() else {
            return checkpoint_failure(
                claim,
                FailureClass::RecoveryRequired,
                "checkpoint_intent_missing",
                "The workspace checkpoint has no durable version reservation.",
                "Request a fresh contribution review; CMA will not infer capture authority.",
            );
        };
        if intent.purpose != layer2_core::WorkspaceCheckpointPurpose::ContributionReview
            || launch.contribution_checkpoint_version != Some(intent.version)
        {
            return checkpoint_failure(
                claim,
                FailureClass::RecoveryRequired,
                "checkpoint_intent_mismatch",
                "The workspace checkpoint reservation does not match this contribution review.",
                "Request a fresh contribution review after the retained capture is reconciled.",
            );
        }
        let version = intent.version;
        let request = layer3_adapter::WorkspaceCheckpointRequest {
            epoch: workspace.workspace_epoch,
            version,
            source_revision: launch.source_sha.clone(),
            baseline_manifest_digest: workspace.baseline.digest.clone(),
            disposition: layer3_adapter::WorkspaceCheckpointDisposition::ResumeAfterCapture,
        };
        let (captured, runtime_generation) =
            match sessions.checkpoint_workspace(launch, &request).await {
                Ok(captured) => captured,
                Err(error) => return checkpoint_runtime_failure(error, claim),
            };
        if runtime_generation != intent.runtime_generation
            || runtime_generation != session.runtime_generation
            || captured.epoch != intent.workspace_epoch
            || captured.version != intent.version
            || !captured
                .source_revision
                .eq_ignore_ascii_case(&launch.source_sha)
            || captured.writer_state != layer3_adapter::WorkspaceWriterState::Resumed
            || captured.writer_resume_token.is_some()
        {
            return checkpoint_failure(
                claim,
                FailureClass::RecoveryRequired,
                "checkpoint_runtime_receipt_mismatch",
                "The runtime checkpoint receipt did not match the reserved session and workspace authority.",
                "The previous complete checkpoint remains authoritative; inspect the retained capture before recovery.",
            );
        }
        let observed_archive_sha256 = format!("{:x}", Sha256::digest(&captured.archive));
        if !observed_archive_sha256.eq_ignore_ascii_case(&captured.archive_sha256) {
            return checkpoint_failure(
                claim,
                FailureClass::RecoveryRequired,
                "checkpoint_archive_mismatch",
                "The captured workspace archive did not match its runtime-sealed digest.",
                "The previous complete checkpoint remains authoritative; inspect the runtime before recovery.",
            );
        }
        let entries = captured
            .entries
            .into_iter()
            .map(|entry| WorkspaceEntry {
                path: entry.path,
                kind: match entry.kind {
                    layer3_adapter::WorkspaceEntryKind::Regular => WorkspaceEntryKind::Regular,
                    layer3_adapter::WorkspaceEntryKind::Symlink => WorkspaceEntryKind::Symlink,
                },
                mode: entry.mode,
                size: entry.size,
                content_sha256: entry.content_sha256,
                provider_object_id: entry.provider_object_id,
                symlink_target: entry.symlink_target,
            })
            .collect::<Vec<_>>();
        let manifest = match WorkspaceManifest::seal(captured.source_revision.clone(), entries) {
            Ok(manifest)
                if manifest.digest == captured.manifest_digest
                    && manifest.total_bytes == captured.total_bytes =>
            {
                manifest
            }
            _ => {
                return checkpoint_failure(
                    claim,
                    FailureClass::RecoveryRequired,
                    "checkpoint_manifest_mismatch",
                    "The captured workspace manifest did not match the sealed archive.",
                    "The previous complete checkpoint remains authoritative; inspect the runtime before recovery.",
                )
            }
        };
        let created_at = rfc3339_at(SystemTime::now());
        let mut checkpoint = WorkspaceCheckpoint {
            id: intent.id.clone(),
            launch_id: launch.id.clone(),
            version,
            workspace_epoch: workspace.workspace_epoch,
            session_id: session.session_id.clone(),
            runtime_generation,
            object_reference: String::new(),
            archive_sha256: captured.archive_sha256,
            manifest,
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: created_at.clone(),
            delete_after: Some(workspace.delete_after.clone()),
        };
        let reference = match store
            .put_checkpoint(&launch.owner, &checkpoint, &captured.archive)
            .await
        {
            Ok(reference) => reference,
            Err(TriggerStoreError::Unavailable(_)) => {
                let retry_at = rfc3339_at(SystemTime::now() + retry_delay(claim, None));
                return EffectExecution::RetryLater {
                    failure: failure(
                        FailureClass::RetryableTransient,
                        "checkpoint_storage_unavailable",
                        "The sealed checkpoint could not yet be proven in durable storage.",
                        "CMA will request the same checkpoint version and destination object again.",
                        claim,
                        Some(retry_at.clone()),
                    ),
                    retry_at,
                    external_resource_ids: BTreeMap::new(),
                };
            }
            Err(_) => {
                return checkpoint_failure(
                    claim,
                    FailureClass::RecoveryRequired,
                    "checkpoint_storage_integrity_failed",
                    "Durable storage did not prove the complete checkpoint object.",
                    "The previous complete checkpoint remains authoritative; inspect storage evidence before recovery.",
                )
            }
        };
        checkpoint.object_reference = reference.clone();
        if checkpoint.validate_complete(&launch.id).is_err() {
            return checkpoint_failure(
                claim,
                FailureClass::RecoveryRequired,
                "checkpoint_commit_invalid",
                "The destination-proven checkpoint record failed its final validation.",
                "The previous complete checkpoint remains authoritative; contact support.",
            );
        }
        let mut resources = retained_resources(launch, claim);
        resources.insert("checkpoint_id".into(), checkpoint.id.0.clone());
        resources.insert("checkpoint_version".into(), version.to_string());
        resources.insert("checkpoint_object".into(), reference);
        resources.insert(
            "checkpoint_manifest_digest".into(),
            checkpoint.manifest.digest.clone(),
        );
        resources.insert(
            "checkpoint_archive_sha256".into(),
            checkpoint.archive_sha256.clone(),
        );
        resources.insert(
            "checkpoint_quiesced_writers".into(),
            captured.quiesced_writers.to_string(),
        );
        EffectExecution::Completed {
            result: LaunchEffectResult::Checkpoint { checkpoint },
            safe_detail:
                "The complete workspace was quiesced, sealed, and verified in durable storage."
                    .into(),
            external_resource_ids: resources,
        }
    }

    async fn run_diff(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let (Some(baseline_store), Some(checkpoint_store), Some(diff_store)) = (
            self.workspace_store.as_ref(),
            self.checkpoint_store.as_ref(),
            self.diff_store.as_ref(),
        ) else {
            return unsupported_effect(claim);
        };
        let Some(workspace) = launch.workspace.as_ref() else {
            return diff_failure(
                claim,
                FailureClass::TerminalSecurity,
                "diff_workspace_missing",
                "The Launch has no immutable workspace baseline.",
                "Contact support; CMA will not infer review authority.",
            );
        };
        let Some(checkpoint) = workspace.sealed_contribution_checkpoint.as_ref() else {
            return diff_failure(
                claim,
                FailureClass::RecoveryRequired,
                "diff_checkpoint_missing",
                "No complete checkpoint is available for review.",
                "Request a fresh contribution review after the session is recoverable.",
            );
        };
        if launch.contribution_checkpoint_version != Some(checkpoint.version) {
            return diff_failure(
                claim,
                FailureClass::RecoveryRequired,
                "diff_checkpoint_fence_mismatch",
                "The retained checkpoint is not the version selected for this review.",
                "Request a fresh contribution review; CMA will not review stale workspace bytes.",
            );
        }

        let authority = match self.github.preflight_capability(launch).await {
            Ok(authority) => authority,
            Err(error) => return broker_failure(error, claim, false),
        };
        let baseline = match baseline_store
            .get_baseline(
                &launch.owner,
                &launch.id,
                &workspace.baseline_object_reference,
                &workspace.baseline_archive_sha256,
            )
            .await
        {
            Ok(bytes) => bytes,
            Err(TriggerStoreError::Unavailable(_)) => {
                return transient_failure(
                    claim,
                    false,
                    "diff_baseline_temporarily_unavailable",
                    "The immutable baseline could not be read for review.",
                    None,
                )
            }
            Err(_) => None,
        };
        let current = match checkpoint_store
            .get_checkpoint(&launch.owner, checkpoint)
            .await
        {
            Ok(bytes) => bytes,
            Err(TriggerStoreError::Unavailable(_)) => {
                return transient_failure(
                    claim,
                    false,
                    "diff_checkpoint_temporarily_unavailable",
                    "The complete checkpoint could not be read for review.",
                    None,
                )
            }
            Err(_) => None,
        };

        let artifact_id = deterministic_diff_id(&claim.intent_key);
        let created_at = checkpoint.created_at.clone();
        let invalid = |outcome| {
            build_invalid_diff_artifact(
                launch,
                checkpoint,
                &authority.branch.current_head,
                self.changed_file_limit,
                artifact_id.clone(),
                created_at.clone(),
                outcome,
            )
        };
        let built = match (baseline.as_deref(), current.as_deref()) {
            (None, _) => invalid(DiffValidationOutcome::MissingBaseline),
            (Some(_), None) => invalid(DiffValidationOutcome::CorruptCheckpoint),
            (Some(baseline), Some(current)) => match build_diff_artifact(
                launch,
                checkpoint,
                baseline,
                current,
                &authority.branch.current_head,
                self.changed_file_limit,
                artifact_id.clone(),
                created_at.clone(),
            ) {
                Err(DiffBuildError::Corrupt(_)) => {
                    invalid(DiffValidationOutcome::CorruptCheckpoint)
                }
                Err(DiffBuildError::ArtifactTooLarge) => {
                    invalid(DiffValidationOutcome::ArtifactTooLarge)
                }
                other => other,
            },
        };
        let mut built = match built {
            Ok(built) => built,
            Err(
                DiffBuildError::Corrupt(_)
                | DiffBuildError::ArtifactTooLarge
                | DiffBuildError::Invalid(_),
            ) => {
                return diff_failure(
                    claim,
                    FailureClass::TerminalSecurity,
                    "diff_artifact_invalid",
                    "CMA could not represent the review artifact within its safety contract.",
                    "Contact support; no GitHub write is authorized.",
                );
            }
        };
        let reference = match diff_store
            .put_diff_artifact(
                &launch.owner,
                &launch.id,
                &built.artifact.id,
                &built.artifact.object_sha256,
                &built.object,
            )
            .await
        {
            Ok(reference) => reference,
            Err(TriggerStoreError::Unavailable(_)) => {
                return transient_failure(
                    claim,
                    false,
                    "diff_artifact_storage_unavailable",
                    "The review artifact could not yet be proven in durable storage.",
                    None,
                )
            }
            Err(_) => {
                return diff_failure(
                    claim,
                    FailureClass::TerminalSecurity,
                    "diff_artifact_storage_integrity_failed",
                    "Durable storage did not prove the exact review artifact.",
                    "Contact support; no GitHub write is authorized.",
                )
            }
        };
        built.artifact.object_reference = reference.clone();
        if built.artifact.validate_for_review(launch).is_err()
            || (built.artifact.validation == DiffValidationOutcome::Valid
                && built.artifact.validate_for_publication(launch).is_err())
        {
            return diff_failure(
                claim,
                FailureClass::TerminalSecurity,
                "diff_publication_preflight_invalid",
                "The completed review did not satisfy the publication boundary.",
                "Contact support; no GitHub write is authorized.",
            );
        }
        let mut resources = retained_resources(launch, claim);
        resources.insert("diff_artifact_id".into(), built.artifact.id.0.clone());
        resources.insert("diff_artifact_digest".into(), built.artifact.digest.clone());
        resources.insert("diff_artifact_object".into(), reference);
        resources.insert(
            "diff_changed_files".into(),
            built.artifact.total_changed_files.to_string(),
        );
        resources.insert(
            "diff_validation".into(),
            diff_validation_name(built.artifact.validation),
        );
        let validation = built.artifact.validation;
        EffectExecution::Completed {
            result: LaunchEffectResult::Diff {
                diff: built.artifact,
            },
            safe_detail: if validation == DiffValidationOutcome::Valid {
                "The exact workspace changes are sealed in a read-only review artifact.".into()
            } else {
                "The read-only review completed with a named validation outcome; no GitHub write is authorized."
                    .into()
            },
            external_resource_ids: resources,
        }
    }

    async fn run_publication(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let (Some(checkpoint_store), Some(diff_store)) =
            (self.checkpoint_store.as_ref(), self.diff_store.as_ref())
        else {
            return unsupported_effect(claim);
        };
        let Some(diff) = launch.diff.as_ref() else {
            return publication_failure(
                claim,
                FailureClass::TerminalValidation,
                "publication_diff_missing",
                "The Launch has no reviewed diff to publish.",
                "Request a fresh contribution review before publishing.",
            );
        };
        let Some(contribution) = launch.contribution.as_ref() else {
            return publication_failure(
                claim,
                FailureClass::TerminalSecurity,
                "publication_confirmation_missing",
                "The Launch has no explicit publication authority.",
                "Confirm the exact reviewed diff before any GitHub write.",
            );
        };
        if contribution.diff_artifact_id != diff.id
            || contribution.diff_digest != diff.digest
            || contribution.confirmation.snapshot_digest != diff.digest
            || diff.validate_for_publication(launch).is_err()
        {
            return publication_failure(
                claim,
                FailureClass::TerminalSecurity,
                "publication_authority_mismatch",
                "The publication intent does not match the exact reviewed diff.",
                "Request a fresh review; CMA will not infer publication authority.",
            );
        }
        let Some(checkpoint) = launch
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.sealed_contribution_checkpoint.as_ref())
            .filter(|checkpoint| checkpoint.id == diff.checkpoint_id)
        else {
            return publication_failure(
                claim,
                FailureClass::RecoveryRequired,
                "publication_checkpoint_missing",
                "The exact reviewed checkpoint is no longer retained.",
                "Request a fresh contribution review from the recoverable session.",
            );
        };

        let authority = if claim.mode == EffectClaimMode::Execute {
            match admitted_effect_authority(launch, claim) {
                Ok(authority) => authority,
                Err(execution) => return *execution,
            }
        } else {
            match self.github.preflight_capability(launch).await {
                Ok(authority) => authority,
                Err(error) => return broker_failure(error, claim, true),
            }
        };
        if authority.branch.current_head != contribution.expected_head
            && contribution
                .commit_id
                .as_ref()
                .is_none_or(|commit| authority.branch.current_head != *commit)
        {
            return broker_failure(GithubBrokerError::BranchHeadConflict, claim, false);
        }
        let artifact_object = match diff_store.get_diff_artifact(&launch.owner, diff).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return publication_failure(
                    claim,
                    FailureClass::RecoveryRequired,
                    "publication_artifact_missing",
                    "The exact reviewed artifact is no longer retained.",
                    "Request a fresh contribution review before publishing.",
                )
            }
            Err(TriggerStoreError::Unavailable(_)) => {
                return transient_failure(
                    claim,
                    claim.mode == EffectClaimMode::Reconcile,
                    "publication_artifact_temporarily_unavailable",
                    "The immutable review artifact is temporarily unavailable.",
                    None,
                )
            }
            Err(_) => {
                return publication_failure(
                    claim,
                    FailureClass::TerminalSecurity,
                    "publication_artifact_integrity_failed",
                    "The retained review artifact failed its storage boundary.",
                    "Contact support; CMA will not publish unverified bytes.",
                )
            }
        };
        let checkpoint_archive = match checkpoint_store
            .get_checkpoint(&launch.owner, checkpoint)
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return publication_failure(
                    claim,
                    FailureClass::RecoveryRequired,
                    "publication_checkpoint_object_missing",
                    "The exact reviewed checkpoint object is no longer retained.",
                    "Request a fresh contribution review from the recoverable session.",
                )
            }
            Err(TriggerStoreError::Unavailable(_)) => {
                return transient_failure(
                    claim,
                    claim.mode == EffectClaimMode::Reconcile,
                    "publication_checkpoint_temporarily_unavailable",
                    "The exact reviewed checkpoint is temporarily unavailable.",
                    None,
                )
            }
            Err(_) => {
                return publication_failure(
                    claim,
                    FailureClass::TerminalSecurity,
                    "publication_checkpoint_integrity_failed",
                    "The retained checkpoint failed its storage boundary.",
                    "Contact support; CMA will not publish unverified workspace bytes.",
                )
            }
        };
        let plan = match super::workspace_diff::build_publication_plan(
            launch,
            diff,
            &artifact_object,
            &checkpoint_archive,
        ) {
            Ok(plan) => plan,
            Err(_) => return publication_failure(
                claim,
                FailureClass::TerminalSecurity,
                "publication_plan_invalid",
                "The reviewed artifact and checkpoint no longer produce the same publication plan.",
                "Request a fresh review; CMA will not publish mismatched bytes.",
            ),
        };
        let context = match super::github::PublicationContext::try_from(launch) {
            Ok(context) => context,
            Err(error) => return broker_failure(error, claim, false),
        };
        let advance = match if claim.mode == EffectClaimMode::Reconcile {
            self.github
                .reconcile_branch_publication(&context, &plan)
                .await
        } else {
            if let Err(execution) = require_live_effect_authority_admission(launch, claim) {
                return *execution;
            }
            self.github.publish_branch(&context, &plan).await
        } {
            Ok(advance) => advance,
            Err(error) => {
                return broker_failure(error, claim, claim.mode == EffectClaimMode::Reconcile)
            }
        };
        let mut contribution = contribution.clone();
        let now = rfc3339_at(SystemTime::now());
        let published = match advance {
            super::github::PublicationAdvance::ObjectWritePending(pending) => {
                contribution.state = layer2_core::PublicationState::IntentRecorded;
                contribution.base_tree_id = Some(pending.base_tree_id);
                contribution.tree_id = pending.tree_id;
                contribution.commit_id = None;
                contribution.resulting_head = None;
                contribution.object_ids = pending.object_ids;
                contribution.object_write = Some(pending.object_write);
                contribution.updated_at = now;
                if contribution.validate_intent(&launch.id).is_err() {
                    return publication_failure(
                        claim,
                        FailureClass::TerminalSecurity,
                        "publication_object_intent_invalid",
                        "The retained Git object recipe did not satisfy the durable publication intent.",
                        "Contact support; CMA will not issue an object write.",
                    );
                }
                return EffectExecution::Progressed {
                    result: LaunchEffectResult::BranchPublication { contribution },
                    external_resource_ids: retained_resources(launch, claim),
                };
            }
            super::github::PublicationAdvance::ObjectsWritten(prepared) => {
                contribution.state = layer2_core::PublicationState::ObjectsWritten;
                contribution.base_tree_id = Some(prepared.base_tree_id);
                contribution.tree_id = Some(prepared.tree_id);
                contribution.commit_id = Some(prepared.commit_id);
                contribution.object_ids = prepared.object_ids;
                contribution.object_write = None;
                contribution.updated_at = now;
                if contribution.validate_intent(&launch.id).is_err() {
                    return publication_failure(
                        claim,
                        FailureClass::TerminalSecurity,
                        "publication_object_evidence_invalid",
                        "GitHub object evidence did not satisfy the durable publication intent.",
                        "Contact support; CMA will not advance the launch ref.",
                    );
                }
                return EffectExecution::Progressed {
                    result: LaunchEffectResult::BranchPublication { contribution },
                    external_resource_ids: retained_resources(launch, claim),
                };
            }
            super::github::PublicationAdvance::RefUpdatePending(prepared) => {
                contribution.state = layer2_core::PublicationState::RefUpdatePending;
                contribution.base_tree_id = Some(prepared.base_tree_id);
                contribution.tree_id = Some(prepared.tree_id);
                contribution.commit_id = Some(prepared.commit_id);
                contribution.object_ids = prepared.object_ids;
                contribution.updated_at = now;
                if contribution.validate_intent(&launch.id).is_err() {
                    return publication_failure(
                        claim,
                        FailureClass::TerminalSecurity,
                        "publication_ref_intent_invalid",
                        "The guarded ref intent did not retain complete Git object evidence.",
                        "Contact support; CMA will not advance the launch ref.",
                    );
                }
                return EffectExecution::Progressed {
                    result: LaunchEffectResult::BranchPublication { contribution },
                    external_resource_ids: retained_resources(launch, claim),
                };
            }
            super::github::PublicationAdvance::Published(published) => published,
        };
        contribution.state = layer2_core::PublicationState::Published;
        contribution.base_tree_id = Some(published.base_tree_id.clone());
        contribution.tree_id = Some(published.tree_id.clone());
        contribution.commit_id = Some(published.commit_id.clone());
        contribution.resulting_head = Some(published.resulting_head.clone());
        contribution.object_ids = published.object_ids.clone();
        contribution.updated_at = now;
        if contribution.validate_intent(&launch.id).is_err() {
            return publication_failure(
                claim,
                FailureClass::TerminalSecurity,
                "publication_result_invalid",
                "GitHub publication evidence did not satisfy the durable intent.",
                "Contact support; CMA will not infer a successful branch update.",
            );
        }
        let mut resources = retained_resources(launch, claim);
        resources.insert("base_tree_id".into(), published.base_tree_id);
        resources.insert("tree_id".into(), published.tree_id);
        resources.insert("commit_id".into(), published.commit_id.clone());
        resources.insert("resulting_head".into(), published.resulting_head);
        EffectExecution::Completed {
            result: LaunchEffectResult::BranchPublication { contribution },
            safe_detail:
                "The reviewed bytes were committed and the launch branch advanced without force."
                    .into(),
            external_resource_ids: resources,
        }
    }

    async fn run_pull_request(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        reconcile_only: bool,
    ) -> EffectExecution {
        let context = match super::github::PullRequestContext::try_from(launch) {
            Ok(context) => context,
            Err(error) => return broker_failure(error, claim, reconcile_only),
        };
        if !reconcile_only {
            if let Err(execution) = require_live_effect_authority_admission(launch, claim) {
                return *execution;
            }
        }
        let result = if reconcile_only {
            self.github.reconcile_pull_request(&context).await
        } else {
            self.github.write_pull_request(&context).await
        };
        match result {
            Ok(super::github::PullRequestAdvance::Ready(pull_request)) => {
                let mut resources = retained_resources(launch, claim);
                if let Some(id) = pull_request.provider_id {
                    resources.insert("pull_request_provider_id".into(), id.to_string());
                }
                if let Some(node_id) = pull_request.provider_node_id.as_ref() {
                    resources.insert("pull_request_node_id".into(), node_id.clone());
                }
                if let Some(number) = pull_request.number {
                    resources.insert("pull_request_number".into(), number.to_string());
                }
                if let Some(url) = pull_request.canonical_url.as_ref() {
                    resources.insert("pull_request_url".into(), url.clone());
                }
                resources.insert(
                    "pull_request_head".into(),
                    pull_request.intent.expected_head_revision.clone(),
                );
                EffectExecution::Completed {
                    result: LaunchEffectResult::PullRequest {
                        pull_request: *pull_request,
                    },
                    safe_detail: "The cross-fork pull request was reconciled by its CMA marker and exact repository refs."
                        .into(),
                    external_resource_ids: resources,
                }
            }
            Ok(super::github::PullRequestAdvance::Absent) => {
                if reconcile_only {
                    reconcile_failure(
                        claim,
                        "pull_request_outcome_pending",
                        "Checking whether GitHub accepted the earlier pull-request request.",
                        retained_resources(launch, claim),
                    )
                } else {
                    EffectExecution::Failed {
                        failure: failure(
                            FailureClass::RecoveryRequired,
                            "pull_request_write_not_observed",
                            "GitHub did not expose the pull request after the create request.",
                            "Review the retained marker and branch evidence before authorizing another create.",
                            claim,
                            None,
                        ),
                    }
                }
            }
            Err(error) => broker_failure(error, claim, reconcile_only),
        }
    }

    async fn run_cleanup(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let Some(objects) = retained_cleanup_objects(launch, claim) else {
            return invalid_cleanup_plan(claim);
        };
        if objects.iter().any(|object| {
            matches!(
                object,
                RetainedCleanupObject::Baseline { .. } | RetainedCleanupObject::BaselineImport
            ) && self.workspace_store.is_none()
                || matches!(object, RetainedCleanupObject::Checkpoint(_))
                    && self.checkpoint_store.is_none()
                || matches!(object, RetainedCleanupObject::CheckpointIntent(_))
                    && self.checkpoint_store.is_none()
                || matches!(object, RetainedCleanupObject::DiffArtifact(_))
                    && self.diff_store.is_none()
        }) {
            return unsupported_effect(claim);
        }

        for object in objects {
            let result = match object {
                RetainedCleanupObject::Baseline { reference } => {
                    let store = self
                        .workspace_store
                        .as_ref()
                        .expect("cleanup stores were validated before deletion");
                    store
                        .delete_baseline(&launch.owner, &launch.id, &reference)
                        .await
                }
                RetainedCleanupObject::BaselineImport => {
                    let store = self
                        .workspace_store
                        .as_ref()
                        .expect("cleanup stores were validated before deletion");
                    store
                        .delete_baseline_import(&launch.owner, &launch.id)
                        .await
                }
                RetainedCleanupObject::Checkpoint(checkpoint) => {
                    let store = self
                        .checkpoint_store
                        .as_ref()
                        .expect("cleanup stores were validated before deletion");
                    store.delete_checkpoint(&launch.owner, &checkpoint).await
                }
                RetainedCleanupObject::CheckpointIntent(intent) => {
                    let store = self
                        .checkpoint_store
                        .as_ref()
                        .expect("cleanup stores were validated before deletion");
                    store
                        .delete_checkpoint_intent(&launch.owner, &launch.id, &intent)
                        .await
                }
                RetainedCleanupObject::DiffArtifact(artifact) => {
                    let store = self
                        .diff_store
                        .as_ref()
                        .expect("cleanup stores were validated before deletion");
                    store.delete_diff_artifact(&launch.owner, &artifact).await
                }
            };
            if let Err(error) = result {
                return cleanup_store_failure(error, claim);
            }
        }

        let targets = launch
            .cleanup
            .as_ref()
            .expect("cleanup validation requires a retained plan")
            .plan
            .targets
            .clone();
        EffectExecution::Completed {
            result: LaunchEffectResult::Cleanup {
                receipt: CleanupReceipt {
                    targets: targets.clone(),
                    completed_at: rfc3339_at(SystemTime::now()),
                },
            },
            safe_detail: "CMA-owned retained workspace objects were deleted.".into(),
            external_resource_ids: BTreeMap::from([(
                "cleanup_target_count".into(),
                targets.len().to_string(),
            )]),
        }
    }

    async fn observe_linked_pull_request_status(
        &self,
        launch: &Launch,
        correlation_id: &str,
    ) -> PullRequestStatusDisposition {
        let (result, provider_request_id) = observe_provider_requests(async {
            let context = PullRequestContext::for_status(launch)?;
            self.github.observe_pull_request(&context).await
        })
        .await;
        match result {
            Ok(pull_request) => {
                let next_attempt_at = (pull_request.state == PullRequestState::Open).then(|| {
                    rfc3339_at(
                        SystemTime::now()
                            + Duration::from_secs(PULL_REQUEST_STATUS_INTERVAL_SECONDS),
                    )
                });
                PullRequestStatusDisposition::Observed {
                    pull_request: Box::new(pull_request),
                    next_attempt_at,
                    provider_request_id,
                }
            }
            Err(error) => {
                pull_request_status_failure(error, launch, correlation_id, provider_request_id)
            }
        }
    }

    async fn admit_effect(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<Option<AuthorityAdmissionSnapshot>, EffectExecution> {
        if !effect_requires_authority_admission(claim.kind) {
            return Ok(None);
        }
        if claim.mode != EffectClaimMode::Execute {
            return Err(authority_admission_failure(
                claim,
                "trigger_authority_admission_mode_invalid",
                "Only a fresh executable claim can request authority admission.",
            ));
        }
        let mut evidence = self
            .github
            .preflight_effect_authority(launch, claim.kind)
            .await
            .map_err(|error| broker_failure(error, claim, false))?;
        if claim.kind == LaunchEffectKind::Reach {
            evidence.authority.session_credential_authority = Some(
                self.fresh_session_credential_authority(launch, claim)
                    .await?,
            );
        }
        let observed_at = evidence.authority.observed_at.clone();
        let admission = AuthorityAdmissionSnapshot::for_effect(
            launch,
            claim,
            evidence.connection,
            evidence.authority,
            evidence.upstream,
            evidence.fork,
            evidence.branch,
            observed_at,
        )
        .map_err(|_| {
            authority_admission_failure(
                claim,
                "trigger_authority_admission_invalid",
                "Fresh provider evidence did not satisfy the durable authority contract.",
            )
        })?;
        Ok(Some(admission))
    }
}

#[async_trait]
impl EffectExecutor for GithubEffectExecutor {
    fn refuse_execute(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
        correlation_id: &str,
    ) -> Option<LaunchFailure> {
        if claim.kind == LaunchEffectKind::Cleanup
            || super::has_current_trigger_runtime_admission(launch)
        {
            return None;
        }
        Some(LaunchFailure {
            class: FailureClass::TerminalSecurity,
            code: "launch_runtime_route_quarantined".into(),
            detail: "The retained Launch runtime route is not authorized for new Trigger effects."
                .into(),
            remediation: "CMA will preserve reconciliation evidence, revoke remaining session authority, and permit only fenced cleanup on this historical Launch. Open a New Launch to continue."
                .into(),
            correlation_id: correlation_id.into(),
            retry_at: None,
            observed_at: rfc3339_at(SystemTime::now()),
        })
    }

    async fn refresh_pull_request_status(
        &self,
        launch: &Launch,
        correlation_id: &str,
    ) -> PullRequestStatusDisposition {
        self.observe_linked_pull_request_status(launch, correlation_id)
            .await
    }

    async fn admit(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<Option<AuthorityAdmissionSnapshot>, EffectExecution> {
        let (result, provider_request_id) =
            observe_provider_requests(self.admit_effect(launch, claim)).await;
        result.map_err(|execution| execution.with_provider_request_id(provider_request_id))
    }

    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let (execution, provider_request_id) = observe_provider_requests(async {
            match claim.kind {
                LaunchEffectKind::Preflight => self.run_preflight(launch, claim).await,
                LaunchEffectKind::Fork => self.run_fork(launch, claim, false).await,
                LaunchEffectKind::Branch => self.run_branch(launch, claim, false).await,
                LaunchEffectKind::WorkspaceImport => self.run_workspace(launch, claim).await,
                LaunchEffectKind::Sandbox => self.run_sandbox(launch, claim).await,
                LaunchEffectKind::Session => self.run_session(launch, claim).await,
                LaunchEffectKind::Reach => self.run_reach(launch, claim, false).await,
                LaunchEffectKind::InitialDispatch => self.run_initial_dispatch(launch, claim).await,
                LaunchEffectKind::Checkpoint => self.run_checkpoint(launch, claim).await,
                LaunchEffectKind::Diff => self.run_diff(launch, claim).await,
                LaunchEffectKind::BranchPublication => self.run_publication(launch, claim).await,
                LaunchEffectKind::PullRequest => self.run_pull_request(launch, claim, false).await,
                LaunchEffectKind::Cleanup => self.run_cleanup(launch, claim).await,
            }
        })
        .await;
        execution.with_provider_request_id(provider_request_id)
    }

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let (execution, provider_request_id) = observe_provider_requests(async {
            match claim.kind {
                LaunchEffectKind::Preflight => self.run_preflight(launch, claim).await,
                LaunchEffectKind::Fork => self.run_fork(launch, claim, true).await,
                LaunchEffectKind::Branch => self.run_branch(launch, claim, true).await,
                // Every provider call is a read or an idempotent write to the
                // Launch's stable object key; repeating it reconciles the same bytes.
                LaunchEffectKind::WorkspaceImport => self.run_workspace(launch, claim).await,
                LaunchEffectKind::Sandbox => self.run_sandbox(launch, claim).await,
                LaunchEffectKind::Session => self.run_session(launch, claim).await,
                LaunchEffectKind::Reach => self.run_reach(launch, claim, false).await,
                LaunchEffectKind::InitialDispatch => self.run_initial_dispatch(launch, claim).await,
                LaunchEffectKind::Checkpoint => self.run_checkpoint(launch, claim).await,
                // Diff generation performs only fenced reads plus an immutable,
                // content-addressed object write, so the same intent is safely
                // repeatable during reconciliation.
                LaunchEffectKind::Diff => self.run_diff(launch, claim).await,
                LaunchEffectKind::BranchPublication => self.run_publication(launch, claim).await,
                LaunchEffectKind::PullRequest => self.run_pull_request(launch, claim, true).await,
                // Deletion uses owner/Launch-derived object keys and treats an
                // absent object as success, so reconciliation repeats the exact
                // retained plan after any ambiguous or partial attempt.
                LaunchEffectKind::Cleanup => self.run_cleanup(launch, claim).await,
            }
        })
        .await;
        execution.with_provider_request_id(provider_request_id)
    }
}

fn initial_dispatch_lifecycle_wait(
    claim: &EffectClaim,
    session: &Session,
    resources: &BTreeMap<String, String>,
) -> Option<EffectExecution> {
    if session.status == SessionStatus::Rotating {
        return Some(session_wait_later(
            "The Trigger session is rotating; its initial task remains fenced and unsent.",
            resources.clone(),
        ));
    }
    match session.recovery_state {
        RecoveryState::Normal => {}
        RecoveryState::Recovering | RecoveryState::Unknown => {
            return Some(session_wait_later(
                "The Trigger session is recovering; CMA will not send or re-send its initial task.",
                resources.clone(),
            ));
        }
        RecoveryState::Idle
        | RecoveryState::Degraded
        | RecoveryState::Retired
        | RecoveryState::Invalid => return Some(session_recovery_failure(claim)),
    }
    match session.status {
        SessionStatus::Ready | SessionStatus::Running => None,
        SessionStatus::Provisioning | SessionStatus::Rotating => Some(session_wait_later(
            "The Trigger session is not yet interactive; its initial task remains unsent.",
            resources.clone(),
        )),
        SessionStatus::Idle | SessionStatus::Terminated => Some(session_recovery_failure(claim)),
    }
}

fn pull_request_status_failure(
    error: GithubBrokerError,
    launch: &Launch,
    correlation_id: &str,
    provider_request_id: Option<String>,
) -> PullRequestStatusDisposition {
    use GithubBrokerError as Error;

    let correlation_id = if correlation_id.trim().is_empty() {
        format!("cma-trigger:{}:pull-request-status", launch.id.0)
    } else {
        correlation_id.to_string()
    };
    let now = SystemTime::now();
    let observed_at = rfc3339_at(now);
    let retry = |class: FailureClass,
                 code: &'static str,
                 detail: &'static str,
                 remediation: &'static str,
                 retry_after: Option<&str>| {
        let next_attempt_at =
            rfc3339_at(now + pull_request_status_retry_delay(launch, retry_after));
        PullRequestStatusDisposition::Retry {
            failure: LaunchFailure {
                class,
                code: code.into(),
                detail: detail.into(),
                remediation: remediation.into(),
                correlation_id: correlation_id.clone(),
                retry_at: Some(next_attempt_at.clone()),
                observed_at: observed_at.clone(),
            },
            next_attempt_at,
            provider_request_id: provider_request_id.clone(),
        }
    };
    let suspended = |class: FailureClass,
                     code: &'static str,
                     detail: &'static str,
                     remediation: &'static str| {
        PullRequestStatusDisposition::Suspended {
            failure: LaunchFailure {
                class,
                code: code.into(),
                detail: detail.into(),
                remediation: remediation.into(),
                correlation_id: correlation_id.clone(),
                retry_at: None,
                observed_at: observed_at.clone(),
            },
            provider_request_id: provider_request_id.clone(),
        }
    };
    let fail_launch = |class: FailureClass,
                       code: &'static str,
                       detail: &'static str,
                       remediation: &'static str| {
        PullRequestStatusDisposition::FailLaunch {
            failure: LaunchFailure {
                class,
                code: code.into(),
                detail: detail.into(),
                remediation: remediation.into(),
                correlation_id: correlation_id.clone(),
                retry_at: None,
                observed_at: observed_at.clone(),
            },
            provider_request_id: provider_request_id.clone(),
        }
    };

    match error {
        Error::RateLimited { retry_after } => retry(
            FailureClass::ResourceQuota,
            "github_rate_limited",
            "GitHub temporarily rate-limited the pull-request status read.",
            "CMA will retry this read-only observation after the persisted provider backoff.",
            retry_after.as_deref(),
        ),
        Error::Transient { .. } => retry(
            FailureClass::RetryableTransient,
            "github_temporarily_unavailable",
            "GitHub pull-request status is temporarily unavailable.",
            "CMA will retry the same read-only observation without changing the pull request.",
            None,
        ),
        Error::OwnerCredentialUnavailable => fail_launch(
            FailureClass::OwnerCredentialUnavailable,
            "owner_credential_unavailable",
            "CMA no longer holds usable sign-in authority for this status read.",
            "Sign in again, then refresh the retained pull-request status.",
        ),
        Error::ProviderAuthenticationUnavailable | Error::InactiveReconnectUnavailable => fail_launch(
            FailureClass::AuthRequired,
            "github_provider_authentication_unavailable",
            "This CMA deployment cannot complete GitHub authorization safely.",
            "Use another active GitHub connection, or enable the exact service in NyxID and then refresh.",
        ),
        Error::ConnectionInactive => fail_launch(
            FailureClass::ServiceInactive,
            "github_service_inactive",
            "The selected GitHub connection is inactive.",
            "Enable the exact service in NyxID or choose another active connection, then refresh.",
        ),
        Error::UnapprovedScope { .. } => fail_launch(
            FailureClass::TerminalSecurity,
            "github_scope_unapproved",
            "The selected GitHub connection grants a scope outside CMA's approved authority ceiling.",
            "Reconnect with only CMA-approved GitHub scopes before retrying this status read.",
        ),
        Error::ConnectionNotFound
        | Error::ConnectionDisconnected
        | Error::InsufficientScope { .. } => fail_launch(
            FailureClass::AuthRequired,
            "github_authorization_required",
            "The selected GitHub connection no longer authorizes this status read.",
            "Reconnect or select an eligible active GitHub connection, then refresh.",
        ),
        Error::PullRequestChanged => {
            fail_launch(
                FailureClass::TerminalSecurity,
                "pull_request_identity_changed",
                "The linked pull-request identity changed outside CMA's retained record.",
                "Contact support; CMA will not bind this Launch to another pull request.",
            )
        }
        Error::BranchHeadConflict | Error::UpstreamBaseConflict => {
            suspended(
                FailureClass::ConflictStaleHead,
                "pull_request_status_stale",
                "The retained branch or base no longer matches this Launch.",
                "Review the current GitHub pull request; CMA will not overwrite or reset manually changed refs.",
            )
        }
        Error::NotFound => suspended(
            FailureClass::ConflictStaleHead,
            "github_pull_request_not_found",
            "The exact linked GitHub pull request no longer exists or is no longer visible.",
            "Review the retained GitHub link and connection access; CMA will not create a replacement automatically.",
        ),
        Error::AuthorityDrift | Error::Malformed(_) | Error::AmbiguousWrite => fail_launch(
            FailureClass::TerminalSecurity,
            "github_pull_request_evidence_invalid",
            "GitHub status evidence no longer matches the retained repository authority.",
            "Contact support with the Launch correlation ID; CMA retained the link and performed no provider write.",
        ),
        Error::ConnectionNotPersonal
        | Error::Forbidden
        | Error::ForkingDisabled
        | Error::ForkNetworkOccupied
        | Error::UnsupportedWorkspace(_)
        | Error::Rejected { .. } => suspended(
            FailureClass::PolicyUserAction,
            "github_status_policy_refused",
            "GitHub policy refused this pull-request status read.",
            "Review the selected account and repository policy, then request another status refresh.",
        ),
    }
}

fn pull_request_status_retry_delay(launch: &Launch, retry_after: Option<&str>) -> Duration {
    if let Some(seconds) = retry_after
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    {
        return Duration::from_secs(seconds.min(MAX_RETRY_AFTER));
    }
    let failures = launch
        .pull_request
        .as_ref()
        .map_or(0, |record| record.status_sync.consecutive_failures);
    Duration::from_secs(2_u64.saturating_pow(failures.min(8).saturating_add(1)))
        .min(MAX_PROVIDER_RETRY)
}

fn retained_cleanup_objects(
    launch: &Launch,
    claim: &EffectClaim,
) -> Option<Vec<RetainedCleanupObject>> {
    if claim.kind != LaunchEffectKind::Cleanup || !launch.state.is_terminal() {
        return None;
    }
    if launch
        .reach
        .as_ref()
        .is_some_and(|reach| reach.state != CapabilityRevocationState::Revoked)
    {
        return None;
    }
    let cleanup = launch.cleanup.as_ref()?;
    if cleanup.completed_at.is_some() {
        return None;
    }
    let Some(workspace) = launch.workspace.as_ref() else {
        let retained_import = launch
            .effects
            .iter()
            .any(|effect| effect.kind == LaunchEffectKind::WorkspaceImport);
        if !retained_import
            || launch.diff.is_some()
            || cleanup.plan.retention_policy_id != STAGING_IMPORT_RETENTION_POLICY_ID
            || launch.terminal_at.as_ref() != Some(&cleanup.plan.delete_after)
            || cleanup.plan.targets != [CleanupTarget::Baseline]
        {
            return None;
        }
        return Some(vec![RetainedCleanupObject::BaselineImport]);
    };
    if cleanup.plan.retention_policy_id != workspace.retention_policy_id
        || cleanup.plan.delete_after != workspace.delete_after
    {
        return None;
    }

    let mut expected = Vec::new();
    if let Some(diff) = launch.diff.as_ref() {
        expected.push(CleanupTarget::DiffArtifact {
            artifact_id: diff.id.clone(),
        });
    }
    let mut checkpoint_ids = std::collections::BTreeSet::new();
    for checkpoint in [
        workspace.retired_checkpoint.as_ref(),
        workspace.previous_checkpoint.as_ref(),
        workspace.latest_checkpoint.as_ref(),
        workspace.sealed_contribution_checkpoint.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if checkpoint_ids.insert(checkpoint.id.clone()) {
            expected.push(CleanupTarget::Checkpoint {
                checkpoint_id: checkpoint.id.clone(),
            });
        }
    }
    if let Some(intent) = workspace.checkpoint_intent.as_ref() {
        if checkpoint_ids.insert(intent.id.clone()) {
            expected.push(CleanupTarget::Checkpoint {
                checkpoint_id: intent.id.clone(),
            });
        }
    }
    expected.push(CleanupTarget::Baseline);
    if cleanup.plan.targets != expected {
        return None;
    }

    let mut objects = Vec::with_capacity(expected.len());
    for target in &expected {
        match target {
            CleanupTarget::Baseline => {
                objects.push(RetainedCleanupObject::Baseline {
                    reference: workspace.baseline_object_reference.clone(),
                });
            }
            CleanupTarget::Checkpoint { checkpoint_id } => {
                let checkpoint = [
                    workspace.retired_checkpoint.as_ref(),
                    workspace.previous_checkpoint.as_ref(),
                    workspace.latest_checkpoint.as_ref(),
                    workspace.sealed_contribution_checkpoint.as_ref(),
                ]
                .into_iter()
                .flatten()
                .find(|checkpoint| &checkpoint.id == checkpoint_id)
                .cloned();
                if let Some(checkpoint) = checkpoint {
                    objects.push(RetainedCleanupObject::Checkpoint(checkpoint));
                } else {
                    let intent = workspace
                        .checkpoint_intent
                        .as_ref()
                        .filter(|intent| &intent.id == checkpoint_id)
                        .cloned()?;
                    objects.push(RetainedCleanupObject::CheckpointIntent(intent));
                }
            }
            CleanupTarget::DiffArtifact { artifact_id } => {
                let artifact = launch
                    .diff
                    .as_ref()
                    .filter(|artifact| &artifact.id == artifact_id)
                    .cloned()?;
                objects.push(RetainedCleanupObject::DiffArtifact(artifact));
            }
        }
    }
    Some(objects)
}

fn invalid_cleanup_plan(claim: &EffectClaim) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(
            FailureClass::TerminalSecurity,
            "cleanup_plan_invalid",
            "The retained cleanup plan does not match this Launch's CMA-owned objects.",
            "Contact support; no retained object was deleted.",
            claim,
            None,
        ),
    }
}

fn cleanup_store_failure(error: TriggerStoreError, claim: &EffectClaim) -> EffectExecution {
    match error {
        TriggerStoreError::Unavailable(_) => transient_failure(
            claim,
            false,
            "cleanup_store_temporarily_unavailable",
            "A CMA-owned retained object could not be deleted yet.",
            None,
        ),
        _ => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "cleanup_store_contract_invalid",
                "The retained cleanup object did not satisfy its tenant-bound storage contract.",
                "Contact support; CMA stopped cleanup without widening the deletion target.",
                claim,
                None,
            ),
        },
    }
}

fn checkpoint_failure(
    claim: &EffectClaim,
    class: FailureClass,
    code: &'static str,
    detail: &'static str,
    remediation: &'static str,
) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(class, code, detail, remediation, claim, None),
    }
}

fn diff_failure(
    claim: &EffectClaim,
    class: FailureClass,
    code: &'static str,
    detail: &'static str,
    remediation: &'static str,
) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(class, code, detail, remediation, claim, None),
    }
}

fn publication_failure(
    claim: &EffectClaim,
    class: FailureClass,
    code: &'static str,
    detail: &'static str,
    remediation: &'static str,
) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(class, code, detail, remediation, claim, None),
    }
}

fn diff_validation_name(outcome: DiffValidationOutcome) -> String {
    match outcome {
        DiffValidationOutcome::Valid => "valid",
        DiffValidationOutcome::NoChanges => "no_changes",
        DiffValidationOutcome::StaleHead => "stale_head",
        DiffValidationOutcome::WorkflowScopeRequired => "workflow_scope_required",
        DiffValidationOutcome::TooManyChanges => "too_many_changes",
        DiffValidationOutcome::TooManyBytes => "too_many_bytes",
        DiffValidationOutcome::UnsupportedFile => "unsupported_file",
        DiffValidationOutcome::MissingBaseline => "missing_baseline",
        DiffValidationOutcome::CorruptCheckpoint => "corrupt_checkpoint",
        DiffValidationOutcome::ArtifactTooLarge => "artifact_too_large",
    }
    .into()
}

fn checkpoint_runtime_failure(error: CoreError, claim: &EffectClaim) -> EffectExecution {
    let (class, code, detail, remediation) = match error {
        CoreError::CapabilityUnsupported(_) | CoreError::SizeOutOfBounds(_) => (
            FailureClass::TerminalValidation,
            "checkpoint_runtime_unsupported",
            "The current runtime image cannot prove coherent workspace checkpoints.",
            "Move the session to an eligible runtime image before contributing.",
        ),
        CoreError::RuntimeUnavailable(_) => (
            FailureClass::RecoveryRequired,
            "checkpoint_runtime_lost",
            "The runtime disappeared before checkpoint quiescence and capture were proven.",
            "Recover from the previous complete checkpoint; CMA will not treat an ambiguous capture as current.",
        ),
        CoreError::RuntimeRefused(_) => (
            FailureClass::RecoveryRequired,
            "checkpoint_quiescence_unproven",
            "The runtime could not prove writer quiescence, sealing, and resume.",
            "Keep the previous complete checkpoint and inspect the still-retained runtime before recovery.",
        ),
        _ => (
            FailureClass::TerminalSecurity,
            "checkpoint_authority_mismatch",
            "The runtime checkpoint request no longer matches the Launch authority.",
            "Contact support; CMA will not capture or mount a workspace under mismatched authority.",
        ),
    };
    checkpoint_failure(claim, class, code, detail, remediation)
}

fn capability_claim(launch: &Launch, fence: u64) -> EffectClaim {
    EffectClaim {
        kind: LaunchEffectKind::Reach,
        mode: layer2_core::EffectClaimMode::Reconcile,
        intent_key: format!(
            "launch:{}:capability:{}:{}",
            launch.id,
            launch
                .reach
                .as_ref()
                .map_or(1, |reach| reach.policy_generation.saturating_add(1)),
            fence
        ),
        attempt: 1,
        reconciliation_attempt: 0,
        fence,
    }
}

async fn retire_session_credential(
    sessions: &dyn TriggerSessionEffectPort,
    session_id: &SessionId,
    claim: &EffectClaim,
) -> Result<(), LaunchFailure> {
    let release_failure = sessions
        .retire_runtime_credential(session_id)
        .await
        .err()
        .map(|error| execution_failure(session_failure(error, claim)));
    match sessions
        .runtime_credential_retirement_settled(session_id)
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(release_failure.unwrap_or_else(|| {
            failure(
                FailureClass::RetryableTransient,
                "trigger_credential_retirement_pending",
                "Session credential custody has not confirmed provider retirement yet.",
                "CMA will retry the same deterministic custody record.",
                claim,
                None,
            )
        })),
        Err(error) => {
            Err(release_failure.unwrap_or_else(|| execution_failure(session_failure(error, claim))))
        }
    }
}

fn execution_failure(execution: EffectExecution) -> LaunchFailure {
    match execution {
        EffectExecution::RetryLater { failure, .. }
        | EffectExecution::ReconcileLater { failure, .. }
        | EffectExecution::RetryWithProgress { failure, .. }
        | EffectExecution::ReconcileWithProgress { failure, .. }
        | EffectExecution::FailedWithProgress { failure, .. }
        | EffectExecution::Failed { failure } => failure,
        EffectExecution::Completed { .. }
        | EffectExecution::Progressed { .. }
        | EffectExecution::WaitLater { .. } => LaunchFailure {
            class: FailureClass::TerminalSecurity,
            code: "trigger_capability_result_mismatch".into(),
            detail: "The capability adapter returned an unexpected result.".into(),
            remediation: "Contact support; delegated access remains fail-closed.".into(),
            correlation_id: "trigger-capability-result".into(),
            retry_at: None,
            observed_at: rfc3339_at(SystemTime::now()),
        },
    }
}

fn session_resources(
    launch: &Launch,
    claim: &EffectClaim,
    session: &Session,
) -> BTreeMap<String, String> {
    let mut resources = retained_resources(launch, claim);
    resources.insert("session_id".into(), session.id.0.clone());
    if let Some(profile) = session.environment_profile.as_ref() {
        resources.insert("environment_profile_id".into(), profile.0.clone());
    }
    resources.insert(
        "runtime_generation".into(),
        u64::from(session.rotations).saturating_add(1).to_string(),
    );
    resources
}

fn session_wait_later(
    detail: &'static str,
    external_resource_ids: BTreeMap<String, String>,
) -> EffectExecution {
    EffectExecution::WaitLater {
        wait_at: rfc3339_at(SystemTime::now() + Duration::from_secs(2)),
        safe_detail: detail.into(),
        remediation:
            "CMA will check the same deterministic session again without creating another one."
                .into(),
        external_resource_ids,
    }
}

fn session_recovery_failure(claim: &EffectClaim) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(
            FailureClass::RecoveryRequired,
            "trigger_session_recovery_required",
            "The deterministic Trigger session is not in a usable recovery state.",
            "Review the retained session evidence before authorizing recovery.",
            claim,
            None,
        ),
    }
}

fn session_failure(error: CoreError, claim: &EffectClaim) -> EffectExecution {
    let (class, code, detail, remediation) = match error {
        CoreError::SessionLimitReached { .. } | CoreError::DeploymentAtCapacity { .. } => (
            FailureClass::ResourceQuota,
            "trigger_session_quota",
            "The Trigger session cannot currently reserve the requested capacity.",
            "Stop unused work or wait for deployment capacity, then retry this Launch.",
        ),
        CoreError::ServiceNotGranted(_) => (
            FailureClass::AuthRequired,
            "github_service_not_granted",
            "The exact GitHub connection is no longer granted to this contributor.",
            "Reconnect the same GitHub service, then resume this Launch.",
        ),
        CoreError::RuntimeUnavailable(_)
        | CoreError::ServiceCatalogUnavailable(_)
        | CoreError::SettingsUnavailable(_) => (
            FailureClass::RetryableTransient,
            "trigger_session_temporarily_unavailable",
            "The deterministic Trigger session could not be reconciled yet.",
            "CMA will retry this exact session identity after a bounded backoff.",
        ),
        CoreError::SizeOutOfBounds(_) | CoreError::CapabilityUnsupported(_) => (
            FailureClass::TerminalValidation,
            "trigger_runtime_contract_unsupported",
            "The selected Trigger runtime configuration is not supported by this deployment.",
            "Choose a supported Trigger configuration or update the eligible runtime cohort.",
        ),
        _ => (
            FailureClass::TerminalSecurity,
            "trigger_session_authority_mismatch",
            "The deterministic Trigger session did not match its frozen Launch authority.",
            "Contact support; CMA will not overwrite or replace the conflicting resource.",
        ),
    };
    if class == FailureClass::RetryableTransient || class == FailureClass::ResourceQuota {
        let retry_at = rfc3339_at(SystemTime::now() + retry_delay(claim, None));
        EffectExecution::RetryLater {
            failure: failure(
                class,
                code,
                detail,
                remediation,
                claim,
                Some(retry_at.clone()),
            ),
            retry_at,
            external_resource_ids: BTreeMap::new(),
        }
    } else {
        EffectExecution::Failed {
            failure: failure(class, code, detail, remediation, claim, None),
        }
    }
}

fn runtime_gateway_reachability_failure(
    error: layer2_core::RuntimeGatewayReachabilityError,
    claim: &EffectClaim,
) -> EffectExecution {
    use layer2_core::RuntimeGatewayReachabilityError as ReachabilityError;

    let terminal = |class, code, detail, remediation| EffectExecution::Failed {
        failure: failure(class, code, detail, remediation, claim, None),
    };
    match error {
        ReachabilityError::CommandSandboxInitialization => terminal(
            FailureClass::TerminalValidation,
            "trigger_runtime_command_sandbox_initialization_failed",
            "The Trigger runtime could not initialize its confined command sandbox.",
            "Update the eligible runtime image; CMA did not dispatch the initial task.",
        ),
        ReachabilityError::GatewayNetworkInitialization => terminal(
            FailureClass::TerminalValidation,
            "trigger_runtime_gateway_network_initialization_failed",
            "The Trigger runtime could not initialize its loopback-only gateway path.",
            "Update the eligible runtime image or sandbox network configuration.",
        ),
        ReachabilityError::PolicyDenied => terminal(
            FailureClass::TerminalSecurity,
            "trigger_runtime_policy_denied",
            "The runtime's signed policy denied the required repository read.",
            "Review the frozen repository, ref, operation, and runtime policy bindings.",
        ),
        ReachabilityError::ProviderDenied => terminal(
            FailureClass::AuthRequired,
            "trigger_runtime_provider_denied",
            "GitHub denied the scoped repository read from this Trigger runtime.",
            "Reconnect the same GitHub service with access to the fork, then resume the Launch.",
        ),
        ReachabilityError::TransientBrokerFailure => transient_failure(
            claim,
            false,
            "trigger_runtime_broker_temporarily_unavailable",
            "The restricted GitHub broker path is temporarily unavailable.",
            None,
        ),
        ReachabilityError::RuntimeUnavailable => transient_failure(
            claim,
            false,
            "trigger_runtime_gateway_unavailable",
            "The exact Trigger runtime did not answer the restricted gateway proof.",
            None,
        ),
        ReachabilityError::ProtocolUnsupported => terminal(
            FailureClass::TerminalValidation,
            "trigger_runtime_gateway_probe_unsupported",
            "The eligible runtime image does not support the required gateway proof protocol.",
            "Roll out a runtime that advertises runtime_gateway_probe_v2 before enabling Trigger admission.",
        ),
        ReachabilityError::ProtocolViolation => terminal(
            FailureClass::TerminalSecurity,
            "trigger_runtime_gateway_probe_mismatch",
            "The runtime gateway proof did not match CMA's exact request contract.",
            "Contact support; CMA kept delegated access and prompt dispatch fail-closed.",
        ),
        ReachabilityError::CredentialAuthorityDrift => terminal(
            FailureClass::TerminalSecurity,
            "trigger_session_credential_authority_drift",
            "The runtime credential authority changed at the policy-install boundary.",
            "Retry only after a fresh Reach authority admission is committed.",
        ),
        ReachabilityError::Core(error) => session_failure(error, claim),
    }
}

fn runtime_gateway_reconciliation_failure(
    error: layer2_core::RuntimeGatewayReachabilityError,
    claim: &EffectClaim,
    resources: BTreeMap<String, String>,
) -> EffectExecution {
    use layer2_core::RuntimeGatewayReachabilityError as ReachabilityError;

    match error {
        ReachabilityError::TransientBrokerFailure | ReachabilityError::RuntimeUnavailable => {
            sponsored_reconciliation(
                claim,
                resources,
                FailureClass::RetryableTransient,
                "trigger_reach_reconciliation_temporarily_unavailable",
                "The exact runtime policy and restricted route could not be observed yet.",
            )
        }
        ReachabilityError::Core(
            CoreError::RuntimeUnavailable(_)
            | CoreError::ServiceCatalogUnavailable(_)
            | CoreError::SettingsUnavailable(_),
        ) => sponsored_reconciliation(
            claim,
            resources,
            FailureClass::RetryableTransient,
            "trigger_reach_reconciliation_temporarily_unavailable",
            "The exact runtime policy and restricted route could not be observed yet.",
        ),
        other => runtime_gateway_reachability_failure(other, claim),
    }
}

fn sponsored_activation_request_id(launch: &Launch, session_id: &SessionId) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cma-trigger-sponsored-activation-v1\0");
    for binding in [
        launch.id.0.as_str(),
        launch.revision.id.0.as_str(),
        launch.revision_digest.as_str(),
        launch.effective.digest.as_str(),
        session_id.0.as_str(),
    ] {
        digest.update((binding.len() as u64).to_be_bytes());
        digest.update(binding.as_bytes());
    }
    format!("trigger-sponsored-activation-v1:{:x}", digest.finalize())
}

fn sponsorship_activation_failure(
    error: SponsorshipError,
    claim: &EffectClaim,
    resources: BTreeMap<String, String>,
) -> EffectExecution {
    match error {
        SponsorshipError::BillingAmbiguous
        | SponsorshipError::ProviderAmbiguous
        | SponsorshipError::ProviderOperationInProgress
        | SponsorshipError::ActivationInProgress => sponsored_reconciliation(
            claim,
            resources,
            FailureClass::AmbiguousExternal,
            "trigger_sponsored_activation_ambiguous",
            "Sponsored grant activation has an unresolved external outcome.",
        ),
        SponsorshipError::BillingUnavailable
        | SponsorshipError::ProviderUnavailable
        | SponsorshipError::Disabled
        | SponsorshipError::KillSwitch
        | SponsorshipError::CapabilityEvidenceMissing
        | SponsorshipError::InvalidGate
        | SponsorshipError::PolicyUnavailable
        | SponsorshipError::CredentialUnavailable
        | SponsorshipError::ProviderPolicyDenied
        | SponsorshipError::PricingUnavailable
        | SponsorshipError::PricingUnsupported => sponsored_reconciliation(
            claim,
            resources,
            FailureClass::RetryableTransient,
            "trigger_sponsored_activation_unavailable",
            "Sponsored grant activation is currently unavailable under the deployment policy.",
        ),
        SponsorshipError::Store(
            SponsorshipStoreError::Unavailable(_)
            | SponsorshipStoreError::Conflict
            | SponsorshipStoreError::StaleGeneration
            | SponsorshipStoreError::LaunchQuotaExceeded
            | SponsorshipStoreError::AggregateQuotaExceeded
            | SponsorshipStoreError::LaunchBudgetExceeded
            | SponsorshipStoreError::AggregateBudgetExceeded
            | SponsorshipStoreError::RateExceeded
            | SponsorshipStoreError::ActivePolicyLimit
            | SponsorshipStoreError::SponsorLaunchRateExceeded
            | SponsorshipStoreError::ConcurrentLaunchLimit,
        ) => sponsored_reconciliation(
            claim,
            resources,
            FailureClass::RetryableTransient,
            "trigger_sponsored_activation_capacity_pending",
            "Sponsored grant activation is waiting for durable capacity or reconciliation.",
        ),
        SponsorshipError::TermsNotAccepted | SponsorshipError::TermsChanged => {
            EffectExecution::Failed {
                failure: failure(
                    FailureClass::PolicyUserAction,
                    "trigger_sponsored_terms_changed",
                    "The current sponsored terms are not accepted for this exact Launch.",
                    "Open a New Launch and accept the current terms before Start.",
                    claim,
                    None,
                ),
            }
        }
        SponsorshipError::BillingRejected | SponsorshipError::ProviderRejected => {
            EffectExecution::Failed {
                failure: failure(
                    FailureClass::PolicyUserAction,
                    "trigger_sponsored_activation_rejected",
                    "The sponsor's billing or provider policy rejected this grant.",
                    "Contact the sponsor or open a Launch using contributor billing.",
                    claim,
                    None,
                ),
            }
        }
        SponsorshipError::GrantUnavailable
        | SponsorshipError::Store(
            SponsorshipStoreError::GateChanged
            | SponsorshipStoreError::LaunchAuthorityChanged
            | SponsorshipStoreError::AcceptanceChanged
            | SponsorshipStoreError::GrantUnavailable,
        ) => EffectExecution::Failed {
            failure: failure(
                FailureClass::PolicyUserAction,
                "trigger_sponsored_authority_changed",
                "The sponsorship authority changed before grant activation completed.",
                "Open a New Launch against the current sponsorship policy.",
                claim,
                None,
            ),
        },
        SponsorshipError::NotFound
        | SponsorshipError::Forbidden
        | SponsorshipError::ScopeDenied
        | SponsorshipError::Validation(_)
        | SponsorshipError::Store(SponsorshipStoreError::Malformed(_)) => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "trigger_sponsored_activation_scope_mismatch",
                "The sponsored grant activation did not match this exact Launch authority.",
                "Contact support; CMA did not install model authority or dispatch the prompt.",
                claim,
                None,
            ),
        },
    }
}

fn sponsored_model_authority_failure(
    error: SponsoredModelAuthorityError,
    claim: &EffectClaim,
    resources: BTreeMap<String, String>,
) -> EffectExecution {
    match error {
        SponsoredModelAuthorityError::RuntimeUnavailable
        | SponsoredModelAuthorityError::CredentialAuthorityUnavailable => sponsored_reconciliation(
            claim,
            resources,
            FailureClass::RetryableTransient,
            "trigger_sponsored_authority_install_pending",
            "The exact runtime did not acknowledge sponsored model authority yet.",
        ),
        SponsoredModelAuthorityError::ProtocolUnsupported => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalValidation,
                "trigger_sponsored_authority_protocol_unsupported",
                "The eligible runtime does not support sponsored model authority.",
                "Roll out an eligible runtime before enabling sponsored Launch admission.",
                claim,
                None,
            ),
        },
        SponsoredModelAuthorityError::ScopeMismatch
        | SponsoredModelAuthorityError::ProtocolViolation
        | SponsoredModelAuthorityError::CredentialAuthorityDrift => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "trigger_sponsored_authority_install_mismatch",
                "The runtime or credential no longer matches the activated sponsored grant.",
                "Contact support; model access and initial prompt dispatch remain fail-closed.",
                claim,
                None,
            ),
        },
    }
}

fn sponsored_reconciliation(
    claim: &EffectClaim,
    resources: BTreeMap<String, String>,
    class: FailureClass,
    code: &'static str,
    detail: &'static str,
) -> EffectExecution {
    let delay = reconciliation_delay(claim.reconciliation_attempt);
    let reconcile_at = rfc3339_at(SystemTime::now() + Duration::from_secs(delay));
    EffectExecution::ReconcileLater {
        failure: failure(
            class,
            code,
            detail,
            "CMA will reconcile the same activation and will not mint a second grant.",
            claim,
            Some(reconcile_at.clone()),
        ),
        reconcile_at,
        external_resource_ids: resources,
    }
}

fn retained_resources(launch: &Launch, claim: &EffectClaim) -> BTreeMap<String, String> {
    launch
        .effects
        .iter()
        .find(|record| record.kind == claim.kind && record.intent_key == claim.intent_key)
        .map(|record| record.external_resource_ids.clone())
        .unwrap_or_default()
}

fn reconciliation_delay(attempt: u32) -> u64 {
    2_u64
        .saturating_pow(attempt.min(4).saturating_add(1))
        .clamp(2, 30)
}

fn fork_reconciliation_delay(launch: &Launch, claim: &EffectClaim) -> u64 {
    let upper = reconciliation_delay(claim.reconciliation_attempt);
    let lower = upper.div_ceil(2);
    let mut digest = Sha256::new();
    digest.update(launch.id.0.as_bytes());
    digest.update([0]);
    digest.update(claim.intent_key.as_bytes());
    digest.update([0]);
    digest.update(claim.reconciliation_attempt.to_be_bytes());
    let bytes = digest.finalize();
    let sample = u64::from_be_bytes(bytes[..8].try_into().expect("SHA-256 has eight bytes"));
    lower + sample % (upper - lower + 1)
}

fn retry_delay(claim: &EffectClaim, retry_after: Option<&str>) -> Duration {
    if let Some(seconds) = retry_after
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    {
        return Duration::from_secs(seconds.min(MAX_RETRY_AFTER));
    }
    let seconds = 2_u64
        .saturating_pow(claim.attempt.min(8).saturating_add(1))
        .max(2);
    Duration::from_secs(seconds).min(MAX_PROVIDER_RETRY)
}

fn broker_failure(
    error: GithubBrokerError,
    claim: &EffectClaim,
    reconcile_only: bool,
) -> EffectExecution {
    use GithubBrokerError as Error;
    match error {
        Error::AmbiguousWrite => reconcile_failure(
            claim,
            "github_write_ambiguous",
            "GitHub did not confirm whether it accepted the request.",
            BTreeMap::new(),
        ),
        Error::RateLimited { retry_after } => {
            let retry_at = rfc3339_at(SystemTime::now() + retry_delay(claim, retry_after.as_deref()));
            let failure = failure(
                FailureClass::ResourceQuota,
                "github_rate_limited",
                "GitHub temporarily rate-limited this step.",
                "CMA will retry after the persisted provider backoff.",
                claim,
                Some(retry_at.clone()),
            );
            if reconcile_only {
                EffectExecution::ReconcileLater {
                    failure,
                    reconcile_at: retry_at,
                    external_resource_ids: BTreeMap::new(),
                }
            } else {
                EffectExecution::RetryLater {
                    failure,
                    retry_at,
                    external_resource_ids: BTreeMap::new(),
                }
            }
        }
        Error::Transient { status } => transient_failure(
            claim,
            reconcile_only,
            "github_temporarily_unavailable",
            if status.is_some() {
                "GitHub temporarily rejected this step."
            } else {
                "GitHub could not be reached at a known-safe boundary."
            },
            None,
        ),
        Error::OwnerCredentialUnavailable => EffectExecution::Failed {
            failure: failure(
                FailureClass::OwnerCredentialUnavailable,
                "owner_credential_unavailable",
                "CMA no longer holds usable sign-in authority for this operation.",
                "Sign in again to resume the same Launch.",
                claim,
                None,
            ),
        },
        Error::ProviderAuthenticationUnavailable | Error::InactiveReconnectUnavailable => {
            EffectExecution::Failed {
                failure: failure(
                    FailureClass::AuthRequired,
                    "github_provider_authentication_unavailable",
                    "This CMA deployment cannot complete GitHub authorization safely.",
                    "Use an active GitHub connection or a CMA deployment with provider-backed sign-in enabled.",
                    claim,
                    None,
                ),
            }
        }
        Error::ConnectionInactive => EffectExecution::Failed {
            failure: failure(
                FailureClass::ServiceInactive,
                "github_service_inactive",
                "The selected GitHub connection is inactive.",
                "Activate or reconnect the same GitHub connection, then resume this Launch.",
                claim,
                None,
            ),
        },
        Error::UnapprovedScope { .. } => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "github_scope_unapproved",
                "The selected GitHub connection grants a scope outside CMA's approved authority ceiling.",
                "Reconnect with only CMA-approved GitHub scopes before resuming this Launch.",
                claim,
                None,
            ),
        },
        Error::ConnectionNotFound
        | Error::ConnectionDisconnected
        | Error::InsufficientScope { .. } => EffectExecution::Failed {
            failure: failure(
                FailureClass::AuthRequired,
                "github_authorization_required",
                "The selected GitHub connection no longer authorizes this operation.",
                "Reconnect or select an eligible GitHub connection, then resume this Launch.",
                claim,
                None,
            ),
        },
        Error::ForkingDisabled => EffectExecution::Failed {
            failure: failure(
                FailureClass::PolicyUserAction,
                "upstream_not_forkable",
                "The upstream repository currently does not permit this fork workflow.",
                "Use a public repository whose current policy permits forking.",
                claim,
                None,
            ),
        },
        Error::ForkNetworkOccupied => EffectExecution::Failed {
            failure: failure(
                FailureClass::PolicyUserAction,
                "fork_network_occupied",
                "Your GitHub account already uses this fork network for an incompatible fork.",
                "Review the existing fork-network relationship; CMA will not delete or replace it.",
                claim,
                None,
            ),
        },
        Error::BranchHeadConflict => EffectExecution::Failed {
            failure: failure(
                FailureClass::ConflictStaleHead,
                "conflict_stale_head",
                "The launch branch already points at a different commit.",
                "Start a New Launch; CMA will not reset or force-push the existing branch.",
                claim,
                None,
            ),
        },
        Error::UpstreamBaseConflict => EffectExecution::Failed {
            failure: failure(
                FailureClass::ConflictStaleHead,
                "upstream_base_changed",
                "The upstream contribution base changed after this Launch captured it.",
                "Review a fresh diff against the current upstream base before publishing again.",
                claim,
                None,
            ),
        },
        Error::PullRequestChanged => EffectExecution::Failed {
            failure: failure(
                FailureClass::ConflictStaleHead,
                "pull_request_changed",
                "The linked pull request or contributor branch changed outside this reviewed intent.",
                "Review the current GitHub PR and request a fresh CMA diff before deciding how to continue.",
                claim,
                None,
            ),
        },
        Error::UnsupportedWorkspace(reason) => {
            let (code, detail) = match reason {
                WorkspaceUnsupported::TreeIncomplete => (
                    "workspace_tree_incomplete",
                    "GitHub could not provide a complete recursive repository tree.",
                ),
                WorkspaceUnsupported::FileCount => (
                    "repository_file_limit_exceeded",
                    "The repository contains more files than this Trigger flow supports.",
                ),
                WorkspaceUnsupported::FileSize => (
                    "repository_file_size_limit_exceeded",
                    "The repository contains a file larger than this Trigger flow supports.",
                ),
                WorkspaceUnsupported::RepositorySize => (
                    "repository_size_limit_exceeded",
                    "The repository is larger than this Trigger flow supports.",
                ),
                WorkspaceUnsupported::UnsafePath => (
                    "unsafe_workspace_entry",
                    "The repository contains a path or symlink that cannot be imported safely.",
                ),
                WorkspaceUnsupported::UnsupportedMode => (
                    "unsupported_git_entry",
                    "The repository contains a Git entry mode this Trigger flow cannot preserve.",
                ),
                WorkspaceUnsupported::Submodule => (
                    "submodule_metadata_unsupported",
                    "A Git submodule is missing its regular .gitmodules metadata or has an invalid gitlink representation.",
                ),
                WorkspaceUnsupported::LargeFileStorage => (
                    "git_lfs_unsupported",
                    "Git LFS content is not supported by the public Trigger flow.",
                ),
            };
            EffectExecution::Failed {
                failure: failure(
                    FailureClass::PolicyUserAction,
                    code,
                    detail,
                    "Use a repository whose complete byte tree fits the documented public limits.",
                    claim,
                    None,
                ),
            }
        }
        Error::Rejected {
            status,
            kind: super::github::GithubRejectionKind::WorkflowScope,
            ..
        } => EffectExecution::Failed {
            failure: failure(
                FailureClass::PolicyUserAction,
                "github_workflow_scope_refused",
                format!(
                    "GitHub refused the workflow-file update at the guarded ref boundary (HTTP {status})."
                ),
                "Reconnect GitHub with workflow authority or remove the workflow content change, then review again.",
                claim,
                None,
            ),
        },
        Error::Rejected {
            status,
            kind: super::github::GithubRejectionKind::BranchProtection,
            ..
        } => EffectExecution::Failed {
            failure: failure(
                FailureClass::PolicyUserAction,
                "github_branch_protection_refused",
                format!(
                    "GitHub branch protection or a repository ruleset refused this operation (HTTP {status})."
                ),
                "Review the repository protection or ruleset requirements, or ask a maintainer; CMA will not bypass protection.",
                claim,
                None,
            ),
        },
        Error::Rejected {
            kind: super::github::GithubRejectionKind::NonFastForward,
            ..
        } => broker_failure(Error::BranchHeadConflict, claim, reconcile_only),
        Error::ConnectionNotPersonal | Error::Forbidden | Error::Rejected { .. } => {
            EffectExecution::Failed {
                failure: failure(
                    FailureClass::PolicyUserAction,
                    "github_policy_refused",
                    "GitHub policy does not permit this operation for the selected account.",
                    "Review the selected account and repository policy before retrying.",
                    claim,
                    None,
                ),
            }
        }
        Error::NotFound => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalValidation,
                "github_resource_not_found",
                "The exact GitHub resource captured by this Trigger no longer exists.",
                "Ask the publisher to publish a new Trigger revision for an available resource.",
                claim,
                None,
            ),
        },
        Error::AuthorityDrift => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "github_authority_drift",
                "GitHub identity or authority changed after the retained preflight.",
                "Reconnect and establish a fresh preflight before any further write.",
                claim,
                None,
            ),
        },
        Error::Malformed(_) => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "github_evidence_malformed",
                "GitHub returned evidence that CMA could not validate safely.",
                "Contact support with the Launch correlation ID; no write will be repeated.",
                claim,
                None,
            ),
        },
    }
}

fn service_failure(error: TriggerServiceError, claim: &EffectClaim) -> EffectExecution {
    match error {
        TriggerServiceError::Authority(authority) => match authority {
            TriggerAuthorityError::OwnerCredentialUnavailable => {
                broker_failure(GithubBrokerError::OwnerCredentialUnavailable, claim, false)
            }
            TriggerAuthorityError::ConnectionNotFound => {
                broker_failure(GithubBrokerError::ConnectionNotFound, claim, false)
            }
            TriggerAuthorityError::ConnectionNotPersonal => {
                broker_failure(GithubBrokerError::ConnectionNotPersonal, claim, false)
            }
            TriggerAuthorityError::ConnectionInactive => {
                broker_failure(GithubBrokerError::ConnectionInactive, claim, false)
            }
            TriggerAuthorityError::ReauthorizationRequired => {
                broker_failure(GithubBrokerError::ConnectionDisconnected, claim, false)
            }
            TriggerAuthorityError::InsufficientScope { required } => broker_failure(
                GithubBrokerError::InsufficientScope {
                    required: if required == "public_repo" {
                        "public_repo"
                    } else {
                        "required_scope"
                    },
                },
                claim,
                false,
            ),
            TriggerAuthorityError::UnapprovedScope { scope } => broker_failure(
                GithubBrokerError::UnapprovedScope { scope },
                claim,
                false,
            ),
            TriggerAuthorityError::AuthorityDrift => {
                broker_failure(GithubBrokerError::AuthorityDrift, claim, false)
            }
            TriggerAuthorityError::Forbidden => {
                broker_failure(GithubBrokerError::Forbidden, claim, false)
            }
            TriggerAuthorityError::ResourceNotFound => {
                broker_failure(GithubBrokerError::NotFound, claim, false)
            }
            TriggerAuthorityError::RateLimited { retry_after } => {
                broker_failure(GithubBrokerError::RateLimited { retry_after }, claim, false)
            }
            TriggerAuthorityError::Transient => {
                broker_failure(GithubBrokerError::Transient { status: None }, claim, false)
            }
            TriggerAuthorityError::ProviderRejected => broker_failure(
                GithubBrokerError::Rejected {
                    status: 0,
                    kind: super::github::GithubRejectionKind::Unknown,
                    provider_code: None,
                },
                claim,
                false,
            ),
            TriggerAuthorityError::UpstreamNotForkable => {
                broker_failure(GithubBrokerError::ForkingDisabled, claim, false)
            }
            TriggerAuthorityError::ForkNetworkOccupied => {
                broker_failure(GithubBrokerError::ForkNetworkOccupied, claim, false)
            }
            TriggerAuthorityError::MalformedEvidence => broker_failure(
                GithubBrokerError::Malformed("provider evidence".into()),
                claim,
                false,
            ),
            TriggerAuthorityError::ProviderAuthenticationUnavailable => EffectExecution::Failed {
                failure: failure(
                    FailureClass::AuthRequired,
                    "github_provider_authentication_unavailable",
                    "This CMA deployment cannot broker GitHub authorization.",
                    "Use a CMA deployment with provider-backed sign-in enabled.",
                    claim,
                    None,
                ),
            },
        },
        TriggerServiceError::Admission(admission) => match admission {
            layer2_core::TriggerAdmissionError::Unavailable(_)
            | layer2_core::TriggerAdmissionError::PricingUnavailable(_) => transient_failure(
                claim,
                false,
                "launch_admission_temporarily_unavailable",
                "CMA could not re-establish safe Launch admission.",
                None,
            ),
            layer2_core::TriggerAdmissionError::QuotaExceeded(_) => EffectExecution::Failed {
                failure: failure(
                    FailureClass::ResourceQuota,
                    "launch_quota_exhausted",
                    "Session quota or deployment capacity is exhausted.",
                    "Release capacity or wait for capacity to become available before retrying.",
                    claim,
                    None,
                ),
            },
            layer2_core::TriggerAdmissionError::PriceChanged
            | layer2_core::TriggerAdmissionError::QuoteExpired => EffectExecution::Failed {
                failure: failure(
                    FailureClass::PolicyUserAction,
                    "launch_pricing_review_required",
                    "The retained Launch no longer has a startable pricing review.",
                    "Open a New Launch and explicitly accept current pricing before Start.",
                    claim,
                    None,
                ),
            },
            layer2_core::TriggerAdmissionError::SponsoredTermsNotAccepted => {
                EffectExecution::Failed {
                    failure: failure(
                        FailureClass::PolicyUserAction,
                        "launch_sponsored_terms_review_required",
                        "The retained Launch no longer has a current sponsored terms acceptance.",
                        "Open a New Launch and explicitly accept the current sponsored terms before Start.",
                        claim,
                        None,
                    ),
                }
            }
            layer2_core::TriggerAdmissionError::CapabilityUnsupported { .. }
            | layer2_core::TriggerAdmissionError::RuntimeChanged => EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalValidation,
                    "launch_runtime_admission_changed",
                    "The deployed runtime no longer satisfies the pinned Launch capability contract.",
                    "Open a New Launch after a compatible runtime is deployed.",
                    claim,
                    None,
                ),
            },
        },
        TriggerServiceError::Validation(_)
        | TriggerServiceError::CursorBeyondEnd { .. }
        | TriggerServiceError::RuntimeRouteUnavailable { .. } => {
            EffectExecution::Failed {
                failure: failure(
                    FailureClass::TerminalValidation,
                    "launch_preflight_invalid",
                    "The retained Launch no longer satisfies its Trigger contract.",
                    "Review the Launch and publish a corrected Trigger revision if needed.",
                    claim,
                    None,
                ),
            }
        }
        TriggerServiceError::NotFound => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalValidation,
                "launch_preflight_resource_missing",
                "A resource required by this Launch no longer exists.",
                "Review the Launch and its published Trigger revision.",
                claim,
                None,
            ),
        },
        TriggerServiceError::Forbidden => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalSecurity,
                "launch_preflight_forbidden",
                "The retained subject does not own this Launch authority.",
                "Sign in as the Launch owner; CMA will not transfer this Launch.",
                claim,
                None,
            ),
        },
        TriggerServiceError::Rollout(denial) => EffectExecution::Failed {
            failure: failure(
                FailureClass::FeaturePaused,
                format!("trigger_{}", denial.code()),
                "GitHub Trigger orchestration paused at a durable boundary.",
                "Wait for the deployment rollout policy to permit this Launch; the session remains available.",
                claim,
                None,
            ),
        },
        TriggerServiceError::Store(_) => transient_failure(
            claim,
            false,
            "launch_preflight_temporarily_unavailable",
            "Preflight evidence is temporarily unavailable.",
            None,
        ),
    }
}

fn transient_failure(
    claim: &EffectClaim,
    reconcile_only: bool,
    code: &'static str,
    detail: &'static str,
    retry_after: Option<&str>,
) -> EffectExecution {
    let retry_at = rfc3339_at(SystemTime::now() + retry_delay(claim, retry_after));
    let failure = failure(
        FailureClass::RetryableTransient,
        code,
        detail,
        "CMA will retry after the persisted provider backoff.",
        claim,
        Some(retry_at.clone()),
    );
    if reconcile_only {
        EffectExecution::ReconcileLater {
            failure,
            reconcile_at: retry_at,
            external_resource_ids: BTreeMap::new(),
        }
    } else {
        EffectExecution::RetryLater {
            failure,
            retry_at,
            external_resource_ids: BTreeMap::new(),
        }
    }
}

fn reconcile_failure(
    claim: &EffectClaim,
    code: &'static str,
    detail: &'static str,
    external_resource_ids: BTreeMap<String, String>,
) -> EffectExecution {
    let reconcile_at = rfc3339_at(SystemTime::now() + Duration::from_secs(2));
    EffectExecution::ReconcileLater {
        failure: failure(
            FailureClass::AmbiguousExternal,
            code,
            detail,
            "CMA will query GitHub before authorizing any replacement write.",
            claim,
            Some(reconcile_at.clone()),
        ),
        reconcile_at,
        external_resource_ids,
    }
}

fn unsupported_effect(claim: &EffectClaim) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(
            FailureClass::TerminalSecurity,
            "effect_adapter_unavailable",
            "The configured worker has no adapter for this Launch step.",
            "Contact support; CMA did not skip or simulate the missing step.",
            claim,
            None,
        ),
    }
}

fn admitted_effect_authority(
    launch: &Launch,
    claim: &EffectClaim,
) -> Result<CapabilityAuthorityEvidence, Box<EffectExecution>> {
    let admission = launch.effect_authority_admission(claim).map_err(|_| {
        Box::new(authority_admission_failure(
            claim,
            "trigger_authority_admission_missing",
            "The exact effect fence has no committed authority admission.",
        ))
    })?;
    authority_from_snapshot(admission, claim)
}

fn require_live_effect_authority_admission(
    launch: &Launch,
    claim: &EffectClaim,
) -> Result<(), Box<EffectExecution>> {
    let now = rfc3339_at(SystemTime::now());
    launch
        .effect_authority_admission_at(claim, &now)
        .map(|_| ())
        .map_err(|_| {
            Box::new(authority_admission_failure(
                claim,
                "trigger_authority_lease_not_live",
                "The admitted effect fence no longer owns a live provider-write lease.",
            ))
        })
}

fn require_live_policy_install_authority(
    launch: &Launch,
    claim: &EffectClaim,
    capability_renewal: bool,
) -> Result<(), Box<EffectExecution>> {
    if !capability_renewal {
        return require_live_effect_authority_admission(launch, claim);
    }
    let lease = launch.lease.as_ref().ok_or_else(|| {
        Box::new(authority_admission_failure(
            claim,
            "trigger_capability_authority_lease_not_live",
            "The admitted capability fence has no live policy-install lease.",
        ))
    })?;
    if lease.fencing_token != claim.fence {
        return Err(Box::new(authority_admission_failure(
            claim,
            "trigger_capability_authority_lease_not_live",
            "The admitted capability fence has no live policy-install lease.",
        )));
    }
    let now = rfc3339_at(SystemTime::now());
    launch
        .capability_authority_admission_for_at(lease, &now)
        .map(|_| ())
        .map_err(|_| {
            Box::new(authority_admission_failure(
                claim,
                "trigger_capability_authority_lease_not_live",
                "The admitted capability fence has no live policy-install lease.",
            ))
        })
}

fn authority_from_snapshot(
    admission: &AuthorityAdmissionSnapshot,
    claim: &EffectClaim,
) -> Result<CapabilityAuthorityEvidence, Box<EffectExecution>> {
    Ok(CapabilityAuthorityEvidence {
        connection: admission.connection.clone(),
        authority: admission.authority.clone(),
        upstream: admission.upstream.clone(),
        fork: admission.fork.clone().ok_or_else(|| {
            Box::new(authority_admission_failure(
                claim,
                "trigger_authority_admission_fork_missing",
                "The committed authority admission has no numeric fork binding.",
            ))
        })?,
        branch: admission.branch.clone().ok_or_else(|| {
            Box::new(authority_admission_failure(
                claim,
                "trigger_authority_admission_branch_missing",
                "The committed authority admission has no guarded branch binding.",
            ))
        })?,
    })
}

fn authority_admission_failure(
    claim: &EffectClaim,
    code: &'static str,
    detail: &'static str,
) -> EffectExecution {
    EffectExecution::Failed {
        failure: failure(
            FailureClass::TerminalSecurity,
            code,
            detail,
            "Contact support; CMA did not admit another external write.",
            claim,
            None,
        ),
    }
}

fn failure(
    class: FailureClass,
    code: impl Into<String>,
    detail: impl Into<String>,
    remediation: impl Into<String>,
    claim: &EffectClaim,
    retry_at: Option<String>,
) -> LaunchFailure {
    LaunchFailure {
        class,
        code: code.into(),
        detail: detail.into(),
        remediation: remediation.into(),
        correlation_id: claim.intent_key.clone(),
        retry_at,
        observed_at: rfc3339_at(SystemTime::now()),
    }
}

#[cfg(test)]
pub(in crate::trigger) mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use base64::Engine as _;
    use layer2_core::{
        BranchRecord, CheckpointId, Confirmation, ConfirmationAction, DiffArtifactId,
        EffectClaimMode, ForkRecord, LaunchState, PublicationState, PullRequestOperation,
        PullRequestState, RepositoryIdentity, RepositoryVisibility, SubjectId,
    };
    use ring::rand::SystemRandom;
    use ring::signature::Ed25519KeyPair;

    use super::*;

    enum ForkAnswer {
        Ready,
        Absent,
        Ambiguous,
        Occupied,
        Transient,
    }

    enum SessionAnswer {
        Evidence {
            status: SessionStatus,
            credential_ready: bool,
            rotations: u32,
        },
        EvidenceWithRecovery {
            status: SessionStatus,
            recovery_state: RecoveryState,
            credential_ready: bool,
            rotations: u32,
        },
        AuthorityMismatch,
    }

    fn test_credential_authority(session_id: &SessionId) -> SessionCredentialAuthority {
        SessionCredentialAuthority {
            session_id: session_id.clone(),
            custody_generation: 1,
            provider_key_id: "provider-session-key-1".into(),
            provider_state_version: 1,
            provider_expires_at_epoch_milliseconds: 4_000_000_000_000,
            grant_fingerprint: "a".repeat(64),
        }
    }

    fn sponsored_capability(
        launch: &Launch,
        session_id: SessionId,
        expires_at: impl Into<String>,
    ) -> InertGrantCapability {
        InertGrantCapability {
            grant_id: layer2_core::SponsoredGrantId::parse("22222222-2222-4222-8222-222222222222")
                .unwrap(),
            grant_generation: 1,
            policy_generation: 1,
            launch_id: launch.id.clone(),
            session_id,
            expires_at: expires_at.into(),
            fingerprint: "f".repeat(64),
        }
    }

    fn sponsored_authority_receipt(
        capability: &InertGrantCapability,
    ) -> layer3_adapter::SponsoredModelAuthorityReceipt {
        layer3_adapter::SponsoredModelAuthorityReceipt {
            runtime_audience: "sandbox-runtime-1".into(),
            session_id: capability.session_id.0.clone(),
            session_credential_generation: 1,
            launch_id: capability.launch_id.0.clone(),
            grant_id: capability.grant_id.0.clone(),
            grant_generation: capability.grant_generation,
            policy_generation: capability.policy_generation,
            maximum_output_tokens_per_request: 500,
            capability_fingerprint: capability.fingerprint.clone(),
        }
    }

    struct FakeSponsorships {
        answers: Mutex<VecDeque<Result<ActivatedSponsoredModelGrant, SponsorshipError>>>,
        contributors: Mutex<Vec<SubjectId>>,
        commands: Mutex<Vec<ActivateSponsoredGrant>>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl FakeSponsorships {
        fn new(
            answers: impl IntoIterator<Item = Result<InertGrantCapability, SponsorshipError>>,
            events: Arc<Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self {
                answers: Mutex::new(
                    answers
                        .into_iter()
                        .map(|answer| {
                            answer.map(|capability| ActivatedSponsoredModelGrant {
                                capability,
                                maximum_output_tokens_per_request: 500,
                            })
                        })
                        .collect(),
                ),
                contributors: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
                events,
            }
        }
    }

    #[async_trait]
    impl TriggerSponsorshipEffectPort for FakeSponsorships {
        async fn activate_grant(
            &self,
            contributor: &SubjectId,
            command: ActivateSponsoredGrant,
        ) -> Result<ActivatedSponsoredModelGrant, SponsorshipError> {
            self.contributors.lock().unwrap().push(contributor.clone());
            self.commands.lock().unwrap().push(command);
            self.events.lock().unwrap().push("grant_activation");
            self.answers.lock().unwrap().pop_front().unwrap()
        }
    }

    struct FakeSessions {
        answers: Mutex<VecDeque<SessionAnswer>>,
        prompt_answers: Mutex<VecDeque<Result<layer2_core::PromptDelivery, CoreError>>>,
        checkpoint_answers:
            Mutex<VecDeque<Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError>>>,
        checkpoint_requests: Mutex<Vec<layer3_adapter::WorkspaceCheckpointRequest>>,
        policy_receipt: Mutex<Option<layer3_adapter::RuntimePolicyReceipt>>,
        reachability_error: Mutex<Option<layer2_core::RuntimeGatewayReachabilityError>>,
        gateway_probe_requests: Mutex<Vec<layer3_adapter::RuntimeGatewayProbeRequest>>,
        installed_policies: Mutex<Vec<LaunchCapability>>,
        reconciliation_receipt: Mutex<Option<layer2_core::RuntimeGatewayReconciliationReceipt>>,
        reconciliation_error: Mutex<Option<layer2_core::RuntimeGatewayReachabilityError>>,
        reconciliation_probe_requests: Mutex<Vec<layer3_adapter::RuntimeGatewayProbeRequest>>,
        model_authority_answers: Mutex<
            VecDeque<
                Result<
                    layer3_adapter::SponsoredModelAuthorityReceipt,
                    SponsoredModelAuthorityError,
                >,
            >,
        >,
        credential_authority: Mutex<Option<SessionCredentialAuthority>>,
        installed_model_credential_authorities: Mutex<Vec<SessionCredentialAuthority>>,
        installed_model_capabilities: Mutex<Vec<InertGrantCapability>>,
        installed_model_gateway_origins: Mutex<Vec<layer3_adapter::SponsoredModelGatewayOrigin>>,
        events: Arc<Mutex<Vec<&'static str>>>,
        calls: AtomicUsize,
        prompt_calls: AtomicUsize,
        checkpoint_calls: AtomicUsize,
        gateway_probe_calls: AtomicUsize,
        reconciliation_calls: AtomicUsize,
        model_authority_calls: AtomicUsize,
        retirement_error: Mutex<Option<CoreError>>,
        retirement_settled: AtomicBool,
        retirement_calls: AtomicUsize,
        retired_session_ids: Mutex<Vec<SessionId>>,
    }

    impl FakeSessions {
        fn new(answers: impl IntoIterator<Item = SessionAnswer>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                prompt_answers: Mutex::new(VecDeque::new()),
                checkpoint_answers: Mutex::new(VecDeque::new()),
                checkpoint_requests: Mutex::new(Vec::new()),
                policy_receipt: Mutex::new(None),
                reachability_error: Mutex::new(None),
                gateway_probe_requests: Mutex::new(Vec::new()),
                installed_policies: Mutex::new(Vec::new()),
                reconciliation_receipt: Mutex::new(None),
                reconciliation_error: Mutex::new(None),
                reconciliation_probe_requests: Mutex::new(Vec::new()),
                model_authority_answers: Mutex::new(VecDeque::new()),
                credential_authority: Mutex::new(None),
                installed_model_credential_authorities: Mutex::new(Vec::new()),
                installed_model_capabilities: Mutex::new(Vec::new()),
                installed_model_gateway_origins: Mutex::new(Vec::new()),
                events: Arc::new(Mutex::new(Vec::new())),
                calls: AtomicUsize::new(0),
                prompt_calls: AtomicUsize::new(0),
                checkpoint_calls: AtomicUsize::new(0),
                gateway_probe_calls: AtomicUsize::new(0),
                reconciliation_calls: AtomicUsize::new(0),
                model_authority_calls: AtomicUsize::new(0),
                retirement_error: Mutex::new(None),
                retirement_settled: AtomicBool::new(true),
                retirement_calls: AtomicUsize::new(0),
                retired_session_ids: Mutex::new(Vec::new()),
            }
        }

        fn with_retirement_response_loss() -> Self {
            let sessions = Self::new([]);
            *sessions.retirement_error.lock().unwrap() = Some(CoreError::RuntimeUnavailable(
                "selected retirement response was lost".into(),
            ));
            sessions
        }

        fn with_prompt_answers(
            answers: impl IntoIterator<Item = Result<layer2_core::PromptDelivery, CoreError>>,
        ) -> Self {
            let sessions = Self::new([SessionAnswer::Evidence {
                status: SessionStatus::Ready,
                credential_ready: true,
                rotations: 0,
            }]);
            *sessions.prompt_answers.lock().unwrap() = answers.into_iter().collect();
            sessions
        }

        fn with_policy_receipt(receipt: layer3_adapter::RuntimePolicyReceipt) -> Self {
            let sessions = Self::new([]);
            *sessions.policy_receipt.lock().unwrap() = Some(receipt);
            sessions
        }

        fn with_reachability_error(error: layer2_core::RuntimeGatewayReachabilityError) -> Self {
            let sessions = Self::new([]);
            *sessions.reachability_error.lock().unwrap() = Some(error);
            sessions
        }

        fn with_checkpoint_answers(
            answers: impl IntoIterator<
                Item = Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError>,
            >,
        ) -> Self {
            let sessions = Self::new([]);
            *sessions.checkpoint_answers.lock().unwrap() = answers.into_iter().collect();
            sessions
        }

        fn with_events(mut self, events: Arc<Mutex<Vec<&'static str>>>) -> Self {
            self.events = events;
            self
        }
    }

    #[async_trait]
    impl TriggerSessionEffectPort for FakeSessions {
        async fn ensure_session(
            &self,
            launch: &Launch,
        ) -> Result<TriggerSessionEvidence, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.answers.lock().unwrap().pop_front().unwrap() {
                SessionAnswer::Evidence {
                    status,
                    credential_ready,
                    rotations,
                } => {
                    let session = trigger_session(launch, status, rotations);
                    let credential_authority =
                        credential_ready.then(|| test_credential_authority(&session.id));
                    Ok(TriggerSessionEvidence {
                        runtime_generation: Some(u64::from(rotations).saturating_add(1)),
                        credential_authority,
                        session,
                    })
                }
                SessionAnswer::EvidenceWithRecovery {
                    status,
                    recovery_state,
                    credential_ready,
                    rotations,
                } => {
                    let mut session = trigger_session(launch, status, rotations);
                    session.recovery_state = recovery_state;
                    let credential_authority =
                        credential_ready.then(|| test_credential_authority(&session.id));
                    Ok(TriggerSessionEvidence {
                        runtime_generation: Some(u64::from(rotations).saturating_add(1)),
                        credential_authority,
                        session,
                    })
                }
                SessionAnswer::AuthorityMismatch => Err(CoreError::InvalidRequest(
                    "the deterministic session is occupied by different authority".into(),
                )),
            }
        }

        async fn credential_authority(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<SessionCredentialAuthority>, CoreError> {
            Ok(Some(
                self.credential_authority
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| test_credential_authority(session_id)),
            ))
        }

        async fn install_runtime_policy_and_probe(
            &self,
            _session_id: &SessionId,
            _expected_credential_authority: &SessionCredentialAuthority,
            mut capability: LaunchCapability,
            signer: Arc<CapabilitySigner>,
            probe: layer3_adapter::RuntimeGatewayProbeRequest,
        ) -> Result<
            layer2_core::RuntimeGatewayReachabilityReceipt,
            layer2_core::RuntimeGatewayReachabilityError,
        > {
            self.gateway_probe_calls.fetch_add(1, Ordering::SeqCst);
            self.gateway_probe_requests
                .lock()
                .unwrap()
                .push(probe.clone());
            if let Some(error) = self.reachability_error.lock().unwrap().clone() {
                return Err(error);
            }
            let Some(receipt) = self.policy_receipt.lock().unwrap().clone() else {
                return Err(layer2_core::RuntimeGatewayReachabilityError::ProtocolUnsupported);
            };
            capability.runtime_audience = "sandbox-runtime-1".into();
            signer
                .sign_policy(&capability)
                .map_err(CoreError::InvalidRequest)
                .map_err(layer2_core::RuntimeGatewayReachabilityError::Core)?;
            self.installed_policies.lock().unwrap().push(capability);
            self.events.lock().unwrap().push("github_probe_allowed");
            Ok(layer2_core::RuntimeGatewayReachabilityReceipt {
                runtime_audience: "sandbox-runtime-1".into(),
                policy: receipt,
                probe_version: probe.version,
            })
        }

        async fn reconcile_runtime_policy_and_probe(
            &self,
            _session_id: &SessionId,
            _expected_credential_authority: &SessionCredentialAuthority,
            probe: layer3_adapter::RuntimeGatewayProbeRequest,
            _expected_launch_id: &str,
            _expected_generation: u64,
            _expected_fence: u64,
        ) -> Result<
            layer2_core::RuntimeGatewayReconciliationReceipt,
            layer2_core::RuntimeGatewayReachabilityError,
        > {
            self.reconciliation_calls.fetch_add(1, Ordering::SeqCst);
            self.reconciliation_probe_requests
                .lock()
                .unwrap()
                .push(probe);
            if let Some(error) = self.reconciliation_error.lock().unwrap().clone() {
                return Err(error);
            }
            let receipt = self
                .reconciliation_receipt
                .lock()
                .unwrap()
                .clone()
                .ok_or(layer2_core::RuntimeGatewayReachabilityError::ProtocolUnsupported)?;
            self.events.lock().unwrap().push("github_probe_reconciled");
            Ok(receipt)
        }

        async fn install_sponsored_model_authority(
            &self,
            _session_id: &SessionId,
            expected_credential_authority: &SessionCredentialAuthority,
            capability: &InertGrantCapability,
            _maximum_output_tokens_per_request: u64,
            gateway_origin: &layer3_adapter::SponsoredModelGatewayOrigin,
        ) -> Result<layer3_adapter::SponsoredModelAuthorityReceipt, SponsoredModelAuthorityError>
        {
            self.model_authority_calls.fetch_add(1, Ordering::SeqCst);
            self.installed_model_credential_authorities
                .lock()
                .unwrap()
                .push(expected_credential_authority.clone());
            self.installed_model_capabilities
                .lock()
                .unwrap()
                .push(capability.clone());
            self.installed_model_gateway_origins
                .lock()
                .unwrap()
                .push(gateway_origin.clone());
            self.events.lock().unwrap().push("model_authority_install");
            self.model_authority_answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(SponsoredModelAuthorityError::ProtocolUnsupported))
        }

        async fn runtime_policy_audience(
            &self,
            _session_id: &SessionId,
        ) -> Result<String, CoreError> {
            Ok("sandbox-runtime-1".into())
        }

        async fn runtime_policy_observation(
            &self,
            _session_id: &SessionId,
        ) -> Result<layer2_core::RuntimePolicyObservation, CoreError> {
            Err(CoreError::CapabilityUnsupported(
                "authenticated_runtime_policy_observation".into(),
            ))
        }

        async fn install_runtime_tombstone(
            &self,
            _session_id: &SessionId,
            mut tombstone: RuntimePolicyTombstone,
            signer: Arc<CapabilitySigner>,
        ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError> {
            let Some(receipt) = self.policy_receipt.lock().unwrap().clone() else {
                return Err(CoreError::CapabilityUnsupported(
                    "authenticated_runtime_policy".into(),
                ));
            };
            tombstone.runtime_audience = "sandbox-runtime-1".into();
            signer
                .sign_tombstone(tombstone)
                .map_err(CoreError::InvalidRequest)?;
            Ok(("sandbox-runtime-1".into(), receipt))
        }

        async fn retire_runtime_credential(&self, session_id: &SessionId) -> Result<(), CoreError> {
            self.retirement_calls.fetch_add(1, Ordering::SeqCst);
            self.retired_session_ids
                .lock()
                .unwrap()
                .push(session_id.clone());
            self.retirement_error
                .lock()
                .unwrap()
                .take()
                .map_or(Ok(()), Err)
        }

        async fn runtime_credential_retirement_settled(
            &self,
            _session_id: &SessionId,
        ) -> Result<bool, CoreError> {
            Ok(self.retirement_settled.load(Ordering::SeqCst))
        }

        async fn dispatch_initial_prompt(
            &self,
            _launch: &Launch,
            _claim: &EffectClaim,
        ) -> Result<layer2_core::PromptDelivery, CoreError> {
            self.prompt_calls.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("prompt_dispatch");
            self.prompt_answers.lock().unwrap().pop_front().unwrap()
        }

        async fn checkpoint_workspace(
            &self,
            _launch: &Launch,
            request: &layer3_adapter::WorkspaceCheckpointRequest,
        ) -> Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError> {
            self.checkpoint_calls.fetch_add(1, Ordering::SeqCst);
            self.checkpoint_requests
                .lock()
                .unwrap()
                .push(request.clone());
            self.checkpoint_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a checkpoint answer was configured")
        }
    }

    struct FakeCheckpointStore {
        answers: Mutex<VecDeque<Result<String, TriggerStoreError>>>,
        writes: Mutex<Vec<(SubjectId, WorkspaceCheckpoint, Vec<u8>)>>,
    }

    impl FakeCheckpointStore {
        fn new(answers: impl IntoIterator<Item = Result<String, TriggerStoreError>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                writes: Mutex::new(Vec::new()),
            }
        }
    }

    struct FakeReviewStore {
        baseline_reads: Mutex<VecDeque<Result<Option<Vec<u8>>, TriggerStoreError>>>,
        checkpoint_reads: Mutex<VecDeque<Result<Option<Vec<u8>>, TriggerStoreError>>>,
        diff_reads: Mutex<VecDeque<Result<Option<Vec<u8>>, TriggerStoreError>>>,
        diff_answers: Mutex<VecDeque<Result<String, TriggerStoreError>>>,
        diff_writes: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeReviewStore {
        fn new(
            baseline_reads: impl IntoIterator<Item = Result<Option<Vec<u8>>, TriggerStoreError>>,
            checkpoint_reads: impl IntoIterator<Item = Result<Option<Vec<u8>>, TriggerStoreError>>,
            diff_answers: impl IntoIterator<Item = Result<String, TriggerStoreError>>,
        ) -> Self {
            Self {
                baseline_reads: Mutex::new(baseline_reads.into_iter().collect()),
                checkpoint_reads: Mutex::new(checkpoint_reads.into_iter().collect()),
                diff_reads: Mutex::new(VecDeque::new()),
                diff_answers: Mutex::new(diff_answers.into_iter().collect()),
                diff_writes: Mutex::new(Vec::new()),
            }
        }

        fn with_publication_reads(
            checkpoint_reads: impl IntoIterator<Item = Result<Option<Vec<u8>>, TriggerStoreError>>,
            diff_reads: impl IntoIterator<Item = Result<Option<Vec<u8>>, TriggerStoreError>>,
        ) -> Self {
            Self {
                baseline_reads: Mutex::new(VecDeque::new()),
                checkpoint_reads: Mutex::new(checkpoint_reads.into_iter().collect()),
                diff_reads: Mutex::new(diff_reads.into_iter().collect()),
                diff_answers: Mutex::new(VecDeque::new()),
                diff_writes: Mutex::new(Vec::new()),
            }
        }
    }

    #[derive(Default)]
    struct FakeImportState {
        source_revision: Option<String>,
        tree_digest: Option<String>,
        blobs: Vec<BaselineImportBlob>,
        chunks: BTreeMap<(String, u32), Vec<u8>>,
        completed: Option<BaselineImportOutcome>,
        final_archive: Option<Vec<u8>>,
        retired: bool,
        deleted: bool,
    }

    struct FakeImportStore {
        state: Mutex<FakeImportState>,
        put_answers: Mutex<VecDeque<Result<(), TriggerStoreError>>>,
        finalize_answers: Mutex<VecDeque<Result<(), TriggerStoreError>>>,
        put_calls: Mutex<Vec<(String, u32)>>,
        reconcile_calls: AtomicUsize,
        finalize_calls: AtomicUsize,
        retire_calls: AtomicUsize,
    }

    impl FakeImportStore {
        fn new(
            put_answers: impl IntoIterator<Item = Result<(), TriggerStoreError>>,
            finalize_answers: impl IntoIterator<Item = Result<(), TriggerStoreError>>,
        ) -> Self {
            Self {
                state: Mutex::new(FakeImportState::default()),
                put_answers: Mutex::new(put_answers.into_iter().collect()),
                finalize_answers: Mutex::new(finalize_answers.into_iter().collect()),
                put_calls: Mutex::new(Vec::new()),
                reconcile_calls: AtomicUsize::new(0),
                finalize_calls: AtomicUsize::new(0),
                retire_calls: AtomicUsize::new(0),
            }
        }

        fn stage_inventory(&self, source: &WorkspaceSourcePlan) {
            let mut state = self.state.lock().unwrap();
            state.source_revision = Some(source.source_revision.clone());
            state.tree_digest = Some(source.tree_digest.clone());
            state.blobs = source.blobs.clone();
        }

        fn stage_chunk(&self, blob: &BaselineImportBlob, ordinal: u32, bytes: Vec<u8>) {
            self.state
                .lock()
                .unwrap()
                .chunks
                .insert((blob.provider_object_id.clone(), ordinal), bytes);
        }

        fn retain_conflicting_final_archive(&self, bytes: Vec<u8>) {
            self.state.lock().unwrap().final_archive = Some(bytes);
        }
    }

    #[async_trait]
    impl BaselineObjectStore for FakeImportStore {
        async fn reconcile_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            source_revision: &str,
        ) -> Result<Option<BaselineImportOutcome>, TriggerStoreError> {
            self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
            let state = self.state.lock().unwrap();
            let Some(completed) = state.completed.as_ref() else {
                return Ok(None);
            };
            if completed.manifest.source_revision != source_revision {
                return Err(TriggerStoreError::Malformed(
                    "completed fake import source changed".into(),
                ));
            }
            let archive = state.final_archive.as_ref().ok_or_else(|| {
                TriggerStoreError::Malformed("completed fake import has no archive".into())
            })?;
            if archive.len() as u64 != completed.archive_bytes
                || format!("{:x}", Sha256::digest(archive)) != completed.archive_sha256
            {
                return Err(TriggerStoreError::Malformed(
                    "completed fake import archive changed".into(),
                ));
            }
            Ok(Some(completed.clone()))
        }

        async fn begin_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            source_revision: &str,
            tree_digest: &str,
            blobs: &[BaselineImportBlob],
        ) -> Result<(), TriggerStoreError> {
            let mut state = self.state.lock().unwrap();
            match (&state.source_revision, &state.tree_digest) {
                (None, None) => {
                    state.source_revision = Some(source_revision.into());
                    state.tree_digest = Some(tree_digest.into());
                    state.blobs = blobs.to_vec();
                    Ok(())
                }
                (Some(retained_source), Some(retained_tree))
                    if retained_source == source_revision
                        && retained_tree == tree_digest
                        && state.blobs == blobs =>
                {
                    Ok(())
                }
                _ => Err(TriggerStoreError::Malformed(
                    "fake import inventory identity changed".into(),
                )),
            }
        }

        async fn get_baseline_import_chunk(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            source_revision: &str,
            blob: &BaselineImportBlob,
            ordinal: u32,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            let state = self.state.lock().unwrap();
            if state.source_revision.as_deref() != Some(source_revision)
                || !state.blobs.contains(blob)
                || ordinal >= blob.chunk_count
            {
                return Err(TriggerStoreError::Malformed(
                    "fake import chunk identity changed".into(),
                ));
            }
            Ok(state
                .chunks
                .get(&(blob.provider_object_id.clone(), ordinal))
                .cloned())
        }

        async fn put_baseline_import_chunk(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            source_revision: &str,
            blob: &BaselineImportBlob,
            chunk: &BaselineImportChunk,
            bytes: &[u8],
        ) -> Result<(), TriggerStoreError> {
            self.put_calls
                .lock()
                .unwrap()
                .push((chunk.provider_object_id.clone(), chunk.ordinal));
            if let Some(answer) = self.put_answers.lock().unwrap().pop_front() {
                answer?;
            }
            if chunk.provider_object_id != blob.provider_object_id
                || chunk.ordinal >= blob.chunk_count
                || chunk.bytes != bytes.len() as u64
                || format!("{:x}", Sha256::digest(bytes)) != chunk.sha256
            {
                return Err(TriggerStoreError::Malformed(
                    "fake import chunk metadata changed".into(),
                ));
            }
            let mut state = self.state.lock().unwrap();
            if state.source_revision.as_deref() != Some(source_revision)
                || !state.blobs.contains(blob)
            {
                return Err(TriggerStoreError::Malformed(
                    "fake import chunk is outside its inventory".into(),
                ));
            }
            let key = (blob.provider_object_id.clone(), chunk.ordinal);
            if let Some(retained) = state.chunks.get(&key) {
                if retained == bytes {
                    return Ok(());
                }
                return Err(TriggerStoreError::Malformed(
                    "fake import key already contains different bytes".into(),
                ));
            }
            state.chunks.insert(key, bytes.to_vec());
            Ok(())
        }

        async fn finalize_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            manifest: &WorkspaceManifest,
            plan: BaselineArchivePlan,
        ) -> Result<BaselineImportOutcome, TriggerStoreError> {
            self.finalize_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(answer) = self.finalize_answers.lock().unwrap().pop_front() {
                answer?;
            }
            let mut state = self.state.lock().unwrap();
            if state.source_revision.as_deref() != Some(plan.source_revision.as_str())
                || state.tree_digest.as_deref() != Some(plan.tree_digest.as_str())
            {
                return Err(TriggerStoreError::Malformed(
                    "fake finalization identity changed".into(),
                ));
            }
            let mut archive = plan.preamble;
            for chunk in plan.chunks {
                let bytes = state
                    .chunks
                    .get(&(chunk.provider_object_id.clone(), chunk.ordinal))
                    .ok_or_else(|| {
                        TriggerStoreError::Unavailable(
                            "fake finalization is missing a staged chunk".into(),
                        )
                    })?;
                if bytes.len() as u64 != chunk.bytes
                    || format!("{:x}", Sha256::digest(bytes)) != chunk.sha256
                {
                    return Err(TriggerStoreError::Malformed(
                        "fake finalization chunk changed".into(),
                    ));
                }
                archive.extend_from_slice(bytes);
            }
            if archive.len() as u64 != plan.archive_bytes {
                return Err(TriggerStoreError::Malformed(
                    "fake final archive has the wrong size".into(),
                ));
            }
            if let Some(retained) = state.final_archive.as_ref() {
                if retained != &archive {
                    return Err(TriggerStoreError::Malformed(
                        "fake final object key contains different bytes".into(),
                    ));
                }
            } else {
                state.final_archive = Some(archive.clone());
            }
            let outcome = BaselineImportOutcome {
                manifest: manifest.clone(),
                object_reference: "fake/import/baseline.pack".into(),
                archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                archive_bytes: archive.len() as u64,
            };
            state.completed = Some(outcome.clone());
            Ok(outcome)
        }

        async fn retire_baseline_import_chunks(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
        ) -> Result<(), TriggerStoreError> {
            self.retire_calls.fetch_add(1, Ordering::SeqCst);
            let mut state = self.state.lock().unwrap();
            if state.completed.is_none() {
                return Err(TriggerStoreError::Malformed(
                    "fake import cannot retire before completion".into(),
                ));
            }
            state.chunks.clear();
            state.retired = true;
            Ok(())
        }

        async fn delete_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
        ) -> Result<(), TriggerStoreError> {
            let mut state = self.state.lock().unwrap();
            state.chunks.clear();
            state.completed = None;
            state.deleted = true;
            Ok(())
        }

        async fn put_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _manifest: &WorkspaceManifest,
            _archive_sha256: &str,
            archive: &[u8],
        ) -> Result<String, TriggerStoreError> {
            self.state.lock().unwrap().final_archive = Some(archive.to_vec());
            Ok("fake/import/baseline.pack".into())
        }

        async fn get_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _reference: &str,
            _archive_sha256: &str,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Ok(self.state.lock().unwrap().final_archive.clone())
        }

        async fn delete_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _reference: &str,
        ) -> Result<(), TriggerStoreError> {
            self.state.lock().unwrap().final_archive = None;
            Ok(())
        }
    }

    struct FakeCleanupStore {
        baseline_answers: Mutex<VecDeque<Result<(), TriggerStoreError>>>,
        checkpoint_answers: Mutex<VecDeque<Result<(), TriggerStoreError>>>,
        diff_answers: Mutex<VecDeque<Result<(), TriggerStoreError>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeCleanupStore {
        fn new(
            baseline_answers: impl IntoIterator<Item = Result<(), TriggerStoreError>>,
            checkpoint_answers: impl IntoIterator<Item = Result<(), TriggerStoreError>>,
            diff_answers: impl IntoIterator<Item = Result<(), TriggerStoreError>>,
        ) -> Self {
            Self {
                baseline_answers: Mutex::new(baseline_answers.into_iter().collect()),
                checkpoint_answers: Mutex::new(checkpoint_answers.into_iter().collect()),
                diff_answers: Mutex::new(diff_answers.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn answer(
            answers: &Mutex<VecDeque<Result<(), TriggerStoreError>>>,
        ) -> Result<(), TriggerStoreError> {
            answers.lock().unwrap().pop_front().unwrap_or(Ok(()))
        }
    }

    #[async_trait]
    impl BaselineObjectStore for FakeCleanupStore {
        async fn reconcile_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
        ) -> Result<Option<BaselineImportOutcome>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never reconciles a baseline import".into(),
            ))
        }

        async fn begin_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
            _tree_digest: &str,
            _blobs: &[BaselineImportBlob],
        ) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never begins a baseline import".into(),
            ))
        }

        async fn get_baseline_import_chunk(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
            _blob: &BaselineImportBlob,
            _ordinal: u32,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never reads a baseline import chunk".into(),
            ))
        }

        async fn put_baseline_import_chunk(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
            _blob: &BaselineImportBlob,
            _chunk: &BaselineImportChunk,
            _bytes: &[u8],
        ) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never writes a baseline import chunk".into(),
            ))
        }

        async fn finalize_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _manifest: &WorkspaceManifest,
            _plan: BaselineArchivePlan,
        ) -> Result<BaselineImportOutcome, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never finalizes a baseline import".into(),
            ))
        }

        async fn retire_baseline_import_chunks(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
        ) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never retires baseline import chunks".into(),
            ))
        }

        async fn delete_baseline_import(
            &self,
            owner: &SubjectId,
            launch: &layer2_core::LaunchId,
        ) -> Result<(), TriggerStoreError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("baseline-import:{}:{}", owner.0, launch.0));
            Self::answer(&self.baseline_answers)
        }

        async fn put_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _manifest: &WorkspaceManifest,
            _archive_sha256: &str,
            _archive: &[u8],
        ) -> Result<String, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never writes a baseline".into(),
            ))
        }

        async fn get_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _reference: &str,
            _archive_sha256: &str,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never reads baseline bytes".into(),
            ))
        }

        async fn delete_baseline(
            &self,
            owner: &SubjectId,
            launch: &layer2_core::LaunchId,
            reference: &str,
        ) -> Result<(), TriggerStoreError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("baseline:{}:{}:{reference}", owner.0, launch.0));
            Self::answer(&self.baseline_answers)
        }
    }

    #[async_trait]
    impl CheckpointObjectStore for FakeCleanupStore {
        async fn put_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
            _archive: &[u8],
        ) -> Result<String, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never writes a checkpoint".into(),
            ))
        }

        async fn get_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never reads checkpoint bytes".into(),
            ))
        }

        async fn delete_checkpoint(
            &self,
            owner: &SubjectId,
            checkpoint: &WorkspaceCheckpoint,
        ) -> Result<(), TriggerStoreError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("checkpoint:{}:{}", owner.0, checkpoint.id.0));
            Self::answer(&self.checkpoint_answers)
        }

        async fn delete_checkpoint_intent(
            &self,
            owner: &SubjectId,
            launch: &layer2_core::LaunchId,
            intent: &layer2_core::WorkspaceCheckpointIntent,
        ) -> Result<(), TriggerStoreError> {
            self.calls.lock().unwrap().push(format!(
                "checkpoint-intent:{}:{}:{}",
                owner.0, launch.0, intent.id.0
            ));
            Self::answer(&self.checkpoint_answers)
        }
    }

    #[async_trait]
    impl DiffArtifactObjectStore for FakeCleanupStore {
        async fn put_diff_artifact(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _artifact: &layer2_core::DiffArtifactId,
            _sha256: &str,
            _bytes: &[u8],
        ) -> Result<String, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never writes a diff artifact".into(),
            ))
        }

        async fn get_diff_artifact(
            &self,
            _owner: &SubjectId,
            _artifact: &layer2_core::DiffArtifact,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "cleanup never reads diff bytes".into(),
            ))
        }

        async fn delete_diff_artifact(
            &self,
            owner: &SubjectId,
            artifact: &layer2_core::DiffArtifact,
        ) -> Result<(), TriggerStoreError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("diff:{}:{}", owner.0, artifact.id.0));
            Self::answer(&self.diff_answers)
        }
    }

    #[async_trait]
    impl BaselineObjectStore for FakeReviewStore {
        async fn reconcile_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
        ) -> Result<Option<BaselineImportOutcome>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot reconcile a baseline import".into(),
            ))
        }

        async fn begin_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
            _tree_digest: &str,
            _blobs: &[BaselineImportBlob],
        ) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot begin a baseline import".into(),
            ))
        }

        async fn get_baseline_import_chunk(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
            _blob: &BaselineImportBlob,
            _ordinal: u32,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot read a baseline import chunk".into(),
            ))
        }

        async fn put_baseline_import_chunk(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _source_revision: &str,
            _blob: &BaselineImportBlob,
            _chunk: &BaselineImportChunk,
            _bytes: &[u8],
        ) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot write a baseline import chunk".into(),
            ))
        }

        async fn finalize_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _manifest: &WorkspaceManifest,
            _plan: BaselineArchivePlan,
        ) -> Result<BaselineImportOutcome, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot finalize a baseline import".into(),
            ))
        }

        async fn retire_baseline_import_chunks(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
        ) -> Result<(), TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot retire baseline import chunks".into(),
            ))
        }

        async fn delete_baseline_import(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }

        async fn put_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _manifest: &WorkspaceManifest,
            _archive_sha256: &str,
            _archive: &[u8],
        ) -> Result<String, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot write a baseline".into(),
            ))
        }

        async fn get_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _reference: &str,
            _archive_sha256: &str,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            self.baseline_reads
                .lock()
                .unwrap()
                .pop_front()
                .expect("a baseline read answer was configured")
        }

        async fn delete_baseline(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _reference: &str,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl CheckpointObjectStore for FakeReviewStore {
        async fn put_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
            _archive: &[u8],
        ) -> Result<String, TriggerStoreError> {
            Err(TriggerStoreError::Malformed(
                "a review test cannot write a checkpoint".into(),
            ))
        }

        async fn get_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            self.checkpoint_reads
                .lock()
                .unwrap()
                .pop_front()
                .expect("a checkpoint read answer was configured")
        }

        async fn delete_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }

        async fn delete_checkpoint_intent(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _intent: &layer2_core::WorkspaceCheckpointIntent,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl DiffArtifactObjectStore for FakeReviewStore {
        async fn put_diff_artifact(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _artifact: &layer2_core::DiffArtifactId,
            _sha256: &str,
            bytes: &[u8],
        ) -> Result<String, TriggerStoreError> {
            self.diff_writes.lock().unwrap().push(bytes.to_vec());
            self.diff_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a diff storage answer was configured")
        }

        async fn get_diff_artifact(
            &self,
            _owner: &SubjectId,
            _artifact: &layer2_core::DiffArtifact,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            self.diff_reads
                .lock()
                .unwrap()
                .pop_front()
                .expect("a diff read answer was configured")
        }

        async fn delete_diff_artifact(
            &self,
            _owner: &SubjectId,
            _artifact: &layer2_core::DiffArtifact,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl CheckpointObjectStore for FakeCheckpointStore {
        async fn put_checkpoint(
            &self,
            owner: &SubjectId,
            checkpoint: &WorkspaceCheckpoint,
            archive: &[u8],
        ) -> Result<String, TriggerStoreError> {
            self.writes
                .lock()
                .unwrap()
                .push((owner.clone(), checkpoint.clone(), archive.to_vec()));
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a checkpoint storage answer was configured")
        }

        async fn get_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
        ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
            Ok(None)
        }

        async fn delete_checkpoint(
            &self,
            _owner: &SubjectId,
            _checkpoint: &WorkspaceCheckpoint,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }

        async fn delete_checkpoint_intent(
            &self,
            _owner: &SubjectId,
            _launch: &layer2_core::LaunchId,
            _intent: &layer2_core::WorkspaceCheckpointIntent,
        ) -> Result<(), TriggerStoreError> {
            Ok(())
        }
    }

    struct FakeGithub {
        answers: Mutex<VecDeque<ForkAnswer>>,
        capability_evidence: Mutex<Option<CapabilityAuthorityEvidence>>,
        workspace_source: Mutex<Option<WorkspaceSourcePlan>>,
        workspace_blobs: Mutex<BTreeMap<String, Vec<u8>>>,
        workspace_import_calls: AtomicUsize,
        workspace_blob_calls: Mutex<Vec<String>>,
        publication_answers:
            Mutex<VecDeque<Result<super::super::github::PublicationAdvance, GithubBrokerError>>>,
        pull_request_answers:
            Mutex<VecDeque<Result<super::super::github::PullRequestAdvance, GithubBrokerError>>>,
        create_calls: AtomicUsize,
        reconcile_calls: AtomicUsize,
        publication_calls: AtomicUsize,
        pull_request_write_calls: AtomicUsize,
        pull_request_reconcile_calls: AtomicUsize,
    }

    impl FakeGithub {
        fn new(answers: impl IntoIterator<Item = ForkAnswer>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                capability_evidence: Mutex::new(None),
                workspace_source: Mutex::new(None),
                workspace_blobs: Mutex::new(BTreeMap::new()),
                workspace_import_calls: AtomicUsize::new(0),
                workspace_blob_calls: Mutex::new(Vec::new()),
                publication_answers: Mutex::new(VecDeque::new()),
                pull_request_answers: Mutex::new(VecDeque::new()),
                create_calls: AtomicUsize::new(0),
                reconcile_calls: AtomicUsize::new(0),
                publication_calls: AtomicUsize::new(0),
                pull_request_write_calls: AtomicUsize::new(0),
                pull_request_reconcile_calls: AtomicUsize::new(0),
            }
        }

        fn with_publication_answers(
            answers: impl IntoIterator<
                Item = Result<super::super::github::PublicationAdvance, GithubBrokerError>,
            >,
        ) -> Self {
            Self {
                answers: Mutex::new(VecDeque::new()),
                capability_evidence: Mutex::new(None),
                workspace_source: Mutex::new(None),
                workspace_blobs: Mutex::new(BTreeMap::new()),
                workspace_import_calls: AtomicUsize::new(0),
                workspace_blob_calls: Mutex::new(Vec::new()),
                publication_answers: Mutex::new(answers.into_iter().collect()),
                pull_request_answers: Mutex::new(VecDeque::new()),
                create_calls: AtomicUsize::new(0),
                reconcile_calls: AtomicUsize::new(0),
                publication_calls: AtomicUsize::new(0),
                pull_request_write_calls: AtomicUsize::new(0),
                pull_request_reconcile_calls: AtomicUsize::new(0),
            }
        }

        fn with_pull_request_answers(
            answers: impl IntoIterator<
                Item = Result<super::super::github::PullRequestAdvance, GithubBrokerError>,
            >,
        ) -> Self {
            Self {
                answers: Mutex::new(VecDeque::new()),
                capability_evidence: Mutex::new(None),
                workspace_source: Mutex::new(None),
                workspace_blobs: Mutex::new(BTreeMap::new()),
                workspace_import_calls: AtomicUsize::new(0),
                workspace_blob_calls: Mutex::new(Vec::new()),
                publication_answers: Mutex::new(VecDeque::new()),
                pull_request_answers: Mutex::new(answers.into_iter().collect()),
                create_calls: AtomicUsize::new(0),
                reconcile_calls: AtomicUsize::new(0),
                publication_calls: AtomicUsize::new(0),
                pull_request_write_calls: AtomicUsize::new(0),
                pull_request_reconcile_calls: AtomicUsize::new(0),
            }
        }

        fn with_capability_evidence(evidence: CapabilityAuthorityEvidence) -> Self {
            let github = Self::new([]);
            *github.capability_evidence.lock().unwrap() = Some(evidence);
            github
        }

        fn with_workspace(
            source: WorkspaceSourcePlan,
            blobs: impl IntoIterator<Item = (String, Vec<u8>)>,
        ) -> Self {
            let github = Self::new([]);
            *github.workspace_source.lock().unwrap() = Some(source);
            *github.workspace_blobs.lock().unwrap() = blobs.into_iter().collect();
            github
        }

        fn answer(&self, context: &ForkContext) -> Result<ForkReconciliation, GithubBrokerError> {
            match self.answers.lock().unwrap().pop_front().unwrap() {
                ForkAnswer::Ready => Ok(ForkReconciliation::Ready(Box::new(fork(context)))),
                ForkAnswer::Absent => Ok(ForkReconciliation::Absent),
                ForkAnswer::Ambiguous => Err(GithubBrokerError::AmbiguousWrite),
                ForkAnswer::Occupied => Err(GithubBrokerError::ForkNetworkOccupied),
                ForkAnswer::Transient => Err(GithubBrokerError::Transient { status: Some(503) }),
            }
        }
    }

    #[async_trait]
    impl GithubEffectPort for FakeGithub {
        async fn preflight(&self, launch: &Launch) -> Result<LaunchAuthority, TriggerServiceError> {
            Ok(LaunchAuthority {
                connection: launch.connection.clone().unwrap(),
                provenance: launch.authority.clone().unwrap(),
                source_sha: launch.source_sha.clone(),
                workflow_paths: launch.workflow_paths.clone(),
            })
        }

        async fn create_fork(
            &self,
            context: &ForkContext,
        ) -> Result<ForkReconciliation, GithubBrokerError> {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            self.answer(context)
        }

        async fn reconcile_fork(
            &self,
            context: &ForkContext,
        ) -> Result<ForkReconciliation, GithubBrokerError> {
            self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
            self.answer(context)
        }

        async fn create_branch(
            &self,
            _context: &BranchContext,
        ) -> Result<BranchReconciliation, GithubBrokerError> {
            Err(GithubBrokerError::Malformed(
                "branch answer was not configured".into(),
            ))
        }

        async fn reconcile_branch(
            &self,
            _context: &BranchContext,
        ) -> Result<BranchReconciliation, GithubBrokerError> {
            Err(GithubBrokerError::Malformed(
                "branch answer was not configured".into(),
            ))
        }

        async fn import_workspace(
            &self,
            _context: &WorkspaceContext,
        ) -> Result<WorkspaceSourcePlan, GithubBrokerError> {
            self.workspace_import_calls.fetch_add(1, Ordering::SeqCst);
            self.workspace_source
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| {
                    GithubBrokerError::Malformed("workspace answer was not configured".into())
                })
        }

        async fn read_workspace_blob(
            &self,
            _context: &WorkspaceContext,
            blob: &BaselineImportBlob,
        ) -> Result<Vec<u8>, GithubBrokerError> {
            self.workspace_blob_calls
                .lock()
                .unwrap()
                .push(blob.provider_object_id.clone());
            self.workspace_blobs
                .lock()
                .unwrap()
                .get(&blob.provider_object_id)
                .cloned()
                .ok_or_else(|| {
                    GithubBrokerError::Malformed("workspace blob answer was not configured".into())
                })
        }

        async fn preflight_capability(
            &self,
            launch: &Launch,
        ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError> {
            if let Some(evidence) = self.capability_evidence.lock().unwrap().take() {
                return Ok(evidence);
            }
            Ok(CapabilityAuthorityEvidence {
                connection: launch.connection.clone().unwrap(),
                authority: launch.authority.clone().unwrap(),
                upstream: launch.revision.content.upstream.repository.clone(),
                fork: launch.fork.as_ref().unwrap().repository.clone(),
                branch: launch.branch.clone().unwrap(),
            })
        }

        async fn preflight_effect_authority(
            &self,
            launch: &Launch,
            kind: LaunchEffectKind,
        ) -> Result<EffectAuthorityEvidence, GithubBrokerError> {
            if matches!(
                kind,
                LaunchEffectKind::Reach
                    | LaunchEffectKind::BranchPublication
                    | LaunchEffectKind::PullRequest
            ) {
                let evidence = self.preflight_capability(launch).await?;
                return Ok(EffectAuthorityEvidence {
                    connection: evidence.connection,
                    authority: evidence.authority,
                    upstream: evidence.upstream,
                    fork: Some(evidence.fork),
                    branch: Some(evidence.branch),
                });
            }
            Ok(EffectAuthorityEvidence {
                connection: launch.connection.clone().ok_or_else(|| {
                    GithubBrokerError::Malformed("test Launch has no connection".into())
                })?,
                authority: launch.authority.clone().ok_or_else(|| {
                    GithubBrokerError::Malformed("test Launch has no authority".into())
                })?,
                upstream: launch.revision.content.upstream.repository.clone(),
                fork: (kind == LaunchEffectKind::Branch)
                    .then(|| launch.fork.as_ref().map(|fork| fork.repository.clone()))
                    .flatten(),
                branch: None,
            })
        }

        async fn publish_branch(
            &self,
            _context: &super::super::github::PublicationContext,
            _plan: &super::super::workspace_diff::PublicationPlan,
        ) -> Result<super::super::github::PublicationAdvance, GithubBrokerError> {
            self.publication_calls.fetch_add(1, Ordering::SeqCst);
            self.publication_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a publication answer was configured")
        }

        async fn reconcile_branch_publication(
            &self,
            _context: &super::super::github::PublicationContext,
            _plan: &super::super::workspace_diff::PublicationPlan,
        ) -> Result<super::super::github::PublicationAdvance, GithubBrokerError> {
            self.publication_calls.fetch_add(1, Ordering::SeqCst);
            self.publication_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a publication reconciliation answer was configured")
        }

        async fn write_pull_request(
            &self,
            _context: &super::super::github::PullRequestContext,
        ) -> Result<super::super::github::PullRequestAdvance, GithubBrokerError> {
            self.pull_request_write_calls.fetch_add(1, Ordering::SeqCst);
            self.pull_request_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a pull-request answer was configured")
        }

        async fn reconcile_pull_request(
            &self,
            _context: &super::super::github::PullRequestContext,
        ) -> Result<super::super::github::PullRequestAdvance, GithubBrokerError> {
            self.pull_request_reconcile_calls
                .fetch_add(1, Ordering::SeqCst);
            self.pull_request_answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a pull-request answer was configured")
        }
    }

    #[tokio::test]
    async fn workspace_import_reuses_complete_blobs_and_converges_partial_chunks_after_restart() {
        let launch = workspace_launch();
        let complete_bytes = b"already staged\n".to_vec();
        let partial_bytes = vec![0x5a; BASELINE_IMPORT_CHUNK_BYTES as usize + 3];
        let (source, blobs) = workspace_source_plan(
            &launch,
            vec![
                ("complete.txt", complete_bytes.clone()),
                ("partial.bin", partial_bytes.clone()),
            ],
        );
        let complete_blob = source
            .blobs
            .iter()
            .find(|blob| blob.provider_object_id == workspace_git_blob_id(&complete_bytes))
            .cloned()
            .unwrap();
        let partial_blob = source
            .blobs
            .iter()
            .find(|blob| blob.provider_object_id == workspace_git_blob_id(&partial_bytes))
            .cloned()
            .unwrap();
        let store = Arc::new(FakeImportStore::new([], []));
        store.stage_inventory(&source);
        store.stage_chunk(&complete_blob, 0, complete_bytes);
        store.stage_chunk(
            &partial_blob,
            0,
            partial_bytes[..BASELINE_IMPORT_CHUNK_BYTES as usize].to_vec(),
        );
        let github = Arc::new(FakeGithub::with_workspace(source, blobs));
        let executor = workspace_executor(Arc::clone(&github), Arc::clone(&store));

        let execution = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::WorkspaceImport,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await;

        assert!(matches!(
            execution,
            EffectExecution::Completed {
                result: LaunchEffectResult::WorkspaceImport { .. },
                ..
            }
        ));
        assert_eq!(
            *github.workspace_blob_calls.lock().unwrap(),
            vec![partial_blob.provider_object_id.clone()],
            "only the provider object with an incomplete chunk set may be refetched"
        );
        assert_eq!(
            *store.put_calls.lock().unwrap(),
            vec![
                (partial_blob.provider_object_id.clone(), 0),
                (partial_blob.provider_object_id.clone(), 1),
            ],
            "the retained first chunk must converge under its exact immutable key"
        );
        let state = store.state.lock().unwrap();
        assert!(state.completed.is_some());
        assert!(state.retired);
        assert!(state.chunks.is_empty());
    }

    #[tokio::test]
    async fn workspace_import_retries_finalization_without_refetching_staged_blobs() {
        let launch = workspace_launch();
        let (source, blobs) = workspace_source_plan(
            &launch,
            vec![
                ("first.txt", b"first\n".to_vec()),
                ("second.txt", b"second\n".to_vec()),
            ],
        );
        let store = Arc::new(FakeImportStore::new(
            [],
            [Err(TriggerStoreError::Unavailable(
                "final object upload timed out".into(),
            ))],
        ));
        let github = Arc::new(FakeGithub::with_workspace(source, blobs));
        let executor = workspace_executor(Arc::clone(&github), Arc::clone(&store));

        let first = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::WorkspaceImport, EffectClaimMode::Execute),
            )
            .await;
        assert!(matches!(
            first,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ..
                },
                ..
            }
        ));
        let initial_blob_calls = github.workspace_blob_calls.lock().unwrap().clone();
        assert_eq!(initial_blob_calls.len(), 2);

        let second = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::WorkspaceImport,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await;
        assert!(matches!(
            second,
            EffectExecution::Completed {
                result: LaunchEffectResult::WorkspaceImport { .. },
                ..
            }
        ));
        assert_eq!(
            *github.workspace_blob_calls.lock().unwrap(),
            initial_blob_calls,
            "a finalization retry must use the complete staged chunk inventory"
        );
        assert_eq!(store.finalize_calls.load(Ordering::SeqCst), 2);
        assert!(store.state.lock().unwrap().retired);
    }

    #[tokio::test]
    async fn workspace_import_reconciles_completed_object_before_launch_cas_without_github() {
        let launch = workspace_launch();
        let (source, blobs) =
            workspace_source_plan(&launch, vec![("README.md", b"hello\n".to_vec())]);
        let store = Arc::new(FakeImportStore::new([], []));
        let github = Arc::new(FakeGithub::with_workspace(source, blobs));
        let executor = workspace_executor(Arc::clone(&github), Arc::clone(&store));

        let first = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::WorkspaceImport, EffectClaimMode::Execute),
            )
            .await;
        assert!(matches!(
            first,
            EffectExecution::Completed {
                result: LaunchEffectResult::WorkspaceImport { .. },
                ..
            }
        ));
        assert_eq!(github.workspace_import_calls.load(Ordering::SeqCst), 1);
        let initial_blob_calls = github.workspace_blob_calls.lock().unwrap().clone();

        *github.workspace_source.lock().unwrap() = None;
        github.workspace_blobs.lock().unwrap().clear();
        let reconciled = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::WorkspaceImport,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await;

        assert!(matches!(
            reconciled,
            EffectExecution::Completed {
                result: LaunchEffectResult::WorkspaceImport { .. },
                ..
            }
        ));
        assert_eq!(
            github.workspace_import_calls.load(Ordering::SeqCst),
            1,
            "the committed object checkpoint must win before any provider read"
        );
        assert_eq!(
            *github.workspace_blob_calls.lock().unwrap(),
            initial_blob_calls
        );
        assert_eq!(store.retire_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn workspace_import_fails_closed_when_a_chunk_key_retains_different_bytes() {
        let launch = workspace_launch();
        let bytes = vec![0x2a; BASELINE_IMPORT_CHUNK_BYTES as usize + 1];
        let (source, blobs) = workspace_source_plan(&launch, vec![("large.bin", bytes.clone())]);
        let blob = source.blobs.first().cloned().unwrap();
        let store = Arc::new(FakeImportStore::new([], []));
        store.stage_inventory(&source);
        store.stage_chunk(&blob, 0, vec![0x7f; BASELINE_IMPORT_CHUNK_BYTES as usize]);
        let github = Arc::new(FakeGithub::with_workspace(source, blobs));
        let executor = workspace_executor(Arc::clone(&github), Arc::clone(&store));

        let execution = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::WorkspaceImport,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await;

        assert_workspace_integrity_failure(execution);
        assert_eq!(
            *github.workspace_blob_calls.lock().unwrap(),
            vec![blob.provider_object_id.clone()]
        );
        assert_eq!(store.finalize_calls.load(Ordering::SeqCst), 0);
        assert!(store.state.lock().unwrap().completed.is_none());
    }

    #[tokio::test]
    async fn workspace_import_rejects_staging_inventory_mismatch_before_blob_reads() {
        let launch = workspace_launch();
        let (source, blobs) =
            workspace_source_plan(&launch, vec![("README.md", b"hello\n".to_vec())]);
        let mut retained = source.clone();
        retained.tree_digest = "f".repeat(64);
        let store = Arc::new(FakeImportStore::new([], []));
        store.stage_inventory(&retained);
        let github = Arc::new(FakeGithub::with_workspace(source, blobs));
        let executor = workspace_executor(Arc::clone(&github), Arc::clone(&store));

        let execution = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::WorkspaceImport,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await;

        assert_workspace_integrity_failure(execution);
        assert_eq!(github.workspace_import_calls.load(Ordering::SeqCst), 1);
        assert!(github.workspace_blob_calls.lock().unwrap().is_empty());
        assert_eq!(store.finalize_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn workspace_import_fails_closed_on_a_conflicting_final_object() {
        let launch = workspace_launch();
        let (source, blobs) =
            workspace_source_plan(&launch, vec![("README.md", b"hello\n".to_vec())]);
        let store = Arc::new(FakeImportStore::new([], []));
        store.retain_conflicting_final_archive(b"different retained object".to_vec());
        let github = Arc::new(FakeGithub::with_workspace(source, blobs));
        let executor = workspace_executor(Arc::clone(&github), Arc::clone(&store));

        let execution = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::WorkspaceImport, EffectClaimMode::Execute),
            )
            .await;

        assert_workspace_integrity_failure(execution);
        assert_eq!(store.finalize_calls.load(Ordering::SeqCst), 1);
        assert!(store.state.lock().unwrap().completed.is_none());
    }

    #[tokio::test]
    async fn ambiguous_fork_write_is_never_returned_as_an_execute_retry() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Ambiguous]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let mut launch = fork_launch();
        let claim = claim(EffectClaimMode::Execute);

        let result = execute_with_admission(&executor, &mut launch, &claim).await;

        assert!(matches!(
            result,
            EffectExecution::ReconcileLater {
                failure: LaunchFailure {
                    class: FailureClass::AmbiguousExternal,
                    ..
                },
                ..
            }
        ));
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 1);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn expired_admitted_fork_never_writes_and_takeover_only_reconciles() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Ready]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let mut launch = fork_launch();
        let execute_claim = claim(EffectClaimMode::Execute);
        install_test_admission(&executor, &mut launch, &execute_claim).await;
        launch.lease.as_mut().unwrap().expires_at = "1970-01-01T00:00:01Z".into();

        let result = executor.execute(&launch, &execute_claim).await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ref code,
                    ..
                }
            } if code == "trigger_authority_lease_not_live"
        ));
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);

        launch.fencing_counter = 2;
        launch.lease = Some(layer2_core::WorkerLease {
            holder: "takeover-worker".into(),
            fencing_token: 2,
            acquired_at: "2026-08-27T00:02:01Z".into(),
            expires_at: "2099-08-27T00:00:00Z".into(),
        });
        let mut takeover = claim(EffectClaimMode::Reconcile);
        takeover.fence = 2;
        let result = executor.reconcile(&launch, &takeover).await;

        assert!(matches!(
            result,
            EffectExecution::Completed {
                result: LaunchEffectResult::Fork { .. },
                ..
            }
        ));
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 1);
        assert!(launch.effects.last().unwrap().authority_admission.is_some());
    }

    #[tokio::test]
    async fn takeover_reconciliation_completes_a_ready_fork_without_create() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Ready]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let launch = fork_launch();

        let result = executor
            .reconcile(&launch, &claim(EffectClaimMode::Reconcile))
            .await;

        assert!(matches!(
            result,
            EffectExecution::Completed {
                result: LaunchEffectResult::Fork { .. },
                ..
            }
        ));
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn occupied_network_slot_is_a_named_user_action_failure() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Occupied]));
        let executor = GithubEffectExecutor::with_port(github, Duration::from_secs(300));
        let mut launch = fork_launch();
        let claim = claim(EffectClaimMode::Execute);

        let result = execute_with_admission(&executor, &mut launch, &claim).await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::PolicyUserAction,
                    ref code,
                    ..
                }
            } if code == "fork_network_occupied"
        ));
    }

    #[tokio::test]
    async fn reconciliation_outage_does_not_reauthorize_the_fork_write() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Transient]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let launch = fork_launch();

        let result = executor
            .reconcile(&launch, &claim(EffectClaimMode::Reconcile))
            .await;

        assert!(matches!(result, EffectExecution::ReconcileLater { .. }));
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn absent_reconciliation_waits_on_the_same_intent() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Absent]));
        let executor = GithubEffectExecutor::with_port(github, Duration::from_secs(300));
        let launch = fork_launch();

        let result = executor
            .reconcile(&launch, &claim(EffectClaimMode::Reconcile))
            .await;

        assert!(matches!(
            result,
            EffectExecution::ReconcileLater {
                failure: LaunchFailure {
                    class: FailureClass::AmbiguousExternal,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn fork_readiness_polling_uses_stable_bounded_jitter() {
        let mut launch = fork_launch();
        launch.id = layer2_core::LaunchId("44444444-4444-4444-8444-444444444444".into());
        let mut first = claim(EffectClaimMode::Reconcile);
        first.intent_key = "intent-a".into();
        first.reconciliation_attempt = 4;
        let mut second = first.clone();
        second.intent_key = "intent-b".into();

        let delay = fork_reconciliation_delay(&launch, &first);
        assert_eq!(delay, fork_reconciliation_delay(&launch, &first));
        let upper = reconciliation_delay(first.reconciliation_attempt);
        assert!((upper.div_ceil(2)..=upper).contains(&delay));
        assert_ne!(
            delay,
            fork_reconciliation_delay(&launch, &second),
            "fixed distinct Launch intents should spread their readiness polls"
        );
    }

    #[test]
    fn fork_readiness_wait_preserves_the_original_deadline() {
        let github = Arc::new(FakeGithub::new([]));
        let executor = GithubEffectExecutor::with_port(github, Duration::from_secs(300));
        let mut launch = fork_launch();
        let mut claim = claim(EffectClaimMode::Reconcile);
        claim.reconciliation_attempt = 4;
        let deadline = rfc3339_at(SystemTime::now() + Duration::from_secs(120));
        launch.effects.push(layer2_core::EffectRecord {
            kind: claim.kind,
            state: layer2_core::EffectState::Reconciling,
            intent_key: claim.intent_key.clone(),
            attempt: claim.attempt,
            reconciliation_attempt: claim.reconciliation_attempt,
            fence: claim.fence,
            authority_admission: None,
            provider_request_id: None,
            external_resource_ids: BTreeMap::from([(
                "fork_readiness_deadline".into(),
                deadline.clone(),
            )]),
            next_attempt_at: None,
            last_failure: None,
            updated_at: "2026-08-27T00:00:00Z".into(),
        });

        let result = executor.defer_fork(
            &launch,
            &claim,
            Some(30),
            "fork_readiness_pending",
            "GitHub is still preparing the fork.",
        );
        let EffectExecution::ReconcileLater {
            external_resource_ids,
            ..
        } = result
        else {
            panic!("a live readiness deadline must remain reconcilable");
        };
        assert_eq!(external_resource_ids["fork_readiness_deadline"], deadline);
    }

    #[tokio::test]
    async fn expired_persisted_fork_readiness_deadline_requires_recovery_without_another_write() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Absent]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let mut launch = fork_launch();
        let claim = claim(EffectClaimMode::Reconcile);
        launch.effects.push(layer2_core::EffectRecord {
            kind: claim.kind,
            state: layer2_core::EffectState::Reconciling,
            intent_key: claim.intent_key.clone(),
            attempt: claim.attempt,
            reconciliation_attempt: claim.reconciliation_attempt,
            fence: claim.fence,
            authority_admission: None,
            provider_request_id: None,
            external_resource_ids: BTreeMap::from([(
                "fork_readiness_deadline".into(),
                "1970-01-01T00:00:01Z".into(),
            )]),
            next_attempt_at: None,
            last_failure: None,
            updated_at: "2026-08-27T00:00:00Z".into(),
        });

        let result = executor.reconcile(&launch, &claim).await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "fork_readiness_deadline_exceeded"
        ));
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn branch_protection_rejection_has_dedicated_user_action_remediation() {
        let result = broker_failure(
            GithubBrokerError::Rejected {
                status: 422,
                kind: super::super::github::GithubRejectionKind::BranchProtection,
                provider_code: Some("protected_branch".into()),
            },
            &claim_for(
                LaunchEffectKind::BranchPublication,
                EffectClaimMode::Execute,
            ),
            false,
        );

        let EffectExecution::Failed { failure } = result else {
            panic!("branch protection must be a terminal user-action failure");
        };
        assert_eq!(failure.class, FailureClass::PolicyUserAction);
        assert_eq!(failure.code, "github_branch_protection_refused");
        assert_ne!(failure.code, "github_policy_refused");
        assert!(failure.detail.contains("HTTP 422"));
        assert!(failure.detail.contains("branch protection"));
        assert!(failure
            .remediation
            .contains("CMA will not bypass protection"));
    }

    #[tokio::test]
    async fn ambiguous_pull_request_create_moves_to_reconciliation_without_a_second_write() {
        let github = Arc::new(FakeGithub::with_pull_request_answers([Err(
            GithubBrokerError::AmbiguousWrite,
        )]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let mut launch = pull_request_launch();
        let claim = claim_for(LaunchEffectKind::PullRequest, EffectClaimMode::Execute);

        let result = execute_with_admission(&executor, &mut launch, &claim).await;

        assert!(matches!(
            result,
            EffectExecution::ReconcileLater {
                failure: LaunchFailure {
                    class: FailureClass::AmbiguousExternal,
                    ..
                },
                ..
            }
        ));
        assert_eq!(github.pull_request_write_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            github.pull_request_reconcile_calls.load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn pull_request_takeover_searches_without_reauthorizing_create() {
        let launch = pull_request_launch();
        let ready = reconciled_pull_request(&launch, PullRequestState::Open);
        let github = Arc::new(FakeGithub::with_pull_request_answers([Ok(
            super::super::github::PullRequestAdvance::Ready(Box::new(ready)),
        )]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));

        let result = executor
            .reconcile(
                &launch,
                &claim_for(LaunchEffectKind::PullRequest, EffectClaimMode::Reconcile),
            )
            .await;

        assert!(matches!(
            result,
            EffectExecution::Completed {
                result: LaunchEffectResult::PullRequest {
                    pull_request: layer2_core::PullRequestRecord {
                        state: PullRequestState::Open,
                        number: Some(7),
                        ..
                    }
                },
                ..
            }
        ));
        assert_eq!(github.pull_request_write_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            github.pull_request_reconcile_calls.load(Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn external_pull_request_change_is_actionable_and_never_retried_automatically() {
        let github = Arc::new(FakeGithub::with_pull_request_answers([Err(
            GithubBrokerError::PullRequestChanged,
        )]));
        let executor = GithubEffectExecutor::with_port(github, Duration::from_secs(300));
        let mut launch = pull_request_launch();
        let claim = claim_for(LaunchEffectKind::PullRequest, EffectClaimMode::Execute);
        let result = execute_with_admission(&executor, &mut launch, &claim).await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::ConflictStaleHead,
                    ref code,
                    ..
                }
            } if code == "pull_request_changed"
        ));
    }

    #[test]
    fn pull_request_status_failures_use_the_closed_read_only_taxonomy() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum Lane {
            Retry,
            Suspended,
            FailLaunch,
        }

        let launch = pull_request_launch();
        let provider_request_id = "48800000-0000-4000-8000-000000000074";
        let cases = [
            (
                GithubBrokerError::RateLimited {
                    retry_after: Some("17".into()),
                },
                Lane::Retry,
                FailureClass::ResourceQuota,
                "github_rate_limited",
            ),
            (
                GithubBrokerError::Transient { status: Some(503) },
                Lane::Retry,
                FailureClass::RetryableTransient,
                "github_temporarily_unavailable",
            ),
            (
                GithubBrokerError::OwnerCredentialUnavailable,
                Lane::FailLaunch,
                FailureClass::OwnerCredentialUnavailable,
                "owner_credential_unavailable",
            ),
            (
                GithubBrokerError::ProviderAuthenticationUnavailable,
                Lane::FailLaunch,
                FailureClass::AuthRequired,
                "github_provider_authentication_unavailable",
            ),
            (
                GithubBrokerError::InactiveReconnectUnavailable,
                Lane::FailLaunch,
                FailureClass::AuthRequired,
                "github_provider_authentication_unavailable",
            ),
            (
                GithubBrokerError::ConnectionNotFound,
                Lane::FailLaunch,
                FailureClass::AuthRequired,
                "github_authorization_required",
            ),
            (
                GithubBrokerError::ConnectionNotPersonal,
                Lane::Suspended,
                FailureClass::PolicyUserAction,
                "github_status_policy_refused",
            ),
            (
                GithubBrokerError::ConnectionInactive,
                Lane::FailLaunch,
                FailureClass::ServiceInactive,
                "github_service_inactive",
            ),
            (
                GithubBrokerError::ConnectionDisconnected,
                Lane::FailLaunch,
                FailureClass::AuthRequired,
                "github_authorization_required",
            ),
            (
                GithubBrokerError::InsufficientScope {
                    required: "public_repo",
                },
                Lane::FailLaunch,
                FailureClass::AuthRequired,
                "github_authorization_required",
            ),
            (
                GithubBrokerError::AuthorityDrift,
                Lane::FailLaunch,
                FailureClass::TerminalSecurity,
                "github_pull_request_evidence_invalid",
            ),
            (
                GithubBrokerError::Forbidden,
                Lane::Suspended,
                FailureClass::PolicyUserAction,
                "github_status_policy_refused",
            ),
            (
                GithubBrokerError::NotFound,
                Lane::Suspended,
                FailureClass::ConflictStaleHead,
                "github_pull_request_not_found",
            ),
            (
                GithubBrokerError::AmbiguousWrite,
                Lane::FailLaunch,
                FailureClass::TerminalSecurity,
                "github_pull_request_evidence_invalid",
            ),
            (
                GithubBrokerError::ForkingDisabled,
                Lane::Suspended,
                FailureClass::PolicyUserAction,
                "github_status_policy_refused",
            ),
            (
                GithubBrokerError::ForkNetworkOccupied,
                Lane::Suspended,
                FailureClass::PolicyUserAction,
                "github_status_policy_refused",
            ),
            (
                GithubBrokerError::BranchHeadConflict,
                Lane::Suspended,
                FailureClass::ConflictStaleHead,
                "pull_request_status_stale",
            ),
            (
                GithubBrokerError::UpstreamBaseConflict,
                Lane::Suspended,
                FailureClass::ConflictStaleHead,
                "pull_request_status_stale",
            ),
            (
                GithubBrokerError::PullRequestChanged,
                Lane::FailLaunch,
                FailureClass::TerminalSecurity,
                "pull_request_identity_changed",
            ),
            (
                GithubBrokerError::UnsupportedWorkspace(
                    super::super::github::WorkspaceUnsupported::LargeFileStorage,
                ),
                Lane::Suspended,
                FailureClass::PolicyUserAction,
                "github_status_policy_refused",
            ),
            (
                GithubBrokerError::Rejected {
                    status: 422,
                    kind: super::super::github::GithubRejectionKind::Validation,
                    provider_code: Some("validation_failed".into()),
                },
                Lane::Suspended,
                FailureClass::PolicyUserAction,
                "github_status_policy_refused",
            ),
            (
                GithubBrokerError::Malformed("invalid status evidence".into()),
                Lane::FailLaunch,
                FailureClass::TerminalSecurity,
                "github_pull_request_evidence_invalid",
            ),
        ];

        for (error, expected_lane, expected_class, expected_code) in cases {
            let disposition = pull_request_status_failure(
                error,
                &launch,
                "status-taxonomy",
                Some(provider_request_id.into()),
            );
            let (lane, failure, retained_request_id, retry_deadline) = match disposition {
                PullRequestStatusDisposition::Observed { .. } => {
                    panic!("a broker failure cannot become a successful status observation")
                }
                PullRequestStatusDisposition::Retry {
                    failure,
                    next_attempt_at,
                    provider_request_id,
                } => (
                    Lane::Retry,
                    failure,
                    provider_request_id,
                    Some(next_attempt_at),
                ),
                PullRequestStatusDisposition::Suspended {
                    failure,
                    provider_request_id,
                } => (Lane::Suspended, failure, provider_request_id, None),
                PullRequestStatusDisposition::FailLaunch {
                    failure,
                    provider_request_id,
                } => (Lane::FailLaunch, failure, provider_request_id, None),
            };

            assert_eq!(lane, expected_lane, "wrong lane for {expected_code}");
            assert_eq!(
                failure.class, expected_class,
                "wrong class for {expected_code}"
            );
            assert_eq!(failure.code, expected_code);
            assert_eq!(failure.correlation_id, "status-taxonomy");
            assert_eq!(retained_request_id.as_deref(), Some(provider_request_id));
            match expected_lane {
                Lane::Retry => {
                    assert_eq!(failure.retry_at, retry_deadline);
                    assert!(failure.retry_at.is_some());
                }
                Lane::Suspended | Lane::FailLaunch => {
                    assert!(failure.retry_at.is_none());
                    assert!(retry_deadline.is_none());
                }
            }
        }
    }

    #[tokio::test]
    async fn sandbox_provisioning_is_reconciled_without_another_identity() {
        let sessions = Arc::new(FakeSessions::new([SessionAnswer::Evidence {
            status: SessionStatus::Provisioning,
            credential_ready: false,
            rotations: 0,
        }]));
        let executor = session_executor(sessions.clone());
        let launch = super::super::worker::tests::launch_fixture();

        let result = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Sandbox, EffectClaimMode::Execute),
            )
            .await;

        assert!(matches!(result, EffectExecution::WaitLater { .. }));
        assert_eq!(sessions.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ready_runtime_without_acknowledged_credential_remains_pending() {
        let sessions = Arc::new(FakeSessions::new([SessionAnswer::Evidence {
            status: SessionStatus::Ready,
            credential_ready: false,
            rotations: 0,
        }]));
        let executor = session_executor(sessions);
        let launch = super::super::worker::tests::launch_fixture();

        let result = executor
            .reconcile(
                &launch,
                &claim_for(LaunchEffectKind::Sandbox, EffectClaimMode::Reconcile),
            )
            .await;

        assert!(matches!(result, EffectExecution::WaitLater { .. }));
    }

    #[tokio::test]
    async fn recovery_and_rotation_are_waiting_states_not_runtime_readiness() {
        for (kind, status, recovery_state) in [
            (
                LaunchEffectKind::Sandbox,
                SessionStatus::Ready,
                RecoveryState::Recovering,
            ),
            (
                LaunchEffectKind::Session,
                SessionStatus::Running,
                RecoveryState::Unknown,
            ),
            (
                LaunchEffectKind::Sandbox,
                SessionStatus::Rotating,
                RecoveryState::Degraded,
            ),
        ] {
            let sessions = Arc::new(FakeSessions::new([SessionAnswer::EvidenceWithRecovery {
                status,
                recovery_state,
                credential_ready: true,
                rotations: 1,
            }]));
            let executor = session_executor(sessions);
            let launch = super::super::worker::tests::launch_fixture();

            let result = executor
                .reconcile(&launch, &claim_for(kind, EffectClaimMode::Reconcile))
                .await;

            assert!(
                matches!(result, EffectExecution::WaitLater { .. }),
                "{kind:?}/{status:?}/{recovery_state:?} advanced unexpectedly: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn degraded_session_requires_explicit_recovery() {
        let sessions = Arc::new(FakeSessions::new([SessionAnswer::EvidenceWithRecovery {
            status: SessionStatus::Running,
            recovery_state: RecoveryState::Degraded,
            credential_ready: true,
            rotations: 1,
        }]));
        let executor = session_executor(sessions);
        let launch = super::super::worker::tests::launch_fixture();

        let result = executor
            .reconcile(
                &launch,
                &claim_for(LaunchEffectKind::Session, EffectClaimMode::Reconcile),
            )
            .await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "trigger_session_recovery_required"
        ));
    }

    #[tokio::test]
    async fn acknowledged_ready_runtime_completes_the_sandbox_effect() {
        let sessions = Arc::new(FakeSessions::new([SessionAnswer::Evidence {
            status: SessionStatus::Ready,
            credential_ready: true,
            rotations: 0,
        }]));
        let executor = session_executor(sessions);
        let launch = super::super::worker::tests::launch_fixture();

        let result = executor
            .reconcile(
                &launch,
                &claim_for(LaunchEffectKind::Sandbox, EffectClaimMode::Reconcile),
            )
            .await;

        assert!(matches!(
            result,
            EffectExecution::Completed {
                result: LaunchEffectResult::Sandbox,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn session_effect_returns_the_exact_stable_attachment() {
        let sessions = Arc::new(FakeSessions::new([SessionAnswer::Evidence {
            status: SessionStatus::Running,
            credential_ready: true,
            rotations: 2,
        }]));
        let executor = session_executor(sessions);
        let launch = super::super::worker::tests::launch_fixture();

        let result = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Session, EffectClaimMode::Execute),
            )
            .await;

        assert!(matches!(
            result,
            EffectExecution::Completed {
                result: LaunchEffectResult::Session {
                    session: SessionLink {
                        ref session_id,
                        ref environment_profile_id,
                        runtime_generation: 3,
                        ref created_at,
                    }
                },
                ..
            } if session_id.0 == "sesn_trigger_effect"
                && environment_profile_id == "envp_trigger_effect"
                && created_at == "2026-08-27T00:00:00Z"
        ));
    }

    #[tokio::test]
    async fn stopped_deterministic_session_requires_explicit_recovery() {
        for status in [SessionStatus::Idle, SessionStatus::Terminated] {
            let sessions = Arc::new(FakeSessions::new([SessionAnswer::Evidence {
                status,
                credential_ready: true,
                rotations: 0,
            }]));
            let executor = session_executor(sessions);
            let launch = super::super::worker::tests::launch_fixture();

            let result = executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::Sandbox, EffectClaimMode::Execute),
                )
                .await;

            assert!(matches!(
                result,
                EffectExecution::Failed {
                    failure: LaunchFailure {
                        class: FailureClass::RecoveryRequired,
                        ..
                    }
                }
            ));
        }
    }

    #[tokio::test]
    async fn mismatched_session_authority_is_a_terminal_security_failure() {
        let sessions = Arc::new(FakeSessions::new([SessionAnswer::AuthorityMismatch]));
        let executor = session_executor(sessions);
        let launch = super::super::worker::tests::launch_fixture();

        let result = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Session, EffectClaimMode::Execute),
            )
            .await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ..
                }
            }
        ));
    }

    #[tokio::test]
    async fn credential_retirement_response_loss_reconciles_exact_deterministic_custody() {
        let sessions = Arc::new(FakeSessions::with_retirement_response_loss());
        let executor = session_executor(sessions.clone());
        let mut launch = super::super::worker::tests::launch_fixture();
        launch.state = LaunchState::Cancelled;
        launch.session = None;
        launch.reach = None;
        let expected = trigger_session_id(&launch.id).unwrap();

        assert_eq!(
            executor
                .retire_session_credential(&launch, 1)
                .await
                .unwrap(),
            expected
        );
        assert_eq!(
            executor
                .retire_session_credential(&launch, 2)
                .await
                .unwrap(),
            expected
        );
        assert_eq!(sessions.retirement_calls.load(Ordering::SeqCst), 2);
        assert!(sessions
            .retired_session_ids
            .lock()
            .unwrap()
            .iter()
            .all(|session_id| session_id == &expected));
    }

    #[test]
    fn capability_rebinding_accepts_locator_refresh_and_rejects_authority_drift() {
        let launch = reach_launch();
        let connection = launch.connection.as_ref().unwrap();
        let repository_evidence =
            |identity: RepositoryIdentity| super::super::github::RepositoryEvidence {
                html_url: format!(
                    "https://github.com/{}/{}",
                    identity.canonical_owner, identity.canonical_name
                ),
                identity,
                permissions: super::super::github::RepositoryPermissions {
                    pull: true,
                    push: true,
                    ..Default::default()
                },
                archived: false,
                disabled: false,
                subject_token_generation: 7,
            };

        let expected_upstream = &launch.revision.content.upstream.repository;
        let mut renamed_upstream = expected_upstream.clone();
        renamed_upstream.canonical_owner = "upstream-renamed".into();
        renamed_upstream.canonical_name = "project-renamed".into();
        renamed_upstream.default_branch = "trunk".into();
        assert!(validate_capability_upstream(
            expected_upstream,
            &repository_evidence(renamed_upstream.clone())
        )
        .is_ok());

        let expected_fork = &launch.fork.as_ref().unwrap().repository;
        let mut renamed_fork = expected_fork.clone();
        renamed_fork.canonical_owner = "contributor-renamed".into();
        renamed_fork.canonical_name = "project-renamed".into();
        renamed_fork.default_branch = "trunk".into();
        assert!(validate_capability_fork(
            expected_fork,
            &repository_evidence(renamed_fork.clone()),
            connection,
        )
        .is_ok());

        let mut transferred = renamed_fork;
        transferred.owner_id += 1;
        assert!(matches!(
            validate_capability_fork(expected_fork, &repository_evidence(transferred), connection,),
            Err(GithubBrokerError::AuthorityDrift)
        ));

        let mut missing_pull = repository_evidence(renamed_upstream);
        missing_pull.permissions.pull = false;
        assert!(matches!(
            validate_capability_upstream(expected_upstream, &missing_pull),
            Err(GithubBrokerError::AuthorityDrift)
        ));

        let mut refreshed_connection = connection.clone();
        refreshed_connection.account.login = "contributor-renamed".into();
        refreshed_connection.account.avatar_url = Some("https://avatars.example/new".into());
        refreshed_connection.granted_scopes.reverse();
        refreshed_connection.satisfied_scope_classes.reverse();
        assert!(same_connection_authority(connection, &refreshed_connection));
        refreshed_connection.account.account_node_id = "U_transferred".into();
        assert!(!same_connection_authority(
            connection,
            &refreshed_connection
        ));
    }

    #[tokio::test]
    async fn authority_admission_rejects_an_out_of_band_branch_advance() {
        let launch = reach_launch();
        let mut branch = launch.branch.clone().unwrap();
        branch.current_head = "e".repeat(40);
        let evidence = CapabilityAuthorityEvidence {
            connection: launch.connection.clone().unwrap(),
            authority: launch.authority.clone().unwrap(),
            upstream: launch.revision.content.upstream.repository.clone(),
            fork: launch.fork.as_ref().unwrap().repository.clone(),
            branch,
        };
        let github = Arc::new(FakeGithub::with_capability_evidence(evidence));
        let executor = GithubEffectExecutor::with_capability_ports(
            github,
            Arc::new(FakeSessions::new([])),
            test_signer(),
        );
        let claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);

        assert!(matches!(
            executor.admit(&launch, &claim).await,
            Err(EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ref code,
                    ..
                }
            }) if code == "trigger_authority_admission_invalid"
        ));
    }

    #[tokio::test]
    async fn reach_completes_only_after_the_exact_policy_tuple_is_acknowledged() {
        let sessions = Arc::new(FakeSessions::with_policy_receipt(
            layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 7,
                revoked: false,
            },
        ));
        let signer = test_signer();
        let executor = GithubEffectExecutor::with_capability_ports(
            Arc::new(FakeGithub::new([])),
            sessions.clone(),
            signer,
        );
        let mut launch = reach_launch();
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;

        let result = execute_with_admission(&executor, &mut launch, &reach_claim).await;

        assert!(matches!(
            &result,
            EffectExecution::Completed {
                result: LaunchEffectResult::Reach {
                    reach: layer2_core::ReachStatus {
                        policy_generation: 1,
                        state: CapabilityRevocationState::Active,
                        ..
                    },
                    ..
                },
                ..
            }
        ));
        let EffectExecution::Completed {
            result:
                LaunchEffectResult::Reach {
                    reach,
                    connection,
                    authority,
                    ..
                },
            external_resource_ids,
            ..
        } = result
        else {
            unreachable!("the Reach result was asserted above")
        };
        assert_eq!(reach.service_id, connection.service_id);
        assert_eq!(reach.runtime_audience, "sandbox-runtime-1");
        assert_eq!(authority.service_id, connection.service_id);
        assert_eq!(
            external_resource_ids.get("service_id"),
            Some(&connection.service_id)
        );
        assert_eq!(
            external_resource_ids.get("runtime_gateway_probe_version"),
            Some(&layer3_adapter::RUNTIME_GATEWAY_PROBE_VERSION.to_string())
        );
        assert_eq!(
            external_resource_ids.get("runtime_gateway_probe_outcome"),
            Some(&"allowed".to_string())
        );
        let policies = sessions.installed_policies.lock().unwrap();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].runtime_audience, "sandbox-runtime-1");
        assert_eq!(policies[0].fence, 7);
        assert_eq!(
            policies[0].allowed_ref,
            format!("refs/heads/cma/{}", launch.id.0)
        );
        let probes = sessions.gateway_probe_requests.lock().unwrap();
        assert_eq!(probes.len(), 1);
        assert_eq!(
            probes[0].version,
            layer3_adapter::RUNTIME_GATEWAY_PROBE_VERSION
        );
        assert_eq!(probes[0].service_id, connection.service_id);
        assert_eq!(probes[0].resource_id, 30);
    }

    #[tokio::test]
    async fn sponsored_reach_probes_then_activates_then_installs_before_initial_dispatch() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut launch = sponsored_reach_launch();
        let capability = live_sponsored_capability(&launch);
        let sessions = Arc::new(
            FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 7,
                revoked: false,
            })
            .with_events(events.clone()),
        );
        sessions
            .model_authority_answers
            .lock()
            .unwrap()
            .push_back(Ok(sponsored_authority_receipt(&capability)));
        let sponsorships = Arc::new(FakeSponsorships::new(
            [Ok(capability.clone())],
            events.clone(),
        ));
        let executor = sponsored_executor(sessions.clone(), sponsorships.clone());
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;

        let result = execute_with_admission(&executor, &mut launch, &reach_claim).await;
        let reach = match result {
            EffectExecution::Completed {
                result: LaunchEffectResult::Reach { reach, .. },
                ..
            } => reach,
            other => panic!("sponsored Reach must complete only after model authority: {other:?}"),
        };
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [
                "github_probe_allowed",
                "grant_activation",
                "model_authority_install"
            ]
        );
        assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 1);
        assert_eq!(sessions.prompt_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            sponsorships.contributors.lock().unwrap().as_slice(),
            std::slice::from_ref(&launch.owner)
        );
        assert_eq!(
            sessions
                .installed_model_capabilities
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&capability)
        );
        assert_eq!(
            sessions.installed_model_gateway_origins.lock().unwrap()[0].as_str(),
            "https://sponsored-model-gateway.example"
        );

        launch.reach = Some(reach);
        sessions
            .answers
            .lock()
            .unwrap()
            .push_back(SessionAnswer::Evidence {
                status: SessionStatus::Ready,
                credential_ready: true,
                rotations: 0,
            });
        sessions
            .prompt_answers
            .lock()
            .unwrap()
            .push_back(Ok(prompt_delivery(
                &launch,
                layer2_core::PromptDeliveryState::Queued,
            )));
        let dispatch = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::InitialDispatch, EffectClaimMode::Execute),
            )
            .await;
        assert!(matches!(
            dispatch,
            EffectExecution::RetryWithProgress {
                result: LaunchEffectResult::InitialDispatch { .. },
                ..
            }
        ));
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [
                "github_probe_allowed",
                "grant_activation",
                "model_authority_install",
                "prompt_dispatch"
            ]
        );
    }

    #[tokio::test]
    async fn sponsored_reach_pre_probe_failure_never_activates_or_installs_model_authority() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut launch = sponsored_reach_launch();
        let capability = live_sponsored_capability(&launch);
        let sessions = Arc::new(
            FakeSessions::with_reachability_error(
                layer2_core::RuntimeGatewayReachabilityError::PolicyDenied,
            )
            .with_events(events.clone()),
        );
        let sponsorships = Arc::new(FakeSponsorships::new([Ok(capability)], events.clone()));
        let executor = sponsored_executor(sessions.clone(), sponsorships.clone());

        let result = execute_with_admission(
            &executor,
            &mut launch,
            &claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute),
        )
        .await;

        assert!(matches!(result, EffectExecution::Failed { .. }));
        assert!(sponsorships.commands.lock().unwrap().is_empty());
        assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 0);
        assert!(sessions
            .installed_model_capabilities
            .lock()
            .unwrap()
            .is_empty());
        assert!(events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sponsored_provider_and_billing_ambiguity_always_enter_reconciliation() {
        for error in [
            SponsorshipError::ProviderAmbiguous,
            SponsorshipError::BillingAmbiguous,
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut launch = sponsored_reach_launch();
            let sessions = Arc::new(
                FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                    generation: 1,
                    fence: 7,
                    revoked: false,
                })
                .with_events(events.clone()),
            );
            let sponsorships = Arc::new(FakeSponsorships::new([Err(error)], events.clone()));
            let executor = sponsored_executor(sessions.clone(), sponsorships);
            let mut claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
            claim.fence = 7;

            let result = execute_with_admission(&executor, &mut launch, &claim).await;

            assert!(matches!(
                result,
                EffectExecution::ReconcileLater {
                    failure: LaunchFailure {
                        class: FailureClass::AmbiguousExternal,
                        ref code,
                        ..
                    },
                    ..
                } if code == "trigger_sponsored_activation_ambiguous"
            ));
            assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                events.lock().unwrap().as_slice(),
                ["github_probe_allowed", "grant_activation"]
            );
        }
    }

    #[tokio::test]
    async fn sponsored_historical_reconciliation_never_reactivates_or_reinstalls_authority() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut launch = sponsored_reach_launch();
        let capability = live_sponsored_capability(&launch);
        let sessions = Arc::new(
            FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 7,
                revoked: false,
            })
            .with_events(events.clone()),
        );
        *sessions.model_authority_answers.lock().unwrap() =
            VecDeque::from([Err(SponsoredModelAuthorityError::RuntimeUnavailable)]);
        let sponsorships = Arc::new(FakeSponsorships::new(
            [Ok(capability.clone())],
            events.clone(),
        ));
        let executor = sponsored_executor(sessions.clone(), sponsorships.clone());
        let mut execute_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        execute_claim.fence = 7;

        let first = execute_with_admission(&executor, &mut launch, &execute_claim).await;
        assert!(matches!(
            first,
            EffectExecution::ReconcileLater {
                failure: LaunchFailure {
                    ref code,
                    class: FailureClass::RetryableTransient,
                    ..
                },
                ..
            } if code == "trigger_sponsored_authority_install_pending"
        ));

        launch.revision_number = launch.revision_number.saturating_add(50);
        let first_reconciliation = promote_reach_to_reconciliation(&mut launch, &execute_claim, 8);
        assert!(matches!(
            executor.reconcile(&launch, &first_reconciliation).await,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "trigger_reach_install_outcome_unknown"
        ));

        launch.revision_number = launch.revision_number.saturating_add(75);
        let restart_reconciliation =
            promote_reach_to_reconciliation(&mut launch, &execute_claim, 9);
        assert!(matches!(
            executor.reconcile(&launch, &restart_reconciliation).await,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "trigger_reach_install_outcome_unknown"
        ));

        assert_eq!(sessions.installed_policies.lock().unwrap().len(), 1);
        assert_eq!(sessions.reconciliation_calls.load(Ordering::SeqCst), 0);
        assert!(sessions
            .reconciliation_probe_requests
            .lock()
            .unwrap()
            .is_empty());
        assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            sessions
                .installed_model_capabilities
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&capability)
        );
        let commands = sponsorships.commands.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].launch_id, launch.id);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [
                "github_probe_allowed",
                "grant_activation",
                "model_authority_install",
            ]
        );
    }

    #[tokio::test]
    async fn sponsored_runtime_authority_acknowledgement_mismatches_fail_closed_field_by_field() {
        for mismatch in 0..8 {
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut launch = sponsored_reach_launch();
            let capability = live_sponsored_capability(&launch);
            let mut receipt = sponsored_authority_receipt(&capability);
            match mismatch {
                0 => receipt.runtime_audience = "another-runtime".into(),
                1 => receipt.session_id = "sesn_another".into(),
                2 => receipt.session_credential_generation += 1,
                3 => receipt.launch_id = "11111111-1111-4111-8111-111111111111".into(),
                4 => receipt.grant_id = "33333333-3333-4333-8333-333333333333".into(),
                5 => receipt.grant_generation += 1,
                6 => receipt.policy_generation += 1,
                7 => receipt.capability_fingerprint = "e".repeat(64),
                _ => unreachable!(),
            }
            let sessions = Arc::new(
                FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                    generation: 1,
                    fence: 7,
                    revoked: false,
                })
                .with_events(events.clone()),
            );
            sessions
                .model_authority_answers
                .lock()
                .unwrap()
                .push_back(Ok(receipt));
            let sponsorships = Arc::new(FakeSponsorships::new([Ok(capability)], events.clone()));
            let executor = sponsored_executor(sessions.clone(), sponsorships);
            let mut claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
            claim.fence = 7;

            let result = execute_with_admission(&executor, &mut launch, &claim).await;

            assert!(matches!(
                result,
                EffectExecution::Failed {
                    failure: LaunchFailure {
                        class: FailureClass::TerminalSecurity,
                        ref code,
                        ..
                    }
                } if code == "trigger_sponsored_authority_acknowledgement_mismatch"
            ));
            assert_eq!(sessions.prompt_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn wrong_session_and_expired_sponsored_capabilities_never_reach_the_runtime() {
        for invalid in ["wrong_session", "expired"] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut launch = sponsored_reach_launch();
            let mut capability = live_sponsored_capability(&launch);
            match invalid {
                "wrong_session" => capability.session_id = SessionId("sesn_wrong".into()),
                "expired" => capability.expires_at = "1970-01-01T00:00:01Z".into(),
                _ => unreachable!(),
            }
            let sessions = Arc::new(
                FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                    generation: 1,
                    fence: 7,
                    revoked: false,
                })
                .with_events(events.clone()),
            );
            let sponsorships = Arc::new(FakeSponsorships::new([Ok(capability)], events.clone()));
            let executor = sponsored_executor(sessions.clone(), sponsorships);
            let mut claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
            claim.fence = 7;

            let result = execute_with_admission(&executor, &mut launch, &claim).await;

            assert!(matches!(
                result,
                EffectExecution::Failed {
                    failure: LaunchFailure {
                        class: FailureClass::TerminalSecurity,
                        ref code,
                        ..
                    }
                } if code == "trigger_sponsored_capability_invalid"
            ));
            assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn configured_sponsorship_does_not_change_ordinary_reach() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut launch = reach_launch();
        let sessions = Arc::new(
            FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 7,
                revoked: false,
            })
            .with_events(events.clone()),
        );
        let sponsorships = Arc::new(FakeSponsorships::new([], events.clone()));
        let executor = sponsored_executor(sessions.clone(), sponsorships.clone());
        let mut claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        claim.fence = 7;

        assert!(matches!(
            execute_with_admission(&executor, &mut launch, &claim).await,
            EffectExecution::Completed {
                result: LaunchEffectResult::Reach { .. },
                ..
            }
        ));
        assert!(sponsorships.commands.lock().unwrap().is_empty());
        assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 0);
        assert_eq!(events.lock().unwrap().as_slice(), ["github_probe_allowed"]);
    }

    #[test]
    fn sponsored_activation_identity_ignores_worker_and_launch_bookkeeping() {
        let mut launch = sponsored_reach_launch();
        let session_id = launch.session.as_ref().unwrap().session_id.clone();
        let stable = sponsored_activation_request_id(&launch, &session_id);

        launch.revision_number = 999;
        launch.fencing_counter = 88;
        launch.lease = Some(layer2_core::WorkerLease {
            holder: "replacement-worker".into(),
            fencing_token: 88,
            acquired_at: "2026-08-27T00:02:00Z".into(),
            expires_at: "2099-08-27T00:00:00Z".into(),
        });
        launch.effects.push(layer2_core::EffectRecord {
            kind: LaunchEffectKind::Reach,
            state: layer2_core::EffectState::Reconciling,
            intent_key: "replacement-reconciliation".into(),
            attempt: 9,
            reconciliation_attempt: 7,
            fence: 88,
            authority_admission: None,
            provider_request_id: None,
            external_resource_ids: BTreeMap::new(),
            next_attempt_at: None,
            last_failure: None,
            updated_at: "2026-08-27T00:02:00Z".into(),
        });
        assert_eq!(
            sponsored_activation_request_id(&launch, &session_id),
            stable
        );

        launch.revision_digest = "e".repeat(64);
        assert_ne!(
            sponsored_activation_request_id(&launch, &session_id),
            stable
        );
    }

    #[tokio::test]
    async fn expired_reach_lease_never_installs_runtime_policy() {
        let sessions = Arc::new(FakeSessions::with_policy_receipt(
            layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 7,
                revoked: false,
            },
        ));
        let executor = GithubEffectExecutor::with_capability_ports(
            Arc::new(FakeGithub::new([])),
            sessions.clone(),
            test_signer(),
        );
        let mut launch = reach_launch();
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;
        install_test_admission(&executor, &mut launch, &reach_claim).await;
        launch.lease.as_mut().unwrap().expires_at = "1970-01-01T00:00:01Z".into();

        let result = executor.execute(&launch, &reach_claim).await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ref code,
                    ..
                }
            } if code == "trigger_authority_lease_not_live"
        ));
        assert_eq!(sessions.gateway_probe_calls.load(Ordering::SeqCst), 0);
        assert!(sessions.installed_policies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn capability_renewal_requires_its_live_admission_but_reach_reconciliation_never_installs(
    ) {
        let sessions = Arc::new(FakeSessions::with_policy_receipt(
            layer3_adapter::RuntimePolicyReceipt {
                generation: 2,
                fence: 9,
                revoked: false,
            },
        ));
        let executor = GithubEffectExecutor::with_capability_ports(
            Arc::new(FakeGithub::new([])),
            sessions.clone(),
            test_signer(),
        );
        let mut launch = initial_dispatch_launch();
        let lease = layer2_core::WorkerLease {
            holder: "capability-renewal-worker".into(),
            fencing_token: 9,
            acquired_at: "2026-08-27T00:00:00Z".into(),
            expires_at: "2099-08-27T00:00:00Z".into(),
        };
        launch.fencing_counter = lease.fencing_token;
        launch.lease = Some(lease.clone());
        let target_expiry = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_add(CAPABILITY_MAX_LIFETIME_SECONDS);
        let admission = executor
            .admit_capability_renewal(&launch, lease.fencing_token, 2, target_expiry)
            .await
            .unwrap();
        launch
            .record_capability_authority_admission(&lease, admission, "2026-08-27T00:00:01Z")
            .unwrap();

        let renewal = executor
            .renew_capability(&launch, lease.fencing_token)
            .await
            .unwrap();
        assert_eq!(renewal.reach.policy_generation, 2);
        assert_eq!(sessions.gateway_probe_calls.load(Ordering::SeqCst), 1);
        assert_eq!(sessions.installed_policies.lock().unwrap().len(), 1);

        sessions.installed_policies.lock().unwrap().clear();
        sessions.gateway_probe_calls.store(0, Ordering::SeqCst);
        launch.lease.as_mut().unwrap().expires_at = "1970-01-01T00:00:01Z".into();
        let failure = executor
            .renew_capability(&launch, lease.fencing_token)
            .await
            .unwrap_err();
        assert_eq!(failure.class, FailureClass::TerminalSecurity);
        assert_eq!(failure.code, "trigger_capability_authority_lease_not_live");
        assert_eq!(sessions.gateway_probe_calls.load(Ordering::SeqCst), 0);
        assert!(sessions.installed_policies.lock().unwrap().is_empty());

        let result = executor
            .reconcile(
                &launch,
                &claim_for(LaunchEffectKind::Reach, EffectClaimMode::Reconcile),
            )
            .await;
        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "trigger_reach_install_outcome_unknown"
        ));
        assert_eq!(sessions.gateway_probe_calls.load(Ordering::SeqCst), 0);
        assert!(sessions.installed_policies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sponsored_capability_renewal_reinstalls_the_fresh_credential_generation() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sessions = Arc::new(
            FakeSessions::with_policy_receipt(layer3_adapter::RuntimePolicyReceipt {
                generation: 2,
                fence: 9,
                revoked: false,
            })
            .with_events(events.clone()),
        );
        let mut launch = initial_dispatch_launch();
        launch.revision.content.commercial_policy.billing_mode = BillingMode::Sponsored;
        let capability = live_sponsored_capability(&launch);
        let mut fresh_credential =
            test_credential_authority(&launch.session.as_ref().unwrap().session_id);
        fresh_credential.custody_generation = 2;
        fresh_credential.provider_key_id = "provider-session-key-2".into();
        *sessions.credential_authority.lock().unwrap() = Some(fresh_credential.clone());
        let mut model_receipt = sponsored_authority_receipt(&capability);
        model_receipt.session_credential_generation = fresh_credential.custody_generation;
        sessions
            .model_authority_answers
            .lock()
            .unwrap()
            .push_back(Ok(model_receipt));
        let sponsorships = Arc::new(FakeSponsorships::new(
            [Ok(capability.clone())],
            events.clone(),
        ));
        let executor = sponsored_executor(sessions.clone(), sponsorships.clone());
        let lease = layer2_core::WorkerLease {
            holder: "sponsored-capability-renewal-worker".into(),
            fencing_token: 9,
            acquired_at: "2026-08-27T00:00:00Z".into(),
            expires_at: "2099-08-27T00:00:00Z".into(),
        };
        launch.fencing_counter = lease.fencing_token;
        launch.lease = Some(lease.clone());
        let target_expiry = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_add(CAPABILITY_MAX_LIFETIME_SECONDS);
        let admission = executor
            .admit_capability_renewal(&launch, lease.fencing_token, 2, target_expiry)
            .await
            .unwrap();
        launch
            .record_capability_authority_admission(&lease, admission, "2026-08-27T00:00:01Z")
            .unwrap();

        let renewal = executor
            .renew_capability(&launch, lease.fencing_token)
            .await
            .unwrap();

        assert_eq!(renewal.reach.policy_generation, 2);
        assert_eq!(sessions.model_authority_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            sessions
                .installed_model_credential_authorities
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&fresh_credential)
        );
        assert_eq!(
            sessions
                .installed_model_capabilities
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&capability)
        );
        assert_eq!(sponsorships.commands.lock().unwrap().len(), 1);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            [
                "github_probe_allowed",
                "grant_activation",
                "model_authority_install"
            ]
        );
    }

    #[tokio::test]
    async fn reach_signs_producer_refreshed_names_after_a_canonical_rename() {
        let mut launch = reach_launch();
        let mut connection = launch.connection.clone().unwrap();
        connection.account.login = "contributor-renamed".into();
        connection.account.avatar_url = Some("https://avatars.example/contributor-renamed".into());
        let mut upstream = launch.revision.content.upstream.repository.clone();
        upstream.canonical_owner = "upstream-renamed".into();
        upstream.canonical_name = "project-renamed".into();
        let mut fork = launch.fork.as_ref().unwrap().repository.clone();
        fork.canonical_owner = "contributor-renamed".into();
        fork.canonical_name = "project-renamed".into();
        let mut branch = launch.branch.clone().unwrap();
        branch.canonical_url = format!(
            "https://github.com/contributor-renamed/project-renamed/tree/{}",
            branch.name
        );
        let evidence = CapabilityAuthorityEvidence {
            connection: connection.clone(),
            authority: launch.authority.clone().unwrap(),
            upstream: upstream.clone(),
            fork: fork.clone(),
            branch: branch.clone(),
        };
        let sessions = Arc::new(FakeSessions::with_policy_receipt(
            layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 7,
                revoked: false,
            },
        ));
        let executor = GithubEffectExecutor::with_capability_ports(
            Arc::new(FakeGithub::with_capability_evidence(evidence)),
            sessions.clone(),
            test_signer(),
        );
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;

        let execution = execute_with_admission(&executor, &mut launch, &reach_claim).await;

        let EffectExecution::Completed {
            result:
                LaunchEffectResult::Reach {
                    connection: returned_connection,
                    branch: returned_branch,
                    ..
                },
            ..
        } = execution
        else {
            panic!("a canonical rename must retain the numeric capability authority")
        };
        assert_eq!(returned_connection, connection);
        assert_eq!(returned_branch, branch);
        let policies = sessions.installed_policies.lock().unwrap();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].upstream, upstream);
        assert_eq!(policies[0].fork, fork);
    }

    #[tokio::test]
    async fn reach_refuses_a_runtime_acknowledgement_for_another_fence() {
        let sessions = Arc::new(FakeSessions::with_policy_receipt(
            layer3_adapter::RuntimePolicyReceipt {
                generation: 1,
                fence: 8,
                revoked: false,
            },
        ));
        let executor = GithubEffectExecutor::with_capability_ports(
            Arc::new(FakeGithub::new([])),
            sessions,
            test_signer(),
        );
        let mut launch = reach_launch();
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;

        let result = execute_with_admission(&executor, &mut launch, &reach_claim).await;

        assert!(matches!(
            result,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ref code,
                    ..
                }
            } if code == "trigger_reach_acknowledgement_mismatch"
        ));
    }

    #[tokio::test]
    async fn reachability_failures_are_closed_typed_and_redacted_before_prompt_dispatch() {
        use layer2_core::RuntimeGatewayReachabilityError as ReachabilityError;

        let cases = [
            (
                ReachabilityError::CommandSandboxInitialization,
                FailureClass::TerminalValidation,
                "trigger_runtime_command_sandbox_initialization_failed",
                false,
            ),
            (
                ReachabilityError::GatewayNetworkInitialization,
                FailureClass::TerminalValidation,
                "trigger_runtime_gateway_network_initialization_failed",
                false,
            ),
            (
                ReachabilityError::PolicyDenied,
                FailureClass::TerminalSecurity,
                "trigger_runtime_policy_denied",
                false,
            ),
            (
                ReachabilityError::ProviderDenied,
                FailureClass::AuthRequired,
                "trigger_runtime_provider_denied",
                false,
            ),
            (
                ReachabilityError::TransientBrokerFailure,
                FailureClass::RetryableTransient,
                "trigger_runtime_broker_temporarily_unavailable",
                true,
            ),
            (
                ReachabilityError::RuntimeUnavailable,
                FailureClass::RetryableTransient,
                "trigger_runtime_gateway_unavailable",
                true,
            ),
            (
                ReachabilityError::ProtocolUnsupported,
                FailureClass::TerminalValidation,
                "trigger_runtime_gateway_probe_unsupported",
                false,
            ),
            (
                ReachabilityError::ProtocolViolation,
                FailureClass::TerminalSecurity,
                "trigger_runtime_gateway_probe_mismatch",
                false,
            ),
            (
                ReachabilityError::Core(CoreError::InvalidRequest(
                    "credential=ghp_must_never_be_retained provider-body=private".into(),
                )),
                FailureClass::TerminalSecurity,
                "trigger_session_authority_mismatch",
                false,
            ),
        ];

        for (error, expected_class, expected_code, expected_retry) in cases {
            let sessions = Arc::new(FakeSessions::with_reachability_error(error));
            let executor = GithubEffectExecutor::with_capability_ports(
                Arc::new(FakeGithub::new([])),
                sessions.clone(),
                test_signer(),
            );
            let mut launch = reach_launch();
            let claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
            let result = execute_with_admission(&executor, &mut launch, &claim).await;

            let (failure, retry) = match &result {
                EffectExecution::Failed { failure } => (failure, false),
                EffectExecution::RetryLater { failure, .. } => (failure, true),
                other => panic!("unexpected reachability outcome: {other:?}"),
            };
            assert_eq!(failure.class, expected_class);
            assert_eq!(failure.code, expected_code);
            assert_eq!(retry, expected_retry);
            assert_eq!(sessions.gateway_probe_calls.load(Ordering::SeqCst), 1);
            assert_eq!(sessions.prompt_calls.load(Ordering::SeqCst), 0);
            let retained = format!("{result:?}");
            for secret in ["ghp_must_never_be_retained", "provider-body=private"] {
                assert!(!retained.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn initial_dispatch_maps_every_delivery_state_without_collapsing_progress() {
        use layer2_core::PromptDeliveryState as State;

        for state in [
            State::Queued,
            State::DispatchFenced,
            State::WriteConfirmed,
            State::ExecutionObserved,
            State::Completed,
            State::Failed,
            State::RecoveryRequired,
        ] {
            let launch = initial_dispatch_launch();
            let delivery = prompt_delivery(&launch, state);
            let sessions = Arc::new(FakeSessions::with_prompt_answers([Ok(delivery.clone())]));
            let executor = session_executor(Arc::clone(&sessions));
            let result = executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::InitialDispatch, EffectClaimMode::Execute),
                )
                .await;

            match state {
                State::Queued => assert!(matches!(
                    result,
                    EffectExecution::RetryWithProgress {
                        result: LaunchEffectResult::InitialDispatch { delivery: observed },
                        failure: LaunchFailure {
                            class: FailureClass::RetryableTransient,
                            ..
                        },
                        ..
                    } if observed == delivery
                )),
                State::WriteConfirmed | State::ExecutionObserved => assert!(matches!(
                    result,
                    EffectExecution::ReconcileWithProgress {
                        result: LaunchEffectResult::InitialDispatch { delivery: observed },
                        failure: LaunchFailure {
                            class: FailureClass::RetryableTransient,
                            ..
                        },
                        ..
                    } if observed == delivery
                )),
                State::Completed => assert!(matches!(
                    result,
                    EffectExecution::Completed {
                        result: LaunchEffectResult::InitialDispatch { delivery: observed },
                        ..
                    } if observed == delivery
                )),
                State::Failed => assert!(matches!(
                    result,
                    EffectExecution::FailedWithProgress {
                        result: LaunchEffectResult::InitialDispatch { delivery: observed },
                        failure: LaunchFailure {
                            class: FailureClass::PolicyUserAction,
                            ..
                        },
                    } if observed == delivery
                )),
                State::DispatchFenced | State::RecoveryRequired => assert!(matches!(
                    result,
                    EffectExecution::FailedWithProgress {
                        result: LaunchEffectResult::InitialDispatch { delivery: observed },
                        failure: LaunchFailure {
                            class: FailureClass::RecoveryRequired,
                            ..
                        },
                    } if observed == delivery
                )),
            }
            assert_eq!(sessions.prompt_calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn initial_dispatch_waits_for_session_lifecycle_without_sending_or_resending() {
        use layer2_core::PromptDeliveryState as State;

        for (delivery_state, status, recovery_state, mode) in [
            (
                State::Queued,
                SessionStatus::Rotating,
                RecoveryState::Normal,
                EffectClaimMode::Execute,
            ),
            (
                State::WriteConfirmed,
                SessionStatus::Ready,
                RecoveryState::Recovering,
                EffectClaimMode::Reconcile,
            ),
            (
                State::ExecutionObserved,
                SessionStatus::Ready,
                RecoveryState::Unknown,
                EffectClaimMode::Reconcile,
            ),
            (
                State::Queued,
                SessionStatus::Provisioning,
                RecoveryState::Normal,
                EffectClaimMode::Execute,
            ),
        ] {
            let mut launch = initial_dispatch_launch();
            if delivery_state != State::Queued {
                launch.delivery = Some(prompt_delivery(&launch, delivery_state));
            }
            let sessions = Arc::new(FakeSessions::new([SessionAnswer::EvidenceWithRecovery {
                status,
                recovery_state,
                credential_ready: true,
                rotations: 1,
            }]));
            let executor = session_executor(Arc::clone(&sessions));
            let claim = claim_for(LaunchEffectKind::InitialDispatch, mode);
            let result = match mode {
                EffectClaimMode::Execute => executor.execute(&launch, &claim).await,
                EffectClaimMode::Reconcile => executor.reconcile(&launch, &claim).await,
            };

            assert!(matches!(result, EffectExecution::WaitLater { .. }));
            assert_eq!(sessions.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                sessions.prompt_calls.load(Ordering::SeqCst),
                0,
                "lifecycle wait for {delivery_state:?} must not cross the prompt boundary"
            );
        }
    }

    #[tokio::test]
    async fn initial_dispatch_rotation_refusal_and_recovery_race_remain_waits() {
        let launch = initial_dispatch_launch();
        let claim = claim_for(LaunchEffectKind::InitialDispatch, EffectClaimMode::Execute);
        let rotating = Arc::new(FakeSessions::with_prompt_answers([Err(
            CoreError::SessionRotating,
        )]));
        let executor = session_executor(Arc::clone(&rotating));

        assert!(matches!(
            executor.execute(&launch, &claim).await,
            EffectExecution::WaitLater { .. }
        ));
        assert_eq!(rotating.calls.load(Ordering::SeqCst), 1);
        assert_eq!(rotating.prompt_calls.load(Ordering::SeqCst), 1);

        let recovering = Arc::new(FakeSessions::with_prompt_answers([Err(
            CoreError::RuntimeUnavailable("rotation raced initial dispatch".into()),
        )]));
        *recovering.answers.lock().unwrap() = VecDeque::from([
            SessionAnswer::Evidence {
                status: SessionStatus::Ready,
                credential_ready: true,
                rotations: 0,
            },
            SessionAnswer::EvidenceWithRecovery {
                status: SessionStatus::Provisioning,
                recovery_state: RecoveryState::Recovering,
                credential_ready: true,
                rotations: 1,
            },
        ]);
        let executor = session_executor(Arc::clone(&recovering));

        assert!(matches!(
            executor.execute(&launch, &claim).await,
            EffectExecution::WaitLater { .. }
        ));
        assert_eq!(recovering.calls.load(Ordering::SeqCst), 2);
        assert_eq!(recovering.prompt_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn initial_dispatch_without_an_active_reach_never_calls_the_session_port() {
        for reach in [None, Some(CapabilityRevocationState::Revoked)] {
            let mut launch = initial_dispatch_launch();
            launch.reach = reach.map(|state| {
                let mut reach = launch.reach.clone().unwrap();
                reach.state = state;
                reach.revocation_requested_at = Some("2026-08-27T00:00:00Z".into());
                reach.revocation_effective_at = Some("2026-08-27T00:00:01Z".into());
                reach.tombstone_acknowledged = true;
                reach
            });
            let sessions = Arc::new(FakeSessions::with_prompt_answers([]));
            let executor = session_executor(Arc::clone(&sessions));

            let result = executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::InitialDispatch, EffectClaimMode::Execute),
                )
                .await;

            assert!(matches!(
                result,
                EffectExecution::Failed {
                    failure: LaunchFailure {
                        class: FailureClass::TerminalSecurity,
                        ref code,
                        ..
                    }
                } if code == "trigger_initial_prompt_reach_unproven"
            ));
            assert_eq!(sessions.prompt_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn checkpoint_completion_requires_runtime_manifest_and_destination_proof() {
        let launch = checkpoint_launch();
        let package = checkpoint_package(&launch);
        let sessions = Arc::new(FakeSessions::with_checkpoint_answers([Ok((
            package.clone(),
            1,
        ))]));
        let store = Arc::new(FakeCheckpointStore::new([Ok(
            "trigger-test/checkpoint-v1.pack".into(),
        )]));
        let executor = GithubEffectExecutor::with_checkpoint_ports(
            Arc::new(FakeGithub::new([])),
            sessions.clone(),
            store.clone(),
        );

        let result = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Checkpoint, EffectClaimMode::Execute),
            )
            .await;
        let EffectExecution::Completed {
            result: LaunchEffectResult::Checkpoint { checkpoint },
            ..
        } = result
        else {
            panic!("a destination-proven checkpoint should complete");
        };
        assert_eq!(checkpoint.version, 1);
        assert_eq!(checkpoint.workspace_epoch, 1);
        assert_eq!(checkpoint.runtime_generation, 1);
        assert_eq!(
            checkpoint.object_reference,
            "trigger-test/checkpoint-v1.pack"
        );
        assert_eq!(checkpoint.manifest.digest, package.manifest_digest);
        assert_eq!(checkpoint.archive_sha256, package.archive_sha256);
        assert_eq!(sessions.checkpoint_calls.load(Ordering::SeqCst), 1);
        let requests = sessions.checkpoint_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].epoch, 1);
        assert_eq!(
            requests[0].baseline_manifest_digest,
            launch.workspace.as_ref().unwrap().baseline.digest
        );
        drop(requests);
        let writes = store.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, launch.owner);
        assert_eq!(writes[0].2, package.archive);
        assert!(writes[0].1.object_reference.is_empty());
    }

    #[tokio::test]
    async fn checkpoint_storage_retry_reuses_the_same_runtime_version_and_object_identity() {
        let launch = checkpoint_launch();
        let package = checkpoint_package(&launch);
        let sessions = Arc::new(FakeSessions::with_checkpoint_answers([
            Ok((package.clone(), 1)),
            Ok((package.clone(), 1)),
        ]));
        let store = Arc::new(FakeCheckpointStore::new([
            Err(TriggerStoreError::Unavailable("storage timeout".into())),
            Ok("trigger-test/checkpoint-v1.pack".into()),
        ]));
        let executor = GithubEffectExecutor::with_checkpoint_ports(
            Arc::new(FakeGithub::new([])),
            sessions.clone(),
            store.clone(),
        );
        let claim = claim_for(LaunchEffectKind::Checkpoint, EffectClaimMode::Execute);

        assert!(matches!(
            executor.execute(&launch, &claim).await,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ref code,
                    ..
                },
                ..
            } if code == "checkpoint_storage_unavailable"
        ));
        assert!(matches!(
            executor.reconcile(&launch, &claim).await,
            EffectExecution::Completed {
                result: LaunchEffectResult::Checkpoint { .. },
                ..
            }
        ));

        let requests = sessions.checkpoint_requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        drop(requests);
        let writes = store.writes.lock().unwrap();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].1.id, writes[1].1.id);
        assert_eq!(writes[0].1.version, writes[1].1.version);
        assert_eq!(writes[0].2, writes[1].2);
    }

    #[tokio::test]
    async fn checkpoint_failures_keep_storage_and_recovery_classification_explicit() {
        let launch = checkpoint_launch();
        let claim = claim_for(LaunchEffectKind::Checkpoint, EffectClaimMode::Execute);
        for (error, class, code) in [
            (
                CoreError::CapabilityUnsupported("workspace_checkpoint_v2".into()),
                FailureClass::TerminalValidation,
                "checkpoint_runtime_unsupported",
            ),
            (
                CoreError::RuntimeUnavailable("runtime lost".into()),
                FailureClass::RecoveryRequired,
                "checkpoint_runtime_lost",
            ),
            (
                CoreError::RuntimeRefused("writers not quiescent".into()),
                FailureClass::RecoveryRequired,
                "checkpoint_quiescence_unproven",
            ),
            (
                CoreError::InvalidRequest("wrong authority".into()),
                FailureClass::TerminalSecurity,
                "checkpoint_authority_mismatch",
            ),
        ] {
            let sessions = Arc::new(FakeSessions::with_checkpoint_answers([Err(error)]));
            let store = Arc::new(FakeCheckpointStore::new([]));
            let executor = GithubEffectExecutor::with_checkpoint_ports(
                Arc::new(FakeGithub::new([])),
                sessions,
                store.clone(),
            );
            assert!(matches!(
                executor.execute(&launch, &claim).await,
                EffectExecution::Failed {
                    failure: LaunchFailure {
                        class: observed,
                        code: observed_code,
                        ..
                    }
                } if observed == class && observed_code == code
            ));
            assert!(store.writes.lock().unwrap().is_empty());
        }

        let mut mismatched = checkpoint_package(&launch);
        mismatched.manifest_digest = "0".repeat(64);
        let sessions = Arc::new(FakeSessions::with_checkpoint_answers([Ok((mismatched, 1))]));
        let store = Arc::new(FakeCheckpointStore::new([]));
        let executor = GithubEffectExecutor::with_checkpoint_ports(
            Arc::new(FakeGithub::new([])),
            sessions,
            store.clone(),
        );
        assert!(matches!(
            executor.execute(&launch, &claim).await,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "checkpoint_manifest_mismatch"
        ));
        assert!(store.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn diff_executor_retains_named_missing_and_corrupt_outcomes_without_github_writes() {
        for (baseline, checkpoint, expected) in [
            (
                Ok(None),
                Ok(Some(b"unused checkpoint".to_vec())),
                DiffValidationOutcome::MissingBaseline,
            ),
            (
                Ok(Some(b"unused baseline".to_vec())),
                Ok(None),
                DiffValidationOutcome::CorruptCheckpoint,
            ),
        ] {
            let launch = diff_launch();
            let github = Arc::new(FakeGithub::new([]));
            let store = Arc::new(FakeReviewStore::new(
                [baseline],
                [checkpoint],
                [Ok("trigger-test/diff-artifact.json".into())],
            ));
            let executor = diff_executor(github.clone(), store.clone());

            let result = executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::Diff, EffectClaimMode::Execute),
                )
                .await;
            let EffectExecution::Completed {
                result: LaunchEffectResult::Diff { diff },
                ..
            } = result
            else {
                panic!("a named, non-publishable review outcome should complete");
            };
            assert_eq!(diff.validation, expected);
            assert!(diff.validate_for_review(&launch).is_ok());
            assert!(diff.validate_for_publication(&launch).is_err());
            assert_eq!(store.diff_writes.lock().unwrap().len(), 1);
            assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
            assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn diff_executor_retries_storage_outages_without_retaining_or_mutating() {
        let launch = diff_launch();
        let github = Arc::new(FakeGithub::new([]));
        let unavailable = Arc::new(FakeReviewStore::new(
            [Err(TriggerStoreError::Unavailable(
                "baseline timeout".into(),
            ))],
            [],
            [],
        ));
        let executor = diff_executor(github.clone(), unavailable.clone());
        assert!(matches!(
            executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::Diff, EffectClaimMode::Execute),
                )
                .await,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ref code,
                    ..
                },
                ..
            } if code == "diff_baseline_temporarily_unavailable"
        ));
        assert!(unavailable.diff_writes.lock().unwrap().is_empty());

        let destination = Arc::new(FakeReviewStore::new(
            [Ok(None)],
            [Ok(None)],
            [Err(TriggerStoreError::Unavailable("object timeout".into()))],
        ));
        let executor = diff_executor(github.clone(), destination.clone());
        assert!(matches!(
            executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::Diff, EffectClaimMode::Execute),
                )
                .await,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ref code,
                    ..
                },
                ..
            } if code == "diff_artifact_storage_unavailable"
        ));
        assert_eq!(destination.diff_writes.lock().unwrap().len(), 1);
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn publication_executor_requires_confirmation_before_any_github_or_storage_call() {
        let mut fixture = publication_launch();
        fixture.launch.contribution = None;
        let github = Arc::new(FakeGithub::new([]));
        let store = Arc::new(FakeReviewStore::with_publication_reads([], []));
        let executor = publication_executor(github.clone(), store);

        assert!(matches!(
            executor
                .execute(
                    &fixture.launch,
                    &claim_for(
                        LaunchEffectKind::BranchPublication,
                        EffectClaimMode::Execute,
                    ),
                )
                .await,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ref code,
                    ..
                }
            } if code == "publication_confirmation_missing"
        ));
        assert_eq!(github.publication_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn publication_executor_retains_the_object_recipe_before_object_writes() {
        let fixture = publication_launch();
        let blob_id = fixture.launch.diff.as_ref().unwrap().files[0]
            .new_provider_object_id
            .clone()
            .unwrap();
        let pending = super::super::github::PendingGitData {
            base_tree_id: "b".repeat(40),
            tree_id: None,
            object_ids: vec![blob_id],
            object_write: layer2_core::PublicationObjectWrite {
                version: layer2_core::PublicationObjectWrite::VERSION,
                request_digest: "9".repeat(64),
                phase: layer2_core::PublicationObjectPhase::BlobsPending,
            },
        };
        let github = Arc::new(FakeGithub::with_publication_answers([Ok(
            super::super::github::PublicationAdvance::ObjectWritePending(pending.clone()),
        )]));
        let store = Arc::new(FakeReviewStore::with_publication_reads(
            [Ok(Some(fixture.checkpoint_archive.clone()))],
            [Ok(Some(fixture.artifact_object.clone()))],
        ));
        let executor = publication_executor(github.clone(), store);
        let mut launch = fixture.launch;
        let claim = claim_for(
            LaunchEffectKind::BranchPublication,
            EffectClaimMode::Execute,
        );

        let EffectExecution::Progressed {
            result: LaunchEffectResult::BranchPublication { contribution },
            ..
        } = execute_with_admission(&executor, &mut launch, &claim).await
        else {
            panic!("the sealed object recipe must be persisted before any object write");
        };
        assert_eq!(contribution.state, PublicationState::IntentRecorded);
        assert_eq!(
            contribution.base_tree_id.as_deref(),
            Some(pending.base_tree_id.as_str())
        );
        assert_eq!(contribution.object_ids, pending.object_ids);
        assert_eq!(contribution.object_write, Some(pending.object_write));
        assert_eq!(github.publication_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn publication_storage_outages_and_missing_objects_have_named_boundaries() {
        let fixture = publication_launch();
        let github = Arc::new(FakeGithub::new([]));
        let claim = claim_for(
            LaunchEffectKind::BranchPublication,
            EffectClaimMode::Execute,
        );

        let artifact_outage = Arc::new(FakeReviewStore::with_publication_reads(
            [],
            [Err(TriggerStoreError::Unavailable(
                "artifact outage".into(),
            ))],
        ));
        let artifact_executor = publication_executor(github.clone(), artifact_outage);
        let mut artifact_launch = fixture.launch.clone();
        assert!(matches!(
            execute_with_admission(&artifact_executor, &mut artifact_launch, &claim).await,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ref code,
                    ..
                },
                ..
            } if code == "publication_artifact_temporarily_unavailable"
        ));

        let checkpoint_outage = Arc::new(FakeReviewStore::with_publication_reads(
            [Err(TriggerStoreError::Unavailable(
                "checkpoint outage".into(),
            ))],
            [Ok(Some(fixture.artifact_object.clone()))],
        ));
        let checkpoint_executor = publication_executor(github.clone(), checkpoint_outage);
        let mut checkpoint_launch = fixture.launch.clone();
        assert!(matches!(
            execute_with_admission(&checkpoint_executor, &mut checkpoint_launch, &claim).await,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ref code,
                    ..
                },
                ..
            } if code == "publication_checkpoint_temporarily_unavailable"
        ));

        let missing = Arc::new(FakeReviewStore::with_publication_reads([], [Ok(None)]));
        let missing_executor = publication_executor(github.clone(), missing);
        let mut missing_launch = fixture.launch.clone();
        assert!(matches!(
            execute_with_admission(&missing_executor, &mut missing_launch, &claim).await,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::RecoveryRequired,
                    ref code,
                    ..
                }
            } if code == "publication_artifact_missing"
        ));
        assert_eq!(github.publication_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn publication_executor_maps_all_durable_phases_without_changing_authority() {
        let fixture = publication_launch();
        let blob_id = fixture.launch.diff.as_ref().unwrap().files[0]
            .new_provider_object_id
            .clone()
            .unwrap();
        let prepared = super::super::github::PreparedGitData {
            base_tree_id: "b".repeat(40),
            tree_id: "c".repeat(40),
            commit_id: "d".repeat(40),
            object_ids: vec![blob_id],
        };
        let published = super::super::github::PublishedGitData {
            base_tree_id: prepared.base_tree_id.clone(),
            tree_id: prepared.tree_id.clone(),
            commit_id: prepared.commit_id.clone(),
            resulting_head: prepared.commit_id.clone(),
            object_ids: prepared.object_ids.clone(),
        };
        let github = Arc::new(FakeGithub::with_publication_answers([
            Ok(super::super::github::PublicationAdvance::ObjectsWritten(
                prepared.clone(),
            )),
            Ok(super::super::github::PublicationAdvance::RefUpdatePending(
                prepared.clone(),
            )),
            Ok(super::super::github::PublicationAdvance::Published(
                published.clone(),
            )),
        ]));
        let store = Arc::new(FakeReviewStore::with_publication_reads(
            [
                Ok(Some(fixture.checkpoint_archive.clone())),
                Ok(Some(fixture.checkpoint_archive.clone())),
                Ok(Some(fixture.checkpoint_archive.clone())),
            ],
            [
                Ok(Some(fixture.artifact_object.clone())),
                Ok(Some(fixture.artifact_object.clone())),
                Ok(Some(fixture.artifact_object.clone())),
            ],
        ));
        let executor = publication_executor(github.clone(), store);
        let mut launch = fixture.launch;
        let immutable = launch.contribution.clone().unwrap();
        let mut phase_claim = claim_for(
            LaunchEffectKind::BranchPublication,
            EffectClaimMode::Execute,
        );

        let EffectExecution::Progressed {
            result: LaunchEffectResult::BranchPublication { contribution },
            ..
        } = execute_with_admission(&executor, &mut launch, &phase_claim).await
        else {
            panic!("object creation must yield durable progress");
        };
        assert_eq!(contribution.state, PublicationState::ObjectsWritten);
        assert_publication_authority_unchanged(&immutable, &contribution);
        launch.contribution = Some(contribution);
        phase_claim.attempt = 2;
        phase_claim.fence = 2;

        let EffectExecution::Progressed {
            result: LaunchEffectResult::BranchPublication { contribution },
            ..
        } = execute_with_admission(&executor, &mut launch, &phase_claim).await
        else {
            panic!("freshly admitted object verification must yield a durable ref intent");
        };
        assert_eq!(contribution.state, PublicationState::RefUpdatePending);
        assert_publication_authority_unchanged(&immutable, &contribution);
        launch.contribution = Some(contribution);
        phase_claim.attempt = 3;
        phase_claim.fence = 3;

        let EffectExecution::Completed {
            result: LaunchEffectResult::BranchPublication { contribution },
            external_resource_ids,
            ..
        } = execute_with_admission(&executor, &mut launch, &phase_claim).await
        else {
            panic!("a freshly admitted guarded ref update must complete publication");
        };
        assert_eq!(contribution.state, PublicationState::Published);
        assert_eq!(
            contribution.resulting_head,
            Some(published.commit_id.clone())
        );
        assert_publication_authority_unchanged(&immutable, &contribution);
        assert_eq!(external_resource_ids["commit_id"], published.commit_id);
        assert_eq!(github.publication_calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn publication_executor_keeps_ambiguous_stale_scope_and_quota_distinct() {
        let cases = [
            (
                GithubBrokerError::AmbiguousWrite,
                FailureClass::AmbiguousExternal,
                "github_write_ambiguous",
            ),
            (
                GithubBrokerError::BranchHeadConflict,
                FailureClass::ConflictStaleHead,
                "conflict_stale_head",
            ),
            (
                GithubBrokerError::Rejected {
                    status: 403,
                    kind: super::super::github::GithubRejectionKind::WorkflowScope,
                    provider_code: Some("missing_workflow_scope".into()),
                },
                FailureClass::PolicyUserAction,
                "github_workflow_scope_refused",
            ),
            (
                GithubBrokerError::RateLimited {
                    retry_after: Some("17".into()),
                },
                FailureClass::ResourceQuota,
                "github_rate_limited",
            ),
        ];
        for (error, class, code) in cases {
            let fixture = publication_launch();
            let github = Arc::new(FakeGithub::with_publication_answers([Err(error)]));
            let store = Arc::new(FakeReviewStore::with_publication_reads(
                [Ok(Some(fixture.checkpoint_archive.clone()))],
                [Ok(Some(fixture.artifact_object.clone()))],
            ));
            let executor = publication_executor(github, store);
            let mut launch = fixture.launch;
            let claim = claim_for(
                LaunchEffectKind::BranchPublication,
                EffectClaimMode::Execute,
            );
            let execution = execute_with_admission(&executor, &mut launch, &claim).await;
            match execution {
                EffectExecution::RetryLater { failure, .. }
                | EffectExecution::ReconcileLater { failure, .. }
                | EffectExecution::Failed { failure } => {
                    assert_eq!(failure.class, class);
                    assert_eq!(failure.code, code);
                }
                other => panic!("unexpected failure projection: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn cleanup_deletes_only_the_exact_retained_cma_objects() {
        let launch = cleanup_launch();
        let targets = launch.cleanup.as_ref().unwrap().plan.targets.clone();
        let checkpoint = launch
            .workspace
            .as_ref()
            .unwrap()
            .latest_checkpoint
            .as_ref()
            .unwrap();
        let diff = launch.diff.as_ref().unwrap();
        let baseline = &launch.workspace.as_ref().unwrap().baseline_object_reference;
        let store = Arc::new(FakeCleanupStore::new([Ok(())], [Ok(())], [Ok(())]));
        let github = Arc::new(FakeGithub::new([]));
        let execution = cleanup_executor(Arc::clone(&github), Arc::clone(&store))
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute),
            )
            .await;
        let EffectExecution::Completed {
            result: LaunchEffectResult::Cleanup { receipt },
            external_resource_ids,
            ..
        } = execution
        else {
            panic!("the exact retained cleanup plan must complete");
        };
        assert_eq!(receipt.targets, targets);
        assert_eq!(external_resource_ids["cleanup_target_count"], "3");
        assert_eq!(
            *store.calls.lock().unwrap(),
            vec![
                format!("diff:{}:{}", launch.owner.0, diff.id.0),
                format!("checkpoint:{}:{}", launch.owner.0, checkpoint.id.0),
                format!("baseline:{}:{}:{baseline}", launch.owner.0, launch.id.0),
            ]
        );
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.publication_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.pull_request_write_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            github.pull_request_reconcile_calls.load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn cleanup_deletes_a_staging_only_import_without_github_access() {
        let launch = staging_cleanup_launch();
        let store = Arc::new(FakeCleanupStore::new([Ok(())], [], []));
        let github = Arc::new(FakeGithub::new([]));
        let execution = cleanup_executor(Arc::clone(&github), Arc::clone(&store))
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute),
            )
            .await;

        assert!(matches!(
            execution,
            EffectExecution::Completed {
                result: LaunchEffectResult::Cleanup { .. },
                ..
            }
        ));
        assert_eq!(
            *store.calls.lock().unwrap(),
            vec![format!(
                "baseline-import:{}:{}",
                launch.owner.0, launch.id.0
            )]
        );
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn staging_import_cleanup_failure_remains_retryable_cleanup_debt() {
        let launch = staging_cleanup_launch();
        let store = Arc::new(FakeCleanupStore::new(
            [
                Err(TriggerStoreError::Unavailable("delete timeout".into())),
                Ok(()),
            ],
            [],
            [],
        ));
        let executor = cleanup_executor(Arc::new(FakeGithub::new([])), Arc::clone(&store));

        assert!(matches!(
            executor
                .execute(
                    &launch,
                    &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute),
                )
                .await,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            executor
                .reconcile(
                    &launch,
                    &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Reconcile),
                )
                .await,
            EffectExecution::Completed {
                result: LaunchEffectResult::Cleanup { .. },
                ..
            }
        ));
        assert_eq!(store.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cleanup_reconciliation_repeats_the_same_keys_after_partial_failure() {
        let launch = cleanup_launch();
        let checkpoint = launch
            .workspace
            .as_ref()
            .unwrap()
            .latest_checkpoint
            .as_ref()
            .unwrap();
        let diff = launch.diff.as_ref().unwrap();
        let baseline = &launch.workspace.as_ref().unwrap().baseline_object_reference;
        let store = Arc::new(FakeCleanupStore::new(
            [Ok(())],
            [
                Err(TriggerStoreError::Unavailable("timeout".into())),
                Ok(()),
            ],
            [Ok(()), Ok(())],
        ));
        let executor = cleanup_executor(Arc::new(FakeGithub::new([])), Arc::clone(&store));
        let first = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute),
            )
            .await;
        assert!(matches!(
            first,
            EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    ..
                },
                ..
            }
        ));
        let second = executor
            .reconcile(
                &launch,
                &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Reconcile),
            )
            .await;
        assert!(matches!(
            second,
            EffectExecution::Completed {
                result: LaunchEffectResult::Cleanup { .. },
                ..
            }
        ));
        assert_eq!(
            *store.calls.lock().unwrap(),
            vec![
                format!("diff:{}:{}", launch.owner.0, diff.id.0),
                format!("checkpoint:{}:{}", launch.owner.0, checkpoint.id.0),
                format!("diff:{}:{}", launch.owner.0, diff.id.0),
                format!("checkpoint:{}:{}", launch.owner.0, checkpoint.id.0),
                format!("baseline:{}:{}:{baseline}", launch.owner.0, launch.id.0),
            ]
        );
    }

    #[tokio::test]
    async fn cleanup_deletes_an_uploaded_object_named_only_by_its_retained_intent() {
        let mut launch = cleanup_launch();
        let checkpoint = launch
            .workspace
            .as_mut()
            .unwrap()
            .latest_checkpoint
            .take()
            .unwrap();
        let workspace = launch.workspace.as_mut().unwrap();
        workspace.previous_checkpoint = None;
        workspace.retired_checkpoint = None;
        workspace.sealed_contribution_checkpoint = None;
        workspace.checkpoint_intent = Some(layer2_core::WorkspaceCheckpointIntent {
            id: checkpoint.id.clone(),
            version: checkpoint.version,
            purpose: layer2_core::WorkspaceCheckpointPurpose::Periodic,
            session_id: checkpoint.session_id,
            runtime_generation: checkpoint.runtime_generation,
            workspace_epoch: checkpoint.workspace_epoch,
            requested_at: checkpoint.created_at,
            retry_at: None,
        });
        let store = Arc::new(FakeCleanupStore::new([Ok(())], [Ok(())], [Ok(())]));
        let execution = cleanup_executor(Arc::new(FakeGithub::new([])), Arc::clone(&store))
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute),
            )
            .await;
        assert!(matches!(
            execution,
            EffectExecution::Completed {
                result: LaunchEffectResult::Cleanup { .. },
                ..
            }
        ));
        assert!(store.calls.lock().unwrap().iter().any(|call| {
            call == &format!(
                "checkpoint-intent:{}:{}:{}",
                launch.owner.0, launch.id.0, checkpoint.id.0
            )
        }));
    }

    #[tokio::test]
    async fn cleanup_rejects_a_plan_target_not_retained_by_the_launch_before_delete() {
        let mut launch = cleanup_launch();
        launch.cleanup.as_mut().unwrap().plan.targets[1] = CleanupTarget::Checkpoint {
            checkpoint_id: CheckpointId::new(),
        };
        let store = Arc::new(FakeCleanupStore::new([Ok(())], [Ok(())], [Ok(())]));
        let execution = cleanup_executor(Arc::new(FakeGithub::new([])), Arc::clone(&store))
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute),
            )
            .await;
        let EffectExecution::Failed { failure } = execution else {
            panic!("a widened cleanup plan must fail closed");
        };
        assert_eq!(failure.class, FailureClass::TerminalSecurity);
        assert_eq!(failure.code, "cleanup_plan_invalid");
        assert!(store.calls.lock().unwrap().is_empty());
    }

    fn assert_publication_authority_unchanged(
        expected: &layer2_core::ContributionRecord,
        actual: &layer2_core::ContributionRecord,
    ) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.launch_id, expected.launch_id);
        assert_eq!(actual.diff_artifact_id, expected.diff_artifact_id);
        assert_eq!(actual.revision_digest, expected.revision_digest);
        assert_eq!(actual.baseline_digest, expected.baseline_digest);
        assert_eq!(actual.checkpoint_digest, expected.checkpoint_digest);
        assert_eq!(actual.diff_digest, expected.diff_digest);
        assert_eq!(actual.branch_name, expected.branch_name);
        assert_eq!(actual.expected_head, expected.expected_head);
        assert_eq!(actual.author, expected.author);
        assert_eq!(actual.committer, expected.committer);
        assert_eq!(actual.confirmation, expected.confirmation);
        assert_eq!(actual.intent_key, expected.intent_key);
    }

    #[test]
    fn incomplete_runtime_admission_refuses_new_effects_but_preserves_cleanup() {
        let executor = GithubEffectExecutor::with_port(
            Arc::new(FakeGithub::new([])),
            DEFAULT_FORK_READINESS_TIMEOUT,
        );
        let claim = claim_for(LaunchEffectKind::Fork, EffectClaimMode::Execute);
        let cleanup = claim_for(LaunchEffectKind::Cleanup, EffectClaimMode::Execute);

        for corruption in [
            "missing",
            "historical-kernel",
            "missing-version",
            "malformed-time",
            "missing-capability",
            "duplicate-capability",
        ] {
            let mut launch = fork_launch();
            match corruption {
                "missing" => launch.runtime_admission = None,
                "historical-kernel" => {
                    launch.runtime_admission.as_mut().unwrap().kernel =
                        layer2_core::KernelId::new(super::super::LEGACY_TRIGGER_KERNEL_ID);
                }
                "missing-version" => {
                    launch.runtime_admission.as_mut().unwrap().runtime_version = String::new();
                }
                "malformed-time" => {
                    launch.runtime_admission.as_mut().unwrap().observed_at = "invalid".into();
                }
                "missing-capability" => {
                    launch
                        .runtime_admission
                        .as_mut()
                        .unwrap()
                        .supported_capabilities
                        .pop();
                }
                "duplicate-capability" => {
                    let runtime = launch.runtime_admission.as_mut().unwrap();
                    runtime
                        .supported_capabilities
                        .push(runtime.supported_capabilities[0]);
                }
                _ => unreachable!("the table contains only declared corruptions"),
            }

            let failure = executor
                .refuse_execute(&launch, &claim, corruption)
                .expect("an incomplete admission cannot execute a new provider effect");
            assert_eq!(
                failure.class,
                FailureClass::TerminalSecurity,
                "{corruption}"
            );
            assert_eq!(
                failure.code, "launch_runtime_route_quarantined",
                "{corruption}"
            );
            assert!(
                executor
                    .refuse_execute(&launch, &cleanup, corruption)
                    .is_none(),
                "cleanup remains available for {corruption}"
            );
        }

        let launch = fork_launch();
        assert!(executor
            .refuse_execute(&launch, &claim, "complete-runtime-admission")
            .is_none());
    }

    fn fork_launch() -> Launch {
        let mut launch = super::super::worker::tests::launch_fixture();
        launch.state = LaunchState::ForkPending;
        launch
    }

    fn reach_launch() -> Launch {
        let mut launch = super::super::worker::tests::launch_fixture();
        let fork_context = ForkContext::try_from(&launch).unwrap();
        launch.fork = Some(fork(&fork_context));
        launch.branch = Some(BranchRecord {
            name: format!("cma/{}", launch.id.0),
            expected_head: launch.source_sha.clone(),
            current_head: launch.source_sha.clone(),
            canonical_url: format!(
                "https://github.com/contributor/project/tree/cma/{}",
                launch.id.0
            ),
            created_at: "2026-08-27T00:00:00Z".into(),
        });
        launch.session = Some(SessionLink {
            session_id: SessionId("sesn_trigger_effect".into()),
            environment_profile_id: "envp_trigger_effect".into(),
            runtime_generation: 1,
            created_at: "2026-08-27T00:00:00Z".into(),
        });
        launch.state = LaunchState::PromptPending;
        launch
    }

    fn sponsored_reach_launch() -> Launch {
        let mut launch = reach_launch();
        // These executor tests isolate the post-Start data plane. The worker
        // fixture has already exercised review/Start admission separately.
        launch.revision.content.commercial_policy.billing_mode = BillingMode::Sponsored;
        launch
    }

    fn live_sponsored_capability(launch: &Launch) -> InertGrantCapability {
        sponsored_capability(
            launch,
            launch.session.as_ref().unwrap().session_id.clone(),
            rfc3339_at(SystemTime::now() + Duration::from_secs(300)),
        )
    }

    fn promote_reach_to_reconciliation(
        launch: &mut Launch,
        execute_claim: &EffectClaim,
        fence: u64,
    ) -> EffectClaim {
        let record = launch
            .effects
            .iter_mut()
            .find(|record| {
                record.kind == execute_claim.kind && record.intent_key == execute_claim.intent_key
            })
            .expect("the executable Reach retained its admitted effect record");
        record.state = layer2_core::EffectState::Reconciling;
        record.reconciliation_attempt = record.reconciliation_attempt.saturating_add(1);
        record.fence = fence;
        launch.fencing_counter = fence;
        launch.lease = Some(layer2_core::WorkerLease {
            holder: format!("github-effect-reconciler-{fence}"),
            fencing_token: fence,
            acquired_at: "2026-08-27T00:01:00Z".into(),
            expires_at: "2099-08-27T00:00:00Z".into(),
        });
        EffectClaim {
            kind: execute_claim.kind,
            mode: EffectClaimMode::Reconcile,
            intent_key: execute_claim.intent_key.clone(),
            attempt: execute_claim.attempt,
            reconciliation_attempt: record.reconciliation_attempt,
            fence,
        }
    }

    fn workspace_launch() -> Launch {
        let mut launch = reach_launch();
        launch.state = LaunchState::WorkspacePending;
        launch.workspace = None;
        launch.session = None;
        launch.reach = None;
        launch
    }

    fn workspace_source_plan(
        launch: &Launch,
        files: Vec<(&str, Vec<u8>)>,
    ) -> (WorkspaceSourcePlan, BTreeMap<String, Vec<u8>>) {
        let mut entries = Vec::with_capacity(files.len());
        let mut blobs = BTreeMap::new();
        let mut blob_bytes = BTreeMap::new();
        for (path, bytes) in files {
            let provider_object_id = workspace_git_blob_id(&bytes);
            entries.push(WorkspaceSourceEntry {
                path: path.into(),
                mode: 0o100644,
                provider_object_id: provider_object_id.clone(),
                bytes: bytes.len() as u64,
            });
            blobs.insert(
                provider_object_id.clone(),
                BaselineImportBlob::new(provider_object_id.clone(), bytes.len() as u64).unwrap(),
            );
            blob_bytes.insert(provider_object_id, bytes);
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let mut tree = Sha256::new();
        tree.update(b"cma-trigger-source-tree-v1\0");
        tree.update(launch.source_sha.as_bytes());
        for entry in &entries {
            tree.update((entry.path.len() as u64).to_be_bytes());
            tree.update(entry.path.as_bytes());
            tree.update(entry.mode.to_be_bytes());
            tree.update(entry.provider_object_id.as_bytes());
            tree.update(entry.bytes.to_be_bytes());
        }
        (
            WorkspaceSourcePlan {
                source_revision: launch.source_sha.clone(),
                tree_digest: format!("{:x}", tree.finalize()),
                entries,
                blobs: blobs.into_values().collect(),
            },
            blob_bytes,
        )
    }

    fn workspace_git_blob_id(bytes: &[u8]) -> String {
        super::super::workspace_diff::git_blob_id(bytes)
    }

    fn assert_workspace_integrity_failure(execution: EffectExecution) {
        assert!(matches!(
            execution,
            EffectExecution::Failed {
                failure: LaunchFailure {
                    class: FailureClass::TerminalSecurity,
                    ref code,
                    ..
                }
            } if code == "workspace_storage_integrity_failed"
        ));
    }

    fn initial_dispatch_launch() -> Launch {
        let mut launch = reach_launch();
        let session_id = launch.session.as_ref().unwrap().session_id.clone();
        launch
            .authority
            .as_mut()
            .unwrap()
            .session_credential_authority = Some(test_credential_authority(&session_id));
        launch.reach = Some(layer2_core::ReachStatus {
            service_id: launch.connection.as_ref().unwrap().service_id.clone(),
            state: CapabilityRevocationState::Active,
            policy_generation: 1,
            policy_fence: 1,
            runtime_audience: "sandbox-runtime-1".into(),
            expires_at_epoch_seconds: 4_000_000_000,
            numeric_binding_observed_at: "2026-08-27T00:00:00Z".into(),
            refreshed_at: "2026-08-27T00:00:00Z".into(),
            revocation_requested_at: None,
            revocation_effective_at: None,
            last_refusal: None,
            tombstone_acknowledged: false,
            agent_key_revocation_confirmed: false,
        });
        launch
    }

    fn checkpoint_launch() -> Launch {
        let mut launch = reach_launch();
        let session = launch.session.as_ref().unwrap();
        let intent = layer2_core::WorkspaceCheckpointIntent {
            id: CheckpointId::new(),
            version: 1,
            purpose: layer2_core::WorkspaceCheckpointPurpose::ContributionReview,
            session_id: session.session_id.clone(),
            runtime_generation: session.runtime_generation,
            workspace_epoch: 1,
            requested_at: "2026-08-27T00:00:00Z".into(),
            retry_at: None,
        };
        launch.workspace = Some(WorkspaceRecord {
            baseline: WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap(),
            baseline_object_reference: "trigger-test/baseline.pack".into(),
            baseline_archive_sha256: "a".repeat(64),
            latest_checkpoint: None,
            previous_checkpoint: None,
            retired_checkpoint: None,
            sealed_contribution_checkpoint: None,
            checkpoint_intent: Some(intent),
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 0,
            workspace_epoch: 1,
            retention_policy_id: "trigger-test-retention".into(),
            delete_after: "2026-09-27T00:00:00Z".into(),
        });
        launch.contribution_checkpoint_version = Some(1);
        launch.state = LaunchState::Contributing;
        launch
    }

    fn diff_launch() -> Launch {
        let mut launch = checkpoint_launch();
        let workspace = launch.workspace.as_mut().unwrap();
        let checkpoint = WorkspaceCheckpoint {
            id: CheckpointId::new(),
            launch_id: launch.id.clone(),
            version: 1,
            workspace_epoch: workspace.workspace_epoch,
            session_id: launch.session.as_ref().unwrap().session_id.clone(),
            runtime_generation: 1,
            object_reference: "trigger-test/checkpoint-v1.pack".into(),
            archive_sha256: "b".repeat(64),
            manifest: WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap(),
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some(workspace.delete_after.clone()),
        };
        workspace.latest_checkpoint = Some(checkpoint.clone());
        workspace.sealed_contribution_checkpoint = Some(checkpoint);
        workspace.checkpoint_intent = None;
        workspace.checkpoint_version = 1;
        launch.contribution_checkpoint_version = Some(1);
        launch
    }

    struct PublicationLaunchFixture {
        launch: Launch,
        artifact_object: Vec<u8>,
        checkpoint_archive: Vec<u8>,
    }

    fn publication_launch() -> PublicationLaunchFixture {
        let mut launch = checkpoint_launch();
        let source = launch.source_sha.clone();
        let baseline = WorkspaceManifest::seal(source.clone(), Vec::new()).unwrap();
        let baseline_archive = test_archive(&baseline, &[], 0, 0);
        let content = b"published review\n".to_vec();
        let content_sha256 = format!("{:x}", Sha256::digest(&content));
        let blob_id = super::super::workspace_diff::git_blob_id(&content);
        let entry = WorkspaceEntry {
            path: "README.md".into(),
            kind: WorkspaceEntryKind::Regular,
            mode: 0o100644,
            size: content.len() as u64,
            content_sha256,
            provider_object_id: Some(blob_id),
            symlink_target: None,
        };
        let checkpoint_manifest =
            WorkspaceManifest::seal(source.clone(), vec![entry.clone()]).unwrap();
        let checkpoint_archive = test_archive(&checkpoint_manifest, &[(entry, content)], 1, 1);
        let checkpoint = WorkspaceCheckpoint {
            id: CheckpointId::new(),
            launch_id: launch.id.clone(),
            version: 1,
            workspace_epoch: 1,
            session_id: launch.session.as_ref().unwrap().session_id.clone(),
            runtime_generation: 1,
            object_reference: "trigger-test/publication-checkpoint.pack".into(),
            archive_sha256: format!("{:x}", Sha256::digest(&checkpoint_archive)),
            manifest: checkpoint_manifest,
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some("2026-09-27T00:00:00Z".into()),
        };
        launch.workspace = Some(WorkspaceRecord {
            baseline: baseline.clone(),
            baseline_object_reference: "trigger-test/publication-baseline.pack".into(),
            baseline_archive_sha256: format!("{:x}", Sha256::digest(&baseline_archive)),
            latest_checkpoint: Some(checkpoint.clone()),
            previous_checkpoint: None,
            retired_checkpoint: None,
            sealed_contribution_checkpoint: Some(checkpoint.clone()),
            checkpoint_intent: None,
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 1,
            workspace_epoch: 1,
            retention_policy_id: "trigger-test-retention".into(),
            delete_after: "2026-09-27T00:00:00Z".into(),
        });
        launch.contribution_checkpoint_version = Some(1);
        launch.reach = Some(layer2_core::ReachStatus {
            service_id: launch.connection.as_ref().unwrap().service_id.clone(),
            state: CapabilityRevocationState::Active,
            policy_generation: 1,
            policy_fence: 1,
            runtime_audience: "runtime-publication".into(),
            expires_at_epoch_seconds: 4_000_000_000,
            numeric_binding_observed_at: "2026-08-27T00:00:00Z".into(),
            refreshed_at: "2026-08-27T00:00:00Z".into(),
            revocation_requested_at: None,
            revocation_effective_at: None,
            last_refusal: None,
            tombstone_acknowledged: false,
            agent_key_revocation_confirmed: false,
        });
        launch.delivery = Some(prompt_delivery(
            &launch,
            layer2_core::PromptDeliveryState::Completed,
        ));
        let mut built = build_diff_artifact(
            &launch,
            &checkpoint,
            &baseline_archive,
            &checkpoint_archive,
            &launch.branch.as_ref().unwrap().current_head,
            TEST_CHANGED_FILE_LIMIT,
            DiffArtifactId::new(),
            "2026-08-27T00:00:01Z".into(),
        )
        .unwrap();
        built.artifact.object_reference = "trigger-test/publication-diff.json".into();
        launch.diff = Some(built.artifact.clone());
        launch
            .confirm_branch_publication(
                Confirmation {
                    action: ConfirmationAction::PublishBranch,
                    subject: launch.owner.clone(),
                    snapshot_digest: built.artifact.digest.clone(),
                    confirmed_at: "2026-08-27T00:00:02Z".into(),
                    idempotency_key_hash: "publication-confirmation".into(),
                },
                "2026-08-27T00:00:02Z",
                "confirm-publication",
            )
            .unwrap();
        PublicationLaunchFixture {
            launch,
            artifact_object: built.object,
            checkpoint_archive,
        }
    }

    pub(in crate::trigger) fn publication_launch_for_worker_test() -> Launch {
        publication_launch().launch
    }

    fn cleanup_launch() -> Launch {
        let mut launch = publication_launch().launch;
        launch.state = LaunchState::Cancelled;
        launch.terminal_at = Some("2026-07-01T00:00:00Z".into());
        launch.updated_at = "2026-08-01T00:00:00Z".into();
        let reach = launch.reach.as_mut().unwrap();
        reach.state = CapabilityRevocationState::Revoked;
        reach.tombstone_acknowledged = true;
        reach.revocation_effective_at = Some("2026-08-01T00:00:00Z".into());
        let workspace = launch.workspace.as_mut().unwrap();
        workspace.delete_after = "2026-08-01T00:00:00Z".into();
        let targets = vec![
            CleanupTarget::DiffArtifact {
                artifact_id: launch.diff.as_ref().unwrap().id.clone(),
            },
            CleanupTarget::Checkpoint {
                checkpoint_id: workspace.latest_checkpoint.as_ref().unwrap().id.clone(),
            },
            CleanupTarget::Baseline,
        ];
        launch.cleanup = Some(layer2_core::CleanupRecord {
            plan: layer2_core::CleanupPlan {
                retention_policy_id: workspace.retention_policy_id.clone(),
                delete_after: workspace.delete_after.clone(),
                targets,
                planned_at: "2026-08-01T00:00:00Z".into(),
            },
            completed_at: None,
        });
        launch
    }

    fn staging_cleanup_launch() -> Launch {
        let mut launch = cleanup_launch();
        launch.workspace = None;
        launch.diff = None;
        launch.reach = None;
        if !launch
            .effects
            .iter()
            .any(|effect| effect.kind == LaunchEffectKind::WorkspaceImport)
        {
            launch.effects.push(layer2_core::EffectRecord {
                kind: LaunchEffectKind::WorkspaceImport,
                state: layer2_core::EffectState::FailedTerminal,
                intent_key: format!("cma-trigger:{}:workspace-import", launch.id.0),
                attempt: 1,
                reconciliation_attempt: 0,
                fence: 1,
                authority_admission: None,
                provider_request_id: None,
                external_resource_ids: BTreeMap::new(),
                next_attempt_at: None,
                last_failure: None,
                updated_at: "2026-08-01T00:00:00Z".into(),
            });
        }
        launch.cleanup = Some(layer2_core::CleanupRecord {
            plan: layer2_core::CleanupPlan {
                retention_policy_id: STAGING_IMPORT_RETENTION_POLICY_ID.into(),
                delete_after: launch.terminal_at.clone().unwrap(),
                targets: vec![CleanupTarget::Baseline],
                planned_at: "2026-08-01T00:00:00Z".into(),
            },
            completed_at: None,
        });
        launch
    }

    fn pull_request_launch() -> Launch {
        let mut launch = publication_launch().launch;
        let contribution = launch.contribution.as_mut().unwrap();
        contribution.state = PublicationState::Published;
        contribution.base_tree_id = Some("b".repeat(40));
        contribution.tree_id = Some("c".repeat(40));
        contribution.commit_id = Some("d".repeat(40));
        contribution.resulting_head = Some("d".repeat(40));
        contribution.object_ids = vec!["f".repeat(40)];
        contribution.updated_at = "2026-08-27T00:00:03Z".into();
        launch.branch.as_mut().unwrap().current_head = "d".repeat(40);
        let draft = launch
            .pull_request_draft(PullRequestOperation::Open, None, None)
            .unwrap();
        launch
            .confirm_pull_request(
                Confirmation {
                    action: ConfirmationAction::OpenPullRequest,
                    subject: launch.owner.clone(),
                    snapshot_digest: draft.intent_digest,
                    confirmed_at: "2026-08-27T00:00:04Z".into(),
                    idempotency_key_hash: "open-pr-confirmation".into(),
                },
                None,
                None,
                "2026-08-27T00:00:04Z",
                "confirm-pr",
            )
            .unwrap();
        launch
    }

    pub(super) fn reconciled_pull_request(
        launch: &Launch,
        state: PullRequestState,
    ) -> layer2_core::PullRequestRecord {
        let mut record = launch.pull_request.clone().unwrap();
        let upstream = &launch.revision.content.upstream.repository;
        record.provider_id = Some(70);
        record.provider_node_id = Some("PR_node_7".into());
        record.number = Some(7);
        record.canonical_url = Some(format!(
            "https://github.com/{}/{}/pull/7",
            upstream.canonical_owner, upstream.canonical_name
        ));
        record.state = state;
        record.base_revision = Some(launch.source_sha.clone());
        record.head_revision = Some("d".repeat(40));
        record.observed_title = Some(record.intent.title.clone());
        record.observed_body_digest = Some(record.intent.body_digest.clone());
        record.mergeability = layer2_core::Mergeability::Mergeable;
        record.checks = layer2_core::CheckSummaryState::Passing;
        record.updated_at = "2026-08-27T00:00:05Z".into();
        record.provider_created_at = Some("2026-08-27T00:00:04Z".into());
        record.provider_updated_at = Some("2026-08-27T00:00:05Z".into());
        record.validate_intent(&launch.id).unwrap();
        record
    }

    pub(super) fn test_archive(
        manifest: &WorkspaceManifest,
        entries: &[(WorkspaceEntry, Vec<u8>)],
        checkpoint_version: u64,
        quiesced_writers: u32,
    ) -> Vec<u8> {
        let mut offset = 0_u64;
        let header_entries = entries
            .iter()
            .map(|(entry, _)| {
                let value = serde_json::json!({
                    "path": entry.path,
                    "kind": entry.kind,
                    "mode": entry.mode,
                    "size": entry.size,
                    "content_sha256": entry.content_sha256,
                    "provider_object_id": entry.provider_object_id,
                    "offset": offset,
                });
                offset += entry.size;
                value
            })
            .collect::<Vec<_>>();
        let header = serde_json::to_vec(&serde_json::json!({
            "cma_workspace": 1,
            "workspace_epoch": 1,
            "checkpoint_version": checkpoint_version,
            "quiesced_writers": quiesced_writers,
            "source_revision": manifest.source_revision,
            "manifest_digest": manifest.digest,
            "entries": header_entries,
        }))
        .unwrap();
        let mut archive = Vec::new();
        archive.extend_from_slice(&(header.len() as u64).to_be_bytes());
        archive.extend_from_slice(&header);
        for (_, bytes) in entries {
            archive.extend_from_slice(bytes);
        }
        archive
    }

    fn checkpoint_package(launch: &Launch) -> layer3_adapter::WorkspaceCheckpointPackage {
        let bytes = b"edited\n";
        let content_sha256 = format!("{:x}", Sha256::digest(bytes));
        let entry = WorkspaceEntry {
            path: "README.md".into(),
            kind: WorkspaceEntryKind::Regular,
            mode: 0o100644,
            size: bytes.len() as u64,
            content_sha256: content_sha256.clone(),
            provider_object_id: Some("b".repeat(40)),
            symlink_target: None,
        };
        let manifest = WorkspaceManifest::seal(launch.source_sha.clone(), vec![entry]).unwrap();
        let archive = b"sealed checkpoint archive".to_vec();
        layer3_adapter::WorkspaceCheckpointPackage {
            epoch: launch.workspace.as_ref().unwrap().workspace_epoch,
            version: 1,
            source_revision: launch.source_sha.clone(),
            manifest_digest: manifest.digest,
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            entries: vec![layer3_adapter::WorkspaceEntryPackage {
                path: "README.md".into(),
                kind: layer3_adapter::WorkspaceEntryKind::Regular,
                mode: 0o100644,
                size: bytes.len() as u64,
                content_sha256,
                provider_object_id: Some("b".repeat(40)),
                symlink_target: None,
            }],
            total_bytes: bytes.len() as u64,
            quiesced_writers: 1,
            writer_state: layer3_adapter::WorkspaceWriterState::Resumed,
            writer_resume_token: None,
            archive,
        }
    }

    pub(super) fn test_signer() -> Arc<CapabilitySigner> {
        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Arc::new(
            CapabilitySigner::from_config(&crate::config::GithubCapabilitySigningConfig {
                key_id: "test-key".into(),
                private_key_pkcs8_base64: base64::engine::general_purpose::STANDARD
                    .encode(key.as_ref()),
                previous_verification_keys_json: "{}".into(),
            })
            .unwrap(),
        )
    }

    pub(super) fn prompt_delivery(
        launch: &Launch,
        state: layer2_core::PromptDeliveryState,
    ) -> layer2_core::PromptDelivery {
        layer2_core::PromptDelivery {
            launch_id: launch.id.clone(),
            session_id: launch.session.as_ref().unwrap().session_id.clone(),
            revision_digest: launch.revision_digest.clone(),
            origin_key: format!(
                "cma-trigger:{}:{}:initial",
                launch.id.0, launch.revision_digest
            ),
            payload_hash: "a".repeat(64),
            state,
            attempt: u32::from(state != layer2_core::PromptDeliveryState::Queued),
            recovery_generation: 0,
            dispatch_fence_sequence: (state != layer2_core::PromptDeliveryState::Queued)
                .then_some(42),
            queued_at: "2026-08-27T00:00:00Z".into(),
            updated_at: "2026-08-27T00:00:01Z".into(),
        }
    }

    fn claim(mode: EffectClaimMode) -> EffectClaim {
        claim_for(LaunchEffectKind::Fork, mode)
    }

    fn claim_for(kind: LaunchEffectKind, mode: EffectClaimMode) -> EffectClaim {
        EffectClaim {
            kind,
            mode,
            intent_key: "cma-trigger:test:effect:1".into(),
            attempt: 1,
            reconciliation_attempt: u32::from(mode == EffectClaimMode::Reconcile),
            fence: 1,
        }
    }

    async fn execute_with_admission(
        executor: &GithubEffectExecutor,
        launch: &mut Launch,
        claim: &EffectClaim,
    ) -> EffectExecution {
        install_test_admission(executor, launch, claim).await;
        executor.execute(launch, claim).await
    }

    async fn install_test_admission(
        executor: &GithubEffectExecutor,
        launch: &mut Launch,
        claim: &EffectClaim,
    ) {
        assert_eq!(claim.mode, EffectClaimMode::Execute);
        let now = "2026-08-27T00:00:10Z";
        let lease = layer2_core::WorkerLease {
            holder: "github-effect-unit-worker".into(),
            fencing_token: claim.fence,
            acquired_at: "2026-08-27T00:00:00Z".into(),
            expires_at: "2099-08-27T00:00:00Z".into(),
        };
        launch.fencing_counter = claim.fence;
        launch.lease = Some(lease.clone());
        if let Some(record) = launch
            .effects
            .iter_mut()
            .find(|record| record.kind == claim.kind && record.intent_key == claim.intent_key)
        {
            record.state = layer2_core::EffectState::Claimed;
            record.attempt = claim.attempt;
            record.reconciliation_attempt = claim.reconciliation_attempt;
            record.fence = claim.fence;
            record.authority_admission = None;
            record.provider_request_id = None;
            record.next_attempt_at = None;
            record.last_failure = None;
            record.updated_at = now.into();
        } else {
            launch.effects.push(layer2_core::EffectRecord {
                kind: claim.kind,
                state: layer2_core::EffectState::Claimed,
                intent_key: claim.intent_key.clone(),
                attempt: claim.attempt,
                reconciliation_attempt: claim.reconciliation_attempt,
                fence: claim.fence,
                authority_admission: None,
                provider_request_id: None,
                external_resource_ids: BTreeMap::new(),
                next_attempt_at: None,
                last_failure: None,
                updated_at: now.into(),
            });
        }
        let admission = match executor.admit(launch, claim).await {
            Ok(Some(admission)) => admission,
            other => panic!("the test effect must pass fresh authority preflight: {other:?}"),
        };
        launch
            .record_effect_authority_admission(&lease, claim, admission, now)
            .expect("the admission CAS must bind the exact test claim");
    }

    fn session_executor(sessions: Arc<FakeSessions>) -> GithubEffectExecutor {
        GithubEffectExecutor::with_session_port(
            Arc::new(FakeGithub::new([])),
            sessions as Arc<dyn TriggerSessionEffectPort>,
        )
    }

    fn sponsored_executor(
        sessions: Arc<FakeSessions>,
        sponsorships: Arc<FakeSponsorships>,
    ) -> GithubEffectExecutor {
        let mut executor = GithubEffectExecutor::with_capability_ports(
            Arc::new(FakeGithub::new([])),
            sessions as Arc<dyn TriggerSessionEffectPort>,
            test_signer(),
        );
        executor.sponsorship = Some(SponsoredEffectRuntime {
            sponsorships,
            gateway_origin: layer3_adapter::SponsoredModelGatewayOrigin::parse(
                "https://sponsored-model-gateway.example",
            )
            .unwrap(),
        });
        executor
    }

    fn diff_executor(github: Arc<FakeGithub>, store: Arc<FakeReviewStore>) -> GithubEffectExecutor {
        GithubEffectExecutor {
            github,
            workspace_store: Some(store.clone() as Arc<dyn BaselineObjectStore>),
            checkpoint_store: Some(store.clone() as Arc<dyn CheckpointObjectStore>),
            diff_store: Some(store as Arc<dyn DiffArtifactObjectStore>),
            sessions: None,
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    fn workspace_executor(
        github: Arc<FakeGithub>,
        store: Arc<FakeImportStore>,
    ) -> GithubEffectExecutor {
        GithubEffectExecutor {
            github,
            workspace_store: Some(store as Arc<dyn BaselineObjectStore>),
            checkpoint_store: None,
            diff_store: None,
            sessions: None,
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    fn publication_executor(
        github: Arc<FakeGithub>,
        store: Arc<FakeReviewStore>,
    ) -> GithubEffectExecutor {
        GithubEffectExecutor {
            github,
            workspace_store: None,
            checkpoint_store: Some(store.clone() as Arc<dyn CheckpointObjectStore>),
            diff_store: Some(store as Arc<dyn DiffArtifactObjectStore>),
            sessions: None,
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    fn cleanup_executor(
        github: Arc<FakeGithub>,
        store: Arc<FakeCleanupStore>,
    ) -> GithubEffectExecutor {
        GithubEffectExecutor {
            github,
            workspace_store: Some(store.clone() as Arc<dyn BaselineObjectStore>),
            checkpoint_store: Some(store.clone() as Arc<dyn CheckpointObjectStore>),
            diff_store: Some(store as Arc<dyn DiffArtifactObjectStore>),
            sessions: None,
            sponsorship: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: TEST_CHANGED_FILE_LIMIT,
        }
    }

    pub(super) fn trigger_session(
        launch: &Launch,
        status: SessionStatus,
        rotations: u32,
    ) -> Session {
        Session {
            options: Default::default(),
            id: layer2_core::SessionId("sesn_trigger_effect".into()),
            owner: launch.owner.clone(),
            kernel: layer2_core::KernelId::new(super::super::TRIGGER_KERNEL_ID),
            status,
            recovery_state: layer2_core::RecoveryState::Normal,
            stop_reason: None,
            geometry: Default::default(),
            last_seq: 0,
            environment_profile: Some(layer2_core::EnvironmentProfileId(
                "envp_trigger_effect".into(),
            )),
            runtime_version: Some("trigger-runtime-v1".into()),
            rotations,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: Some(Vec::new()),
            granted_skills: Some(Vec::new()),
            pricing: None,
            created_at: "2026-08-27T00:00:00Z".into(),
        }
    }

    pub(super) fn fork(context: &ForkContext) -> ForkRecord {
        ForkRecord {
            repository: RepositoryIdentity {
                repository_id: 30,
                repository_node_id: "R_fork".into(),
                owner_id: context.connection.account.account_id,
                owner_node_id: context.connection.account.account_node_id.clone(),
                canonical_owner: context.connection.account.login.clone(),
                canonical_name: context.upstream.canonical_name.clone(),
                visibility: RepositoryVisibility::Public,
                allow_forking: true,
                default_branch: "main".into(),
                selected_ref: context.upstream.selected_ref.clone(),
                fork_parent_id: Some(context.upstream.repository_id),
                fork_source_id: Some(
                    context
                        .upstream
                        .fork_source_id
                        .unwrap_or(context.upstream.repository_id),
                ),
            },
            reused: false,
            source_revision: context.source_sha.clone(),
            canonical_url: "https://github.com/contributor/project".into(),
            ready_at: "2026-08-27T00:00:00Z".into(),
        }
    }
}

#[cfg(test)]
#[path = "release_verification_tests.rs"]
mod release_verification_tests;
