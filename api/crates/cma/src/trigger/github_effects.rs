//! Durable Trigger effects backed by the typed NyxID/GitHub broker.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, seconds_from_rfc3339, AuthorityProvenance, BaselineObjectStore, BranchRecord,
    CapabilityOperation, CapabilityRevocationState, ConnectionSnapshot, CoreError, DelegationState,
    EffectClaim, FailureClass, Launch, LaunchAuthority, LaunchCapability, LaunchEffectKind,
    LaunchEffectResult, LaunchFailure, RepositoryIdentity, Session, SessionId, SessionLink,
    SessionService, SessionStatus, TriggerAuthorityError, TriggerAuthorityVerifier,
    TriggerServiceError, TriggerStoreError, WorkspaceRecord, CAPABILITY_MAX_LIFETIME_SECONDS,
    CAPABILITY_POLICY_VERSION,
};

use super::capability_signing::{
    CapabilitySigner, RuntimePolicyTombstone, POLICY_AUDIENCE, POLICY_VERSION,
};
use super::github::{
    BranchContext, BranchReconciliation, ForkContext, ForkReconciliation, GithubBroker,
    GithubBrokerError, ImportedWorkspace, WorkspaceContext, WorkspaceUnsupported,
};
use super::worker::{EffectExecution, EffectExecutor};

const DEFAULT_FORK_READINESS_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PROVIDER_RETRY: Duration = Duration::from_secs(300);
const MAX_RETRY_AFTER: u64 = 3_600;

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
    sessions: Option<Arc<dyn TriggerSessionEffectPort>>,
    capability_signer: Option<Arc<CapabilitySigner>>,
    fork_readiness_timeout: Duration,
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
            sessions: None,
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
        }
    }

    pub(crate) fn with_runtime(
        github: Arc<GithubBroker>,
        workspace_store: Arc<dyn BaselineObjectStore>,
        sessions: Arc<SessionService>,
        capability_signer: Arc<CapabilitySigner>,
    ) -> Self {
        Self {
            github,
            workspace_store: Some(workspace_store),
            sessions: Some(Arc::new(SessionServiceEffectPort(sessions))),
            capability_signer: Some(capability_signer),
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_port(github: Arc<dyn GithubEffectPort>, fork_readiness_timeout: Duration) -> Self {
        Self {
            github,
            workspace_store: None,
            sessions: None,
            capability_signer: None,
            fork_readiness_timeout,
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
            sessions: Some(sessions),
            capability_signer: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
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
            sessions: Some(sessions),
            capability_signer: Some(capability_signer),
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
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
            _ => unsupported_effect(claim),
        }
    }
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
        | EffectExecution::Failed { failure } => failure,
        EffectExecution::Completed { .. } => LaunchFailure {
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
        Error::RateLimited { retry_after } => transient_failure(
            claim,
            reconcile_only,
            "github_rate_limited",
            "GitHub temporarily rate-limited this step.",
            retry_after.as_deref(),
        ),
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
            TriggerAuthorityError::ProviderRejected => {
                broker_failure(GithubBrokerError::Rejected { status: 0 }, claim, false)
            }
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
        BranchRecord, EffectClaimMode, ForkRecord, LaunchState, RepositoryIdentity,
        RepositoryVisibility,
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
        policy_receipt: Mutex<Option<layer3_adapter::RuntimePolicyReceipt>>,
        installed_policies: Mutex<Vec<LaunchCapability>>,
        calls: AtomicUsize,
    }

    impl FakeSessions {
        fn new(answers: impl IntoIterator<Item = SessionAnswer>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                policy_receipt: Mutex::new(None),
                installed_policies: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
            }
        }

        fn with_policy_receipt(receipt: layer3_adapter::RuntimePolicyReceipt) -> Self {
            Self {
                answers: Mutex::new(VecDeque::new()),
                policy_receipt: Mutex::new(Some(receipt)),
                installed_policies: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
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
    }

    struct FakeGithub {
        answers: Mutex<VecDeque<ForkAnswer>>,
        create_calls: AtomicUsize,
        reconcile_calls: AtomicUsize,
    }

    impl FakeGithub {
        fn new(answers: impl IntoIterator<Item = ForkAnswer>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                create_calls: AtomicUsize::new(0),
                reconcile_calls: AtomicUsize::new(0),
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
