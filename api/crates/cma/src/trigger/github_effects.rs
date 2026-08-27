//! Durable Trigger effects backed by the typed NyxID/GitHub broker.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, seconds_from_rfc3339, AuthorityProvenance, BaselineObjectStore, BranchRecord,
    CapabilityOperation, CapabilityRevocationState, CheckpointId, CheckpointIntegrity,
    CheckpointObjectStore, ConnectionSnapshot, CoreError, DelegationState, DiffArtifactObjectStore,
    DiffValidationOutcome, EffectClaim, EffectClaimMode, FailureClass, Launch, LaunchAuthority,
    LaunchCapability, LaunchEffectKind, LaunchEffectResult, LaunchFailure, RepositoryIdentity,
    Session, SessionId, SessionLink, SessionService, SessionStatus, TriggerAuthorityError,
    TriggerAuthorityVerifier, TriggerServiceError, TriggerStoreError, WorkspaceCheckpoint,
    WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest, WorkspaceRecord,
    CAPABILITY_MAX_LIFETIME_SECONDS, CAPABILITY_POLICY_VERSION,
};
use sha2::{Digest as _, Sha256};

use super::capability_signing::{
    CapabilitySigner, RuntimePolicyTombstone, POLICY_AUDIENCE, POLICY_VERSION,
};
use super::github::{
    BranchContext, BranchReconciliation, ForkContext, ForkReconciliation, GithubBroker,
    GithubBrokerError, ImportedWorkspace, WorkspaceContext, WorkspaceUnsupported,
};
use super::worker::{EffectExecution, EffectExecutor};
use super::workspace_diff::{
    build_diff_artifact, build_invalid_diff_artifact, deterministic_diff_id, DiffBuildError,
};

const DEFAULT_FORK_READINESS_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PROVIDER_RETRY: Duration = Duration::from_secs(300);
const MAX_RETRY_AFTER: u64 = 3_600;
/// Conservative implementation default until #666's pinned-provider exercise
/// establishes the deployment-owned publication ceiling.
const PROVISIONAL_CHANGED_FILE_LIMIT: u32 = 1_000;

#[derive(Debug, Clone)]
struct CapabilityAuthorityEvidence {
    connection: ConnectionSnapshot,
    authority: AuthorityProvenance,
    upstream: RepositoryIdentity,
    fork: RepositoryIdentity,
    branch: BranchRecord,
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
    ) -> Result<ImportedWorkspace, GithubBrokerError>;

    async fn preflight_capability(
        &self,
        launch: &Launch,
    ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError>;

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
}

#[derive(Debug, Clone)]
struct TriggerSessionEvidence {
    session: Session,
    credential_ready: bool,
}

#[async_trait]
trait TriggerSessionEffectPort: Send + Sync {
    async fn ensure_session(&self, launch: &Launch) -> Result<TriggerSessionEvidence, CoreError>;

    async fn install_runtime_policy(
        &self,
        session_id: &SessionId,
        capability: LaunchCapability,
        signer: Arc<CapabilitySigner>,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError>;

    async fn runtime_policy_audience(&self, session_id: &SessionId) -> Result<String, CoreError>;

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

struct SessionServiceEffectPort(Arc<SessionService>);

#[async_trait]
impl TriggerSessionEffectPort for SessionServiceEffectPort {
    async fn ensure_session(&self, launch: &Launch) -> Result<TriggerSessionEvidence, CoreError> {
        let session = self.0.ensure_trigger_session(launch).await?;
        let credential_ready = self.0.trigger_credential_ready(&session.id).await;
        Ok(TriggerSessionEvidence {
            session,
            credential_ready,
        })
    }

    async fn install_runtime_policy(
        &self,
        session_id: &SessionId,
        mut capability: LaunchCapability,
        signer: Arc<CapabilitySigner>,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError> {
        self.0
            .install_runtime_policy(session_id, move |runtime_audience| {
                capability.runtime_audience = runtime_audience.to_string();
                signer
                    .sign_policy(&capability)
                    .map_err(CoreError::InvalidRequest)
            })
            .await
    }

    async fn runtime_policy_audience(&self, session_id: &SessionId) -> Result<String, CoreError> {
        self.0.runtime_policy_audience(session_id).await
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
    ) -> Result<ImportedWorkspace, GithubBrokerError> {
        GithubBroker::import_workspace(self, context).await
    }

    async fn preflight_capability(
        &self,
        launch: &Launch,
    ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError> {
        let expected_connection = launch.connection.as_ref().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no selected connection".into())
        })?;
        let expected_upstream = &launch.revision.content.upstream.repository;
        let mut upstream = self
            .repository_by_id(
                &launch.owner,
                &expected_connection.service_id,
                expected_upstream.repository_id,
            )
            .await?;
        upstream.identity.selected_ref = expected_upstream.selected_ref.clone();
        if upstream.identity != *expected_upstream || upstream.archived || upstream.disabled {
            return Err(GithubBrokerError::AuthorityDrift);
        }

        let expected_fork = &launch
            .fork
            .as_ref()
            .ok_or_else(|| GithubBrokerError::Malformed("the Launch has no fork".into()))?
            .repository;
        let mut fork = self
            .repository_by_id(
                &launch.owner,
                &expected_connection.service_id,
                expected_fork.repository_id,
            )
            .await?;
        fork.identity.selected_ref = expected_fork.selected_ref.clone();
        if fork.identity != *expected_fork
            || fork.archived
            || fork.disabled
            || fork.identity.owner_id != expected_connection.account.account_id
            || fork.identity.owner_node_id != expected_connection.account.account_node_id
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }

        let branch_context = BranchContext::try_from(launch)?;
        let mut observed_branch = self.observe_branch(&branch_context).await?;
        let expected_branch = launch.branch.as_ref().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no fenced branch".into())
        })?;
        if observed_branch.name != expected_branch.name
            || observed_branch.expected_head != expected_branch.expected_head
            || observed_branch.canonical_url != expected_branch.canonical_url
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        observed_branch.created_at = expected_branch.created_at.clone();

