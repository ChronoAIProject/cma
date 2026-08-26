//! Durable Trigger effects backed by the typed NyxID/GitHub broker.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    rfc3339_at, seconds_from_rfc3339, BaselineObjectStore, EffectClaim, FailureClass, Launch,
    LaunchAuthority, LaunchEffectKind, LaunchEffectResult, LaunchFailure, TriggerAuthorityError,
    TriggerAuthorityVerifier, TriggerServiceError, TriggerStoreError, WorkspaceRecord,
};

use super::github::{
    BranchContext, BranchReconciliation, ForkContext, ForkReconciliation, GithubBroker,
    GithubBrokerError, ImportedWorkspace, WorkspaceContext, WorkspaceUnsupported,
};
use super::worker::{EffectExecution, EffectExecutor};

const DEFAULT_FORK_READINESS_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PROVIDER_RETRY: Duration = Duration::from_secs(300);
const MAX_RETRY_AFTER: u64 = 3_600;

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
}

/// GitHub-backed part of the closed Trigger effect vocabulary.
///
/// Later runtime/workspace adapters compose beside this type. Unsupported
/// kinds fail closed so a configuration mistake cannot silently skip a step.
pub(crate) struct GithubEffectExecutor {
    github: Arc<dyn GithubEffectPort>,
    workspace_store: Option<Arc<dyn BaselineObjectStore>>,
    fork_readiness_timeout: Duration,
}

impl GithubEffectExecutor {
    pub(crate) fn new(github: Arc<GithubBroker>) -> Self {
        Self {
            github,
            workspace_store: None,
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
        }
    }

    pub(crate) fn with_workspace_store(
        github: Arc<GithubBroker>,
        workspace_store: Arc<dyn BaselineObjectStore>,
    ) -> Self {
        Self {
            github,
            workspace_store: Some(workspace_store),
            fork_readiness_timeout: DEFAULT_FORK_READINESS_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_port(github: Arc<dyn GithubEffectPort>, fork_readiness_timeout: Duration) -> Self {
        Self {
            github,
            workspace_store: None,
            fork_readiness_timeout,
        }
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
}

#[async_trait]
impl EffectExecutor for GithubEffectExecutor {
    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        match claim.kind {
            LaunchEffectKind::Preflight => self.run_preflight(launch, claim).await,
            LaunchEffectKind::Fork => self.run_fork(launch, claim, false).await,
            LaunchEffectKind::Branch => self.run_branch(launch, claim, false).await,
            LaunchEffectKind::WorkspaceImport => self.run_workspace(launch, claim).await,
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
            _ => unsupported_effect(claim),
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

    use layer2_core::{
        EffectClaimMode, ForkRecord, LaunchState, RepositoryIdentity, RepositoryVisibility,
    };

    use super::*;

    enum ForkAnswer {
        Ready,
        Absent,
        Ambiguous,
        Occupied,
        Transient,
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

    fn fork_launch() -> Launch {
        let mut launch = super::super::worker::tests::launch_fixture();
        launch.state = LaunchState::ForkPending;
        launch
    }

    fn claim(mode: EffectClaimMode) -> EffectClaim {
        EffectClaim {
            kind: LaunchEffectKind::Fork,
            mode,
            intent_key: "cma-trigger:test:fork:1".into(),
            attempt: 1,
            reconciliation_attempt: u32::from(mode == EffectClaimMode::Reconcile),
            fence: 1,
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