        // Last provider read before policy issue: this proves the exact
        // UserService, account, backing key and scope still match the Launch.
        let current = self
            .preflight_connection(&launch.owner, &expected_connection.service_id)
            .await?;
        if !same_connection_authority(expected_connection, &current.snapshot) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let observed_at = rfc3339_at(SystemTime::now());
        Ok(CapabilityAuthorityEvidence {
            connection: current.snapshot.clone(),
            authority: AuthorityProvenance {
                subject: launch.owner.clone(),
                subject_token_generation: current.subject_token_generation,
                service_id: current.snapshot.service_id.clone(),
                service_state_version: current.snapshot.service_state_version,
                backing_key_id: current.snapshot.backing_key_id.clone(),
                session_credential_generation: launch
                    .authority
                    .as_ref()
                    .and_then(|authority| authority.session_credential_generation),
                state: DelegationState::Available,
                observed_at: observed_at.clone(),
            },
            upstream: upstream.identity,
            fork: fork.identity,
            branch: observed_branch,
        })
    }

    async fn publish_branch(
        &self,
        context: &super::github::PublicationContext,
        plan: &super::workspace_diff::PublicationPlan,
    ) -> Result<super::github::PublicationAdvance, GithubBrokerError> {
        GithubBroker::publish_branch(self, context, plan).await
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
        && expected.service_state_version == current.service_state_version
        && expected.backing_key_id == current.backing_key_id
        && expected.account == current.account
        && expected.granted_scopes == current.granted_scopes
        && expected.satisfied_scope_classes == current.satisfied_scope_classes
        && current.state == layer2_core::ConnectionState::Active
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
    capability_signer: Option<Arc<CapabilitySigner>>,
    fork_readiness_timeout: Duration,
    changed_file_limit: u32,
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
}

impl GithubEffectExecutor {
    pub(crate) fn new(github: Arc<GithubBroker>) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: None,
            diff_store: None,
            sessions: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
        }
    }

    pub(crate) fn with_runtime(
        github: Arc<GithubBroker>,
        workspace_store: Arc<dyn BaselineObjectStore>,
        checkpoint_store: Arc<dyn CheckpointObjectStore>,
        diff_store: Arc<dyn DiffArtifactObjectStore>,
        sessions: Arc<SessionService>,
        capability_signer: Arc<CapabilitySigner>,
    ) -> Self {
        Self {
            github,
            workspace_store: Some(workspace_store),
            checkpoint_store: Some(checkpoint_store),
            diff_store: Some(diff_store),
            sessions: Some(Arc::new(SessionServiceEffectPort(sessions))),
            capability_signer: Some(capability_signer),
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
        }
    }

    #[cfg(test)]
    fn with_port(github: Arc<dyn GithubEffectPort>, fork_readiness_timeout: Duration) -> Self {
        Self {
            github,
            workspace_store: None,
            checkpoint_store: None,
            diff_store: None,
            sessions: None,
            capability_signer: None,
            fork_readiness_timeout,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
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
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
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
            capability_signer: Some(capability_signer),
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
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
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
        }
    }

    pub(crate) async fn capability_runtime_audience(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<String, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        let session = launch.session.as_ref().ok_or_else(|| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_reach_session_missing",
                "The Launch has no deterministic session for delegated access.",
                "Contact support; CMA will not infer a runtime audience.",
                &claim,
                None,
            )
        })?;
        let sessions = self
            .sessions
            .as_ref()
            .ok_or_else(|| execution_failure(unsupported_effect(&claim)))?;
        sessions
            .runtime_policy_audience(&session.session_id)
            .await
            .map_err(|error| execution_failure(session_failure(error, &claim)))
    }

    pub(crate) async fn renew_capability(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRenewal, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        match self.run_reach(launch, &claim).await {
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

    pub(crate) async fn revoke_capability(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRevocation, LaunchFailure> {
        let claim = capability_claim(launch, fence);
        let session = launch.session.as_ref().ok_or_else(|| {
            failure(
                FailureClass::TerminalSecurity,
                "trigger_reach_session_missing",
                "The Launch has no deterministic session whose reach can be revoked.",
                "Contact support; CMA cannot report revocation without positive enforcement.",
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
        let signer = self
            .capability_signer
            .as_ref()
            .ok_or_else(|| execution_failure(unsupported_effect(&claim)))?;
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

        let mut first_failure = None;
        let mut runtime_audience = reach.runtime_audience.clone();
        let mut tombstone_acknowledged = false;
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

        if let Err(error) = sessions
            .retire_runtime_credential(&session.session_id)
            .await
        {
            first_failure.get_or_insert_with(|| execution_failure(session_failure(error, &claim)));
        }
        let agent_key_revocation_confirmed = match sessions
            .runtime_credential_retirement_settled(&session.session_id)
            .await
        {
            Ok(settled) => settled,
            Err(error) => {
                first_failure
                    .get_or_insert_with(|| execution_failure(session_failure(error, &claim)));
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
        })
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
                    result: LaunchEffectResult::Fork { fork },
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
        let delay = reconciliation_delay(claim.reconciliation_attempt).min(remaining.max(1));
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
        let imported = match self.github.import_workspace(&context).await {
            Ok(imported) => imported,
            Err(error) => return broker_failure(error, claim, false),
        };
        let reference = match store
            .put_baseline(
                &launch.owner,
                &launch.id,
                &imported.manifest,
                &imported.archive_sha256,
                &imported.archive,
            )
            .await
        {
            Ok(reference) => reference,
            Err(TriggerStoreError::Unavailable(_)) => {
                return transient_failure(
                    claim,
                    false,
                    "workspace_storage_temporarily_unavailable",
                    "The repository baseline could not be confirmed in durable storage.",
                    None,
                )
            }
            Err(_) => {
                return EffectExecution::Failed {
                    failure: failure(
                        FailureClass::TerminalSecurity,
                        "workspace_storage_integrity_failed",
                        "The durable repository baseline failed its integrity contract.",
                        "Contact support with the Launch correlation ID; no partial baseline was accepted.",
                        claim,
                        None,
                    ),
                }
            }
        };
        let now = SystemTime::now();
        let mut resources = retained_resources(launch, claim);
        resources.insert("workspace_object_reference".into(), reference.clone());
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
                    baseline_object_reference: reference,
                    baseline_archive_sha256: imported.archive_sha256,
                    latest_checkpoint: None,
                    previous_checkpoint: None,
                    workspace_epoch: 1,
                    retention_policy_id: "trigger-workspace-v1".into(),
                    delete_after: rfc3339_at(now + Duration::from_secs(30 * 24 * 60 * 60)),
                },
            },
            safe_detail: "The exact repository baseline is stored and verified.".into(),
            external_resource_ids: resources,
        }
    }

    async fn run_sandbox(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let evidence = match self.ensure_trigger_session(launch, claim).await {
            Ok(evidence) => evidence,
            Err(execution) => return execution,
        };
        let resources = session_resources(launch, claim, &evidence.session);
        match evidence.session.status {
            SessionStatus::Ready | SessionStatus::Running if evidence.credential_ready => {
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
            | SessionStatus::Running => session_reconcile_later(
                claim,
                "trigger_runtime_pending",
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
        if !matches!(
            evidence.session.status,
            SessionStatus::Ready | SessionStatus::Running
        ) || !evidence.credential_ready
        {
            return session_reconcile_later(
                claim,
                "trigger_session_attachment_pending",
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
                    runtime_generation: u64::from(evidence.session.rotations).saturating_add(1),
                    created_at: evidence.session.created_at,
                },
            },
            safe_detail: "The Codex session is attached to this Launch.".into(),
            external_resource_ids: resources,
        }
    }

    async fn run_reach(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let Some(sessions) = self.sessions.as_ref() else {
            return unsupported_effect(claim);
        };
        let Some(signer) = self.capability_signer.as_ref() else {
            return unsupported_effect(claim);
        };
        let authority = match self.github.preflight_capability(launch).await {
            Ok(authority) => authority,
            Err(error) => return broker_failure(error, claim, false),
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
        let expires_at = now.saturating_add(CAPABILITY_MAX_LIFETIME_SECONDS);
        let generation = launch
            .reach
            .as_ref()
            .map_or(1, |reach| reach.policy_generation.saturating_add(1));
        let capability = LaunchCapability {
            version: CAPABILITY_POLICY_VERSION,
            launch_id: launch.id.clone(),
            session_id: session.session_id.clone(),
            // Layer 2 fills this from the exact runtime handle while holding
            // the session lifecycle fence.
            runtime_audience: String::new(),
            service_id: authority.connection.service_id.clone(),
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
            fence: claim.fence,
            not_before_epoch_seconds: now,
            expires_at_epoch_seconds: expires_at,
            issuer: "cma-trigger".into(),
            audience: POLICY_AUDIENCE.into(),
            signature_key_id: signer.key_id().to_string(),
            numeric_binding_observed_at: authority.authority.observed_at.clone(),
            revocation: CapabilityRevocationState::Active,
        };
        let (runtime_audience, receipt) = match sessions
            .install_runtime_policy(&session.session_id, capability, Arc::clone(signer))
            .await
        {
            Ok(installed) => installed,
            Err(error) => return session_failure(error, claim),
        };
        if receipt.generation != generation || receipt.fence != claim.fence || receipt.revoked {
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
        resources.insert("policy_generation".into(), generation.to_string());
        resources.insert("policy_fence".into(), claim.fence.to_string());
        resources.insert("runtime_audience".into(), runtime_audience.clone());
        resources.insert("policy_expires_at".into(), expires_at.to_string());
        EffectExecution::Completed {
            result: LaunchEffectResult::Reach {
                reach: layer2_core::ReachStatus {
                    service_id: authority.connection.service_id.clone(),
                    state: CapabilityRevocationState::Active,
                    policy_generation: generation,
                    policy_fence: claim.fence,
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
            safe_detail: "The runtime acknowledged its launch-scoped GitHub policy.".into(),
            external_resource_ids: resources,
        }
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
        let delivery = match sessions.dispatch_initial_prompt(launch, claim).await {
            Ok(delivery) => delivery,
            Err(error) => return session_failure(error, claim),
        };
        let result = LaunchEffectResult::InitialDispatch {
            delivery: delivery.clone(),
        };
        let mut resources = retained_resources(launch, claim);
        resources.insert("session_id".into(), delivery.session_id.0.clone());
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
        let version = match workspace.latest_checkpoint.as_ref() {
            None => 1,
            Some(checkpoint) => match checkpoint.version.checked_add(1) {
                Some(version) => version,
                None => {
                    return checkpoint_failure(
                        claim,
                        FailureClass::RecoveryRequired,
                        "checkpoint_version_exhausted",
                        "The workspace checkpoint version cannot advance safely.",
                        "Contact support; the previous complete checkpoint remains retained.",
                    )
                }
            },
        };
        let request = layer3_adapter::WorkspaceCheckpointRequest {
            epoch: workspace.workspace_epoch,
            version,
            source_revision: launch.source_sha.clone(),
            baseline_manifest_digest: workspace.baseline.digest.clone(),
        };
        let (captured, runtime_generation) =
            match sessions.checkpoint_workspace(launch, &request).await {
                Ok(captured) => captured,
                Err(error) => return checkpoint_runtime_failure(error, claim),
            };
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
            id: deterministic_checkpoint_id(&claim.intent_key, version),
            launch_id: launch.id.clone(),
            version,
            workspace_epoch: workspace.workspace_epoch,
            session_id: session.session_id.clone(),
            runtime_generation,
            object_reference: String::new(),
            archive_sha256: captured.archive_sha256,
            manifest,
            integrity: CheckpointIntegrity::Complete,
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
        let Some(checkpoint) = workspace.latest_checkpoint.as_ref() else {
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
            .and_then(|workspace| workspace.latest_checkpoint.as_ref())
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

        let authority = match self.github.preflight_capability(launch).await {
            Ok(authority) => authority,
            Err(error) => {
                return broker_failure(error, claim, claim.mode == EffectClaimMode::Reconcile)
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
        let advance = match self.github.publish_branch(&context, &plan).await {
            Ok(advance) => advance,
            Err(error) => {
                return broker_failure(error, claim, claim.mode == EffectClaimMode::Reconcile)
            }
        };
        let mut contribution = contribution.clone();
        let now = rfc3339_at(SystemTime::now());
        let published = match advance {
            super::github::PublicationAdvance::ObjectsWritten(prepared) => {
                contribution.state = layer2_core::PublicationState::ObjectsWritten;
                contribution.base_tree_id = Some(prepared.base_tree_id);
                contribution.tree_id = Some(prepared.tree_id);
                contribution.commit_id = Some(prepared.commit_id);
                contribution.object_ids = prepared.object_ids;
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
                    result: LaunchEffectResult::PullRequest { pull_request },
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
}

#[async_trait]
impl EffectExecutor for GithubEffectExecutor {
    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        match claim.kind {
            LaunchEffectKind::Preflight => self.run_preflight(launch, claim).await,
            LaunchEffectKind::Fork => self.run_fork(launch, claim, false).await,
            LaunchEffectKind::Branch => self.run_branch(launch, claim, false).await,
            LaunchEffectKind::WorkspaceImport => self.run_workspace(launch, claim).await,
            LaunchEffectKind::Sandbox => self.run_sandbox(launch, claim).await,
            LaunchEffectKind::Session => self.run_session(launch, claim).await,
            LaunchEffectKind::Reach => self.run_reach(launch, claim).await,
            LaunchEffectKind::InitialDispatch => self.run_initial_dispatch(launch, claim).await,
            LaunchEffectKind::Checkpoint => self.run_checkpoint(launch, claim).await,
            LaunchEffectKind::Diff => self.run_diff(launch, claim).await,
            LaunchEffectKind::BranchPublication => self.run_publication(launch, claim).await,
            LaunchEffectKind::PullRequest => self.run_pull_request(launch, claim, false).await,
            _ => unsupported_effect(claim),
        }
    }

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        match claim.kind {
            LaunchEffectKind::Preflight => self.run_preflight(launch, claim).await,
            LaunchEffectKind::Fork => self.run_fork(launch, claim, true).await,
            LaunchEffectKind::Branch => self.run_branch(launch, claim, true).await,
            // Every provider call is a read or an idempotent write to the
            // Launch's stable object key; repeating it reconciles the same bytes.
            LaunchEffectKind::WorkspaceImport => self.run_workspace(launch, claim).await,
            LaunchEffectKind::Sandbox => self.run_sandbox(launch, claim).await,
            LaunchEffectKind::Session => self.run_session(launch, claim).await,
            LaunchEffectKind::Reach => self.run_reach(launch, claim).await,
            LaunchEffectKind::InitialDispatch => self.run_initial_dispatch(launch, claim).await,
            LaunchEffectKind::Checkpoint => self.run_checkpoint(launch, claim).await,
            // Diff generation performs only fenced reads plus an immutable,
            // content-addressed object write, so the same intent is safely
            // repeatable during reconciliation.
            LaunchEffectKind::Diff => self.run_diff(launch, claim).await,
            LaunchEffectKind::BranchPublication => self.run_publication(launch, claim).await,
            LaunchEffectKind::PullRequest => self.run_pull_request(launch, claim, true).await,
            _ => unsupported_effect(claim),
        }
    }
}

fn deterministic_checkpoint_id(intent_key: &str, version: u64) -> CheckpointId {
    let digest = Sha256::digest(format!("cma-checkpoint-v1\0{intent_key}\0{version}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 name-based shape over CMA's SHA-256-derived namespace.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    CheckpointId(uuid::Uuid::from_bytes(bytes).to_string())
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

fn execution_failure(execution: EffectExecution) -> LaunchFailure {
    match execution {
        EffectExecution::RetryLater { failure, .. }
        | EffectExecution::ReconcileLater { failure, .. }
        | EffectExecution::RetryWithProgress { failure, .. }
        | EffectExecution::ReconcileWithProgress { failure, .. }
        | EffectExecution::FailedWithProgress { failure, .. }
        | EffectExecution::Failed { failure } => failure,
        EffectExecution::Completed { .. } | EffectExecution::Progressed { .. } => LaunchFailure {
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

fn session_reconcile_later(
    claim: &EffectClaim,
    code: &'static str,
    detail: &'static str,
    external_resource_ids: BTreeMap<String, String>,
) -> EffectExecution {
    let reconcile_at = rfc3339_at(SystemTime::now() + Duration::from_secs(2));
    EffectExecution::ReconcileLater {
        failure: failure(
            FailureClass::RetryableTransient,
            code,
            detail,
            "CMA will reconcile the same deterministic session without creating another one.",
            claim,
            Some(reconcile_at.clone()),
        ),
        reconcile_at,
        external_resource_ids,
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
        }
    } else {
        EffectExecution::Failed {
            failure: failure(class, code, detail, remediation, claim, None),
        }
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
                EffectExecution::RetryLater { failure, retry_at }
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
                    "submodule_unsupported",
                    "Git submodules are not dereferenced by the public Trigger flow.",
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
        TriggerServiceError::Validation(_) => EffectExecution::Failed {
            failure: failure(
                FailureClass::TerminalValidation,
                "launch_preflight_invalid",
                "The retained Launch no longer satisfies its Trigger contract.",
                "Review the Launch and publish a corrected Trigger revision if needed.",
                claim,
                None,
            ),
        },
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
        EffectExecution::RetryLater { failure, retry_at }
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
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use base64::Engine as _;
    use layer2_core::{
        BranchRecord, Confirmation, ConfirmationAction, DiffArtifactId, EffectClaimMode,
        ForkRecord, LaunchState, PublicationState, PullRequestOperation, PullRequestState,
        RepositoryIdentity, RepositoryVisibility, SubjectId,
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
        AuthorityMismatch,
    }

    struct FakeSessions {
        answers: Mutex<VecDeque<SessionAnswer>>,
        prompt_answers: Mutex<VecDeque<Result<layer2_core::PromptDelivery, CoreError>>>,
        checkpoint_answers:
            Mutex<VecDeque<Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError>>>,
        checkpoint_requests: Mutex<Vec<layer3_adapter::WorkspaceCheckpointRequest>>,
        policy_receipt: Mutex<Option<layer3_adapter::RuntimePolicyReceipt>>,
        installed_policies: Mutex<Vec<LaunchCapability>>,
        calls: AtomicUsize,
        prompt_calls: AtomicUsize,
        checkpoint_calls: AtomicUsize,
    }

    impl FakeSessions {
        fn new(answers: impl IntoIterator<Item = SessionAnswer>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                prompt_answers: Mutex::new(VecDeque::new()),
                checkpoint_answers: Mutex::new(VecDeque::new()),
                checkpoint_requests: Mutex::new(Vec::new()),
                policy_receipt: Mutex::new(None),
                installed_policies: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                prompt_calls: AtomicUsize::new(0),
                checkpoint_calls: AtomicUsize::new(0),
            }
        }

        fn with_prompt_answers(
            answers: impl IntoIterator<Item = Result<layer2_core::PromptDelivery, CoreError>>,
        ) -> Self {
            Self {
                answers: Mutex::new(VecDeque::new()),
                prompt_answers: Mutex::new(answers.into_iter().collect()),
                checkpoint_answers: Mutex::new(VecDeque::new()),
                checkpoint_requests: Mutex::new(Vec::new()),
                policy_receipt: Mutex::new(None),
                installed_policies: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                prompt_calls: AtomicUsize::new(0),
                checkpoint_calls: AtomicUsize::new(0),
            }
        }

        fn with_policy_receipt(receipt: layer3_adapter::RuntimePolicyReceipt) -> Self {
            Self {
                answers: Mutex::new(VecDeque::new()),
                prompt_answers: Mutex::new(VecDeque::new()),
                checkpoint_answers: Mutex::new(VecDeque::new()),
                checkpoint_requests: Mutex::new(Vec::new()),
                policy_receipt: Mutex::new(Some(receipt)),
                installed_policies: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                prompt_calls: AtomicUsize::new(0),
                checkpoint_calls: AtomicUsize::new(0),
            }
        }

        fn with_checkpoint_answers(
            answers: impl IntoIterator<
                Item = Result<(layer3_adapter::WorkspaceCheckpointPackage, u64), CoreError>,
            >,
        ) -> Self {
            Self {
                answers: Mutex::new(VecDeque::new()),
                prompt_answers: Mutex::new(VecDeque::new()),
                checkpoint_answers: Mutex::new(answers.into_iter().collect()),
                checkpoint_requests: Mutex::new(Vec::new()),
                policy_receipt: Mutex::new(None),
                installed_policies: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                prompt_calls: AtomicUsize::new(0),
                checkpoint_calls: AtomicUsize::new(0),
            }
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
                } => Ok(TriggerSessionEvidence {
                    session: trigger_session(launch, status, rotations),
                    credential_ready,
                }),
                SessionAnswer::AuthorityMismatch => Err(CoreError::InvalidRequest(
                    "the deterministic session is occupied by different authority".into(),
                )),
            }
        }

        async fn install_runtime_policy(
            &self,
            _session_id: &SessionId,
            mut capability: LaunchCapability,
            signer: Arc<CapabilitySigner>,
        ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError> {
            let Some(receipt) = self.policy_receipt.lock().unwrap().clone() else {
                return Err(CoreError::CapabilityUnsupported(
                    "authenticated_runtime_policy".into(),
                ));
            };
            capability.runtime_audience = "sandbox-runtime-1".into();
            signer
                .sign_policy(&capability)
                .map_err(CoreError::InvalidRequest)?;
            self.installed_policies.lock().unwrap().push(capability);
            Ok(("sandbox-runtime-1".into(), receipt))
        }

        async fn runtime_policy_audience(
            &self,
            _session_id: &SessionId,
        ) -> Result<String, CoreError> {
            Ok("sandbox-runtime-1".into())
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

        async fn retire_runtime_credential(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn runtime_credential_retirement_settled(
            &self,
            _session_id: &SessionId,
        ) -> Result<bool, CoreError> {
            Ok(true)
        }

        async fn dispatch_initial_prompt(
            &self,
            _launch: &Launch,
            _claim: &EffectClaim,
        ) -> Result<layer2_core::PromptDelivery, CoreError> {
            self.prompt_calls.fetch_add(1, Ordering::SeqCst);
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

    #[async_trait]
    impl BaselineObjectStore for FakeReviewStore {
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
    }

    struct FakeGithub {
        answers: Mutex<VecDeque<ForkAnswer>>,
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
                publication_answers: Mutex::new(VecDeque::new()),
                pull_request_answers: Mutex::new(answers.into_iter().collect()),
                create_calls: AtomicUsize::new(0),
                reconcile_calls: AtomicUsize::new(0),
                publication_calls: AtomicUsize::new(0),
                pull_request_write_calls: AtomicUsize::new(0),
                pull_request_reconcile_calls: AtomicUsize::new(0),
            }
        }

        fn answer(&self, context: &ForkContext) -> Result<ForkReconciliation, GithubBrokerError> {
            match self.answers.lock().unwrap().pop_front().unwrap() {
                ForkAnswer::Ready => Ok(ForkReconciliation::Ready(fork(context))),
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
        ) -> Result<ImportedWorkspace, GithubBrokerError> {
            Err(GithubBrokerError::Malformed(
                "workspace answer was not configured".into(),
            ))
        }

        async fn preflight_capability(
            &self,
            launch: &Launch,
        ) -> Result<CapabilityAuthorityEvidence, GithubBrokerError> {
            Ok(CapabilityAuthorityEvidence {
                connection: launch.connection.clone().unwrap(),
                authority: launch.authority.clone().unwrap(),
                upstream: launch.revision.content.upstream.repository.clone(),
                fork: launch.fork.as_ref().unwrap().repository.clone(),
                branch: launch.branch.clone().unwrap(),
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
    async fn ambiguous_fork_write_is_never_returned_as_an_execute_retry() {
        let github = Arc::new(FakeGithub::new([ForkAnswer::Ambiguous]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let launch = fork_launch();

        let result = executor
            .execute(&launch, &claim(EffectClaimMode::Execute))
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
        assert_eq!(github.create_calls.load(Ordering::SeqCst), 1);
        assert_eq!(github.reconcile_calls.load(Ordering::SeqCst), 0);
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
        let launch = fork_launch();

        let result = executor
            .execute(&launch, &claim(EffectClaimMode::Execute))
            .await;

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

    #[tokio::test]
    async fn ambiguous_pull_request_create_moves_to_reconciliation_without_a_second_write() {
        let github = Arc::new(FakeGithub::with_pull_request_answers([Err(
            GithubBrokerError::AmbiguousWrite,
        )]));
        let executor = GithubEffectExecutor::with_port(github.clone(), Duration::from_secs(300));
        let launch = pull_request_launch();

        let result = executor
            .execute(
                &launch,
                &claim_for(LaunchEffectKind::PullRequest, EffectClaimMode::Execute),
            )
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
            super::super::github::PullRequestAdvance::Ready(ready),
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
        let result = executor
            .execute(
                &pull_request_launch(),
                &claim_for(LaunchEffectKind::PullRequest, EffectClaimMode::Execute),
            )
            .await;

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

        assert!(matches!(result, EffectExecution::ReconcileLater { .. }));
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

        assert!(matches!(result, EffectExecution::ReconcileLater { .. }));
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
        let launch = reach_launch();
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;

        let result = executor.execute(&launch, &reach_claim).await;

        assert!(matches!(
            result,
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
        let policies = sessions.installed_policies.lock().unwrap();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].runtime_audience, "sandbox-runtime-1");
        assert_eq!(policies[0].fence, 7);
        assert_eq!(
            policies[0].allowed_ref,
            format!("refs/heads/cma/{}", launch.id.0)
        );
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
        let launch = reach_launch();
        let mut reach_claim = claim_for(LaunchEffectKind::Reach, EffectClaimMode::Execute);
        reach_claim.fence = 7;

        let result = executor.execute(&launch, &reach_claim).await;

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
            let launch = reach_launch();
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
    async fn checkpoint_completion_requires_runtime_manifest_and_destination_proof() {
        let launch = checkpoint_launch();
        let package = checkpoint_package(&launch);
        let sessions = Arc::new(FakeSessions::with_checkpoint_answers([Ok((
            package.clone(),
            2,
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
        assert_eq!(checkpoint.runtime_generation, 2);
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
        assert!(matches!(
            publication_executor(github.clone(), artifact_outage)
                .execute(&fixture.launch, &claim)
                .await,
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
        assert!(matches!(
            publication_executor(github.clone(), checkpoint_outage)
                .execute(&fixture.launch, &claim)
                .await,
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
        assert!(matches!(
            publication_executor(github.clone(), missing)
                .execute(&fixture.launch, &claim)
                .await,
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

        let EffectExecution::Progressed {
            result: LaunchEffectResult::BranchPublication { contribution },
        } = executor
            .execute(
                &launch,
                &claim_for(
                    LaunchEffectKind::BranchPublication,
                    EffectClaimMode::Execute,
                ),
            )
            .await
        else {
            panic!("object creation must yield durable progress");
        };
        assert_eq!(contribution.state, PublicationState::ObjectsWritten);
        assert_publication_authority_unchanged(&immutable, &contribution);
        launch.contribution = Some(contribution);

        let EffectExecution::Progressed {
            result: LaunchEffectResult::BranchPublication { contribution },
        } = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::BranchPublication,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await
        else {
            panic!("object verification must yield a durable ref intent");
        };
        assert_eq!(contribution.state, PublicationState::RefUpdatePending);
        assert_publication_authority_unchanged(&immutable, &contribution);
        launch.contribution = Some(contribution);

        let EffectExecution::Completed {
            result: LaunchEffectResult::BranchPublication { contribution },
            external_resource_ids,
            ..
        } = executor
            .reconcile(
                &launch,
                &claim_for(
                    LaunchEffectKind::BranchPublication,
                    EffectClaimMode::Reconcile,
                ),
            )
            .await
        else {
            panic!("verified ref must complete publication");
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
            let execution = publication_executor(github, store)
                .execute(
                    &fixture.launch,
                    &claim_for(
                        LaunchEffectKind::BranchPublication,
                        EffectClaimMode::Execute,
                    ),
                )
                .await;
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

    fn checkpoint_launch() -> Launch {
        let mut launch = reach_launch();
        launch.workspace = Some(WorkspaceRecord {
            baseline: WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap(),
            baseline_object_reference: "trigger-test/baseline.pack".into(),
            baseline_archive_sha256: "a".repeat(64),
            latest_checkpoint: None,
            previous_checkpoint: None,
            workspace_epoch: 1,
            retention_policy_id: "trigger-test-retention".into(),
            delete_after: "2026-09-27T00:00:00Z".into(),
        });
        launch.state = LaunchState::Contributing;
        launch
    }

    fn diff_launch() -> Launch {
        let mut launch = checkpoint_launch();
        let workspace = launch.workspace.as_mut().unwrap();
        workspace.latest_checkpoint = Some(WorkspaceCheckpoint {
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
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some(workspace.delete_after.clone()),
        });
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
            created_at: "2026-08-27T00:00:00Z".into(),
            delete_after: Some("2026-09-27T00:00:00Z".into()),
        };
        launch.workspace = Some(WorkspaceRecord {
            baseline: baseline.clone(),
            baseline_object_reference: "trigger-test/publication-baseline.pack".into(),
            baseline_archive_sha256: format!("{:x}", Sha256::digest(&baseline_archive)),
            latest_checkpoint: Some(checkpoint.clone()),
            previous_checkpoint: None,
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
            PROVISIONAL_CHANGED_FILE_LIMIT,
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

    fn reconciled_pull_request(
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

    fn test_archive(
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
            archive,
        }
    }

    fn test_signer() -> Arc<CapabilitySigner> {
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

    fn prompt_delivery(
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

    fn session_executor(sessions: Arc<FakeSessions>) -> GithubEffectExecutor {
        GithubEffectExecutor::with_session_port(
            Arc::new(FakeGithub::new([])),
            sessions as Arc<dyn TriggerSessionEffectPort>,
        )
    }

    fn diff_executor(github: Arc<FakeGithub>, store: Arc<FakeReviewStore>) -> GithubEffectExecutor {
        GithubEffectExecutor {
            github,
            workspace_store: Some(store.clone() as Arc<dyn BaselineObjectStore>),
            checkpoint_store: Some(store.clone() as Arc<dyn CheckpointObjectStore>),
            diff_store: Some(store as Arc<dyn DiffArtifactObjectStore>),
            sessions: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
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
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
            changed_file_limit: PROVISIONAL_CHANGED_FILE_LIMIT,
        }
    }

    fn trigger_session(launch: &Launch, status: SessionStatus, rotations: u32) -> Session {
        Session {
            options: Default::default(),
            id: layer2_core::SessionId("sesn_trigger_effect".into()),
            owner: launch.owner.clone(),
            kernel: layer2_core::KernelId::new("codex"),
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

    fn fork(context: &ForkContext) -> ForkRecord {
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
