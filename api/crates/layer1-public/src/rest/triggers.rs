//! Public Trigger, connection, Launch, and progress resources.

use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use layer2_core::{
    BillingMode, BranchRecord, CheckResult, CommercialPolicy, CommitIdentity,
    ConfirmPullRequestCommand, Confirmation, ConfirmationAction, ContributionPolicy,
    ContributionRecord, CredentialRoute, DiffArtifact, DiffArtifactId, DiffFile, EditableField,
    EffectiveConfiguration, ExecutorPolicy, ForkRecord, InputFieldDefinition, Launch,
    LaunchBlockedReason, LaunchEvent, LaunchEventKind, LaunchFailure, LaunchId, LaunchOverrides,
    LaunchProgressState, LaunchState, LaunchStep, OverridePolicy, PricingSnapshot, PromptDelivery,
    PublicProjectionPolicy, PublicTriggerId, PullRequestDraft, PullRequestOperation,
    PullRequestRecord, QuotaSnapshot, ReachStatus, RecoverInitialPromptCommand,
    RepositoryAccessPolicy, RequiredCapability, ResourcePolicy, RuntimeAdmissionSnapshot,
    ScopeClass, SessionCredentialRetirement, SessionId, SessionLink, SetupPolicy, SourceMode,
    StartAdmissionSnapshot, StartLaunchCommand, TriggerAvailability, TriggerConnection,
    TriggerConnectionAuthorization, TriggerConnectionManager, TriggerDraft, TriggerId,
    TriggerPageRequest, TriggerPreview, TriggerPreviewCost, TriggerRepositorySelection,
    TriggerRevision, TriggerRevisionContent, TriggerRevisionId, TriggerServiceError,
    TriggerSessionPolicy, TriggerStoreError, TriggerTask, UpstreamRevision, WorkflowChangePolicy,
    WorkspaceCheckpoint, WorkspaceManifest, WorkspaceRecord,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::trigger_openapi_shapes as trigger_schema;
use super::{
    idempotency, problem_document, problem_response, BodyProblem, RequestFailure,
    TriggerValidationCategory, TriggersState,
};
use crate::auth::AuthenticatedCaller;

const PUBLIC_PREVIEW_CACHE_SECONDS: u64 = 60;

pub(super) fn documented_router(state: TriggersState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(public_preview))
        .routes(routes!(create_trigger, list_triggers))
        .routes(routes!(read_trigger, update_trigger))
        .routes(routes!(publish_trigger))
        .routes(routes!(list_revisions))
        .routes(routes!(read_revision))
        .routes(routes!(retire_revision))
        .routes(routes!(list_connections, begin_connection))
        .routes(routes!(revoke_connection))
        .routes(routes!(reconnect_connection))
        .routes(routes!(preflight_connection))
        .routes(routes!(resolve_connection_repository))
        .routes(routes!(create_or_resume_launch))
        .routes(routes!(create_new_launch))
        .routes(routes!(list_launches))
        .routes(routes!(read_launch))
        .routes(routes!(update_launch_overrides))
        .routes(routes!(select_launch_connection))
        .routes(routes!(start_launch))
        .routes(routes!(review_launch_diff))
        .routes(routes!(read_launch_diff))
        .routes(routes!(publish_launch_branch))
        .routes(routes!(review_pull_request))
        .routes(routes!(open_pull_request))
        .routes(routes!(update_pull_request))
        .routes(routes!(refresh_pull_request_status))
        .routes(routes!(download_diff_artifact))
        .routes(routes!(recover_initial_prompt))
        .routes(routes!(complete_launch))
        .routes(routes!(cancel_launch))
        .routes(routes!(launch_progress))
        .with_state(state)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RepositorySelectionRequest {
    repository_id: u64,
    selected_ref: String,
    contribution_base_ref: String,
    #[schema(value_type = String)]
    source_mode: SourceMode,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct TriggerContentRequest {
    schema_version: u32,
    publisher_display_identity: String,
    repository: RepositorySelectionRequest,
    #[schema(value_type = trigger_schema::TriggerTask)]
    task: TriggerTask,
    #[schema(value_type = trigger_schema::TriggerSessionPolicy)]
    session: TriggerSessionPolicy,
    #[schema(value_type = trigger_schema::RepositoryAccessPolicy)]
    repository_access: RepositoryAccessPolicy,
    #[schema(value_type = trigger_schema::SetupPolicy)]
    setup: SetupPolicy,
    #[schema(value_type = trigger_schema::ContributionPolicy)]
    contribution: ContributionPolicy,
    #[schema(value_type = trigger_schema::CommercialPolicy)]
    commercial_policy: CommercialPolicy,
    #[schema(value_type = trigger_schema::OverridePolicy)]
    override_policy: OverridePolicy,
    #[schema(value_type = trigger_schema::PublicProjectionPolicy)]
    public_projection: PublicProjectionPolicy,
}

impl TriggerContentRequest {
    async fn resolve(
        self,
        connections: &dyn TriggerConnectionManager,
        subject: &layer2_core::SubjectId,
        selected_service_id: &str,
    ) -> Result<TriggerRevisionContent, TriggerServiceError> {
        let selected = connections
            .resolve_repository(
                subject,
                selected_service_id,
                self.repository.repository_id,
                &self.repository.selected_ref,
            )
            .await?;
        if self.repository.contribution_base_ref != self.repository.selected_ref {
            let base = connections
                .resolve_repository(
                    subject,
                    selected_service_id,
                    self.repository.repository_id,
                    &self.repository.contribution_base_ref,
                )
                .await?;
            if base.repository.repository_id != selected.repository.repository_id
                || base.repository.repository_node_id != selected.repository.repository_node_id
            {
                return Err(layer2_core::TriggerAuthorityError::AuthorityDrift.into());
            }
        }
        Ok(self.into_content(subject.clone(), selected))
    }

    fn into_content(
        self,
        subject: layer2_core::SubjectId,
        selected: TriggerRepositorySelection,
    ) -> TriggerRevisionContent {
        TriggerRevisionContent {
            schema_version: self.schema_version,
            trigger_id: TriggerId::new(),
            revision_number: 1,
            publisher_subject_id: subject,
            publisher_display_identity: self.publisher_display_identity,
            upstream: UpstreamRevision {
                repository: selected.repository,
                source_mode: self.repository.source_mode,
                observed_source_sha: Some(selected.source_sha),
                contribution_base_ref: self.repository.contribution_base_ref,
            },
            task: self.task,
            session: self.session,
            repository_access: self.repository_access,
            setup: self.setup,
            contribution: self.contribution,
            commercial_policy: self.commercial_policy,
            override_policy: self.override_policy,
            public_projection: self.public_projection,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CreateTriggerRequest {
    selected_service_id: String,
    content: TriggerContentRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateTriggerRequest {
    expected_version: u64,
    selected_service_id: String,
    content: TriggerContentRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PublishTriggerRequest {
    expected_version: u64,
    selected_service_id: String,
}

#[derive(Debug, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
struct EmptyCommand {}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RepositoryLocatorParams {
    owner: String,
    name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct BeginConnectionRequest {
    intent_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateOverridesRequest {
    expected_revision: u64,
    #[schema(value_type = trigger_schema::LaunchOverrides)]
    overrides: LaunchOverrides,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SelectConnectionRequest {
    expected_revision: u64,
    service_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ConfirmLaunchRequest {
    expected_revision: u64,
    snapshot_digest: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RecoverInitialPromptRequest {
    expected_revision: u64,
    snapshot_digest: String,
    #[serde(default)]
    acknowledge_duplicate_execution_risk: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum PullRequestOperationRequest {
    Open,
    Update,
}

impl From<PullRequestOperationRequest> for PullRequestOperation {
    fn from(value: PullRequestOperationRequest) -> Self {
        match value {
            PullRequestOperationRequest::Open => Self::Open,
            PullRequestOperationRequest::Update => Self::Update,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PullRequestReviewRequest {
    operation: PullRequestOperationRequest,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ConfirmPullRequestRequest {
    expected_revision: u64,
    snapshot_digest: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RefreshPullRequestStatusRequest {
    expected_revision: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct StartLaunchRequest {
    expected_revision: u64,
    snapshot_digest: String,
    #[serde(default)]
    acknowledge_workflow_risk: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct NewLaunchRequest {
    acknowledge_additional_resources: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageParams {
    #[serde(default = "default_page_limit")]
    limit: u32,
    #[serde(default)]
    cursor: Option<String>,
}

fn default_page_limit() -> u32 {
    layer2_core::DEFAULT_TRIGGER_PAGE_SIZE
}

impl Default for PageParams {
    fn default() -> Self {
        Self {
            limit: default_page_limit(),
            cursor: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchPageParams {
    #[serde(default)]
    state: Option<LaunchState>,
    #[serde(default)]
    trigger_public_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default = "default_page_limit")]
    limit: u32,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressParams {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_progress_limit")]
    limit: usize,
}

fn default_progress_limit() -> usize {
    100
}

#[derive(Debug, Serialize, ToSchema)]
struct SessionSizeView {
    cpu_millis: u32,
    memory_mib: u32,
}

impl From<layer2_core::SessionSize> for SessionSizeView {
    fn from(value: layer2_core::SessionSize) -> Self {
        Self {
            cpu_millis: value.cpu_millis(),
            memory_mib: value.memory_mib(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TriggerPreviewView {
    public_id: String,
    #[schema(value_type = trigger_schema::TriggerPreviewAvailability)]
    availability: String,
    #[schema(value_type = Option<trigger_schema::TriggerPreviewUnavailableReason>)]
    unavailable_reason: Option<String>,
    publisher_display_identity: String,
    repository_id: u64,
    repository_owner: String,
    repository_name: String,
    #[schema(value_type = trigger_schema::RepositoryVisibility)]
    visibility: String,
    allow_forking: bool,
    public_summary: String,
    #[schema(value_type = trigger_schema::PermissionMode)]
    permission_ceiling: String,
    #[schema(value_type = Vec<trigger_schema::RequiredCapability>)]
    required_capabilities: Vec<String>,
    resource_default: SessionSizeView,
    resource_ceiling: SessionSizeView,
    timeout_default_seconds: u32,
    timeout_ceiling_seconds: u32,
    #[schema(value_type = trigger_schema::BillingMode)]
    billing_mode: String,
    pricing_policy_reference: String,
    estimated_cost: TriggerPreviewCostView,
    revision_digest: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TriggerPreviewCostView {
    ExactZero {
        currency: String,
        amount_micros: u64,
        display: String,
    },
    ContributorEstimate {
        currency: String,
        amount_micros: u64,
        display: String,
        quoted_at: String,
    },
    QuoteUnavailable,
}

impl From<TriggerPreviewCost> for TriggerPreviewCostView {
    fn from(value: TriggerPreviewCost) -> Self {
        match value {
            TriggerPreviewCost::ExactZero {
                currency,
                amount_micros,
                display,
            } => Self::ExactZero {
                currency,
                amount_micros,
                display,
            },
            TriggerPreviewCost::ContributorEstimate {
                currency,
                amount_micros,
                display,
                quoted_at,
            } => Self::ContributorEstimate {
                currency,
                amount_micros,
                display,
                quoted_at,
            },
            TriggerPreviewCost::QuoteUnavailable => Self::QuoteUnavailable,
        }
    }
}

impl From<TriggerPreview> for TriggerPreviewView {
    fn from(value: TriggerPreview) -> Self {
        Self {
            public_id: value.public_id.0,
            availability: enum_name(value.availability),
            unavailable_reason: value.unavailable_reason.map(enum_name),
            publisher_display_identity: value.publisher_display_identity,
            repository_id: value.repository_id,
            repository_owner: value.repository_owner,
            repository_name: value.repository_name,
            visibility: enum_name(value.visibility),
            allow_forking: value.allow_forking,
            public_summary: value.public_summary,
            permission_ceiling: enum_name(value.permission_ceiling),
            required_capabilities: value
                .required_capabilities
                .into_iter()
                .map(enum_name)
                .collect(),
            resource_default: value.resource_default.into(),
            resource_ceiling: value.resource_ceiling.into(),
            timeout_default_seconds: value.timeout_default_seconds,
            timeout_ceiling_seconds: value.timeout_ceiling_seconds,
            billing_mode: enum_name(value.billing_mode),
            pricing_policy_reference: value.pricing_policy_reference,
            estimated_cost: value.estimated_cost.into(),
            revision_digest: value.revision_digest,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerRepositorySelectionView {
    #[schema(value_type = trigger_schema::RepositoryIdentity)]
    repository: layer2_core::RepositoryIdentity,
    source_sha: String,
}

impl From<TriggerRepositorySelection> for TriggerRepositorySelectionView {
    fn from(value: TriggerRepositorySelection) -> Self {
        Self {
            repository: value.repository,
            source_sha: value.source_sha,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerDraftView {
    id: String,
    selected_service_id: String,
    version: u64,
    next_revision_number: u32,
    #[schema(value_type = trigger_schema::TriggerRevisionContent)]
    content: TriggerRevisionContent,
    created_at: String,
    updated_at: String,
}

impl From<TriggerDraft> for TriggerDraftView {
    fn from(value: TriggerDraft) -> Self {
        Self {
            id: value.id.0,
            selected_service_id: value.selected_service_id,
            version: value.version,
            next_revision_number: value.next_revision_number,
            content: value.content,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct PublisherAuthorityView {
    service_id: String,
    service_state_version: i64,
    account_id: u64,
    account_login: String,
    #[schema(value_type = trigger_schema::PublisherRepositoryRole)]
    role: String,
    verified_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerRevisionView {
    id: String,
    public_id: String,
    digest: String,
    #[schema(value_type = trigger_schema::TriggerAvailability)]
    availability: String,
    #[schema(value_type = trigger_schema::TriggerRevisionContent)]
    content: TriggerRevisionContent,
    authority: PublisherAuthorityView,
    published_at: String,
    retired_at: Option<String>,
}

impl From<TriggerRevision> for TriggerRevisionView {
    fn from(value: TriggerRevision) -> Self {
        let authority = PublisherAuthorityView {
            service_id: value.publisher_authority.connection.service_id.clone(),
            service_state_version: value.publisher_authority.connection.service_state_version,
            account_id: value.publisher_authority.connection.account.account_id,
            account_login: value.publisher_authority.connection.account.login.clone(),
            role: enum_name(value.publisher_authority.role),
            verified_at: value.publisher_authority.verified_at.clone(),
        };
        Self {
            id: value.id.0,
            public_id: value.public_id.0,
            digest: value.digest,
            availability: enum_name(value.availability),
            content: value.content,
            authority,
            published_at: value.published_at,
            retired_at: value.retired_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerDraftPageView {
    items: Vec<TriggerDraftView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerRevisionPageView {
    items: Vec<TriggerRevisionView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerConnectionView {
    service_id: String,
    state_version: i64,
    #[schema(value_type = trigger_schema::ConnectionState)]
    state: String,
    account_id: Option<u64>,
    account_node_id: Option<String>,
    account_login: Option<String>,
    avatar_url: Option<String>,
    granted_scopes: Vec<String>,
    consent_observed_at: Option<String>,
}

impl From<TriggerConnection> for TriggerConnectionView {
    fn from(value: TriggerConnection) -> Self {
        let (account_id, account_node_id, account_login, avatar_url) = value
            .account
            .map(|account| {
                (
                    Some(account.account_id),
                    Some(account.account_node_id),
                    Some(account.login),
                    account.avatar_url,
                )
            })
            .unwrap_or_default();
        Self {
            service_id: value.service_id,
            state_version: value.state_version,
            state: enum_name(value.state),
            account_id,
            account_node_id,
            account_login,
            avatar_url,
            granted_scopes: value.granted_scopes,
            consent_observed_at: value.consent_observed_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerConnectionsView {
    connections: Vec<TriggerConnectionView>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ConnectionAuthorizationView {
    service_id: String,
    authorization_url: String,
    attempt_nonce: String,
}

impl From<TriggerConnectionAuthorization> for ConnectionAuthorizationView {
    fn from(value: TriggerConnectionAuthorization) -> Self {
        Self {
            service_id: value.service_id,
            authorization_url: value.authorization_url,
            attempt_nonce: value.attempt_nonce,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LaunchView {
    id: String,
    trigger_public_id: String,
    revision_digest: String,
    source_sha: String,
    #[schema(value_type = trigger_schema::LaunchState)]
    state: String,
    revision_number: u64,
    #[schema(value_type = trigger_schema::TriggerRevisionContent)]
    revision_content: TriggerRevisionContent,
    #[schema(value_type = Option<trigger_schema::LaunchBlockedReason>)]
    blocked_reason: Option<LaunchBlockedReason>,
    #[schema(value_type = Option<trigger_schema::LaunchFailure>)]
    failure: Option<LaunchFailure>,
    /// Durable step retained while authentication or another recovery action
    /// blocks orchestration.
    #[schema(value_type = Option<trigger_schema::LaunchState>)]
    resume_state: Option<String>,
    #[schema(value_type = trigger_schema::LaunchOverrides)]
    overrides: LaunchOverrides,
    #[schema(value_type = trigger_schema::EffectiveConfiguration)]
    effective: EffectiveConfiguration,
    #[schema(value_type = Option<trigger_schema::PricingSnapshot>)]
    pricing: Option<PricingSnapshot>,
    #[schema(value_type = Option<trigger_schema::RuntimeAdmissionSnapshot>)]
    runtime_admission: Option<RuntimeAdmissionSnapshot>,
    #[schema(value_type = Option<trigger_schema::QuotaSnapshot>)]
    review_quota: Option<QuotaSnapshot>,
    #[schema(value_type = Option<trigger_schema::StartAdmissionSnapshot>)]
    start_admission: Option<StartAdmissionSnapshot>,
    connection: Option<TriggerConnectionView>,
    workflow_paths: Vec<String>,
    workflow_risk_acknowledged: bool,
    steps: Vec<LaunchStepView>,
    #[schema(value_type = Option<trigger_schema::ForkRecord>)]
    fork: Option<ForkRecord>,
    #[schema(value_type = Option<trigger_schema::BranchRecord>)]
    branch: Option<BranchRecord>,
    workspace: Option<WorkspaceRecordView>,
    #[schema(value_type = Option<trigger_schema::SessionLink>)]
    session: Option<SessionLink>,
    #[schema(value_type = Option<trigger_schema::SessionCredentialRetirement>)]
    session_credential_retirement: Option<SessionCredentialRetirement>,
    #[schema(value_type = Option<trigger_schema::ReachStatus>)]
    reach: Option<ReachStatus>,
    #[schema(value_type = Option<trigger_schema::PromptDelivery>)]
    delivery: Option<PromptDelivery>,
    contribution_checkpoint_version: Option<u64>,
    diff: Option<DiffArtifactView>,
    contribution: Option<ContributionRecordView>,
    pull_request: Option<PullRequestRecordView>,
    created_at: String,
    updated_at: String,
    draft_expires_at: String,
    started_at: Option<String>,
    terminal_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DiffArtifactView {
    id: String,
    checkpoint_id: String,
    baseline_digest: String,
    checkpoint_digest: String,
    expected_branch_head: String,
    observed_branch_head: String,
    changed_file_limit: u32,
    total_changed_files: u32,
    #[schema(value_type = Vec<trigger_schema::DiffFile>)]
    files: Vec<DiffFile>,
    changed_bytes: u64,
    digest: String,
    object_bytes: u64,
    #[schema(value_type = trigger_schema::CommitIdentity)]
    author: CommitIdentity,
    #[schema(value_type = trigger_schema::CommitIdentity)]
    committer: CommitIdentity,
    workflow_paths: Vec<String>,
    #[schema(value_type = trigger_schema::DiffValidationOutcome)]
    validation: String,
    #[schema(value_type = Vec<trigger_schema::CheckResult>)]
    checks: Vec<CheckResult>,
    created_at: String,
    delete_after: String,
    download_url: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct WorkspaceRecordView {
    #[schema(value_type = trigger_schema::WorkspaceManifest)]
    baseline: WorkspaceManifest,
    baseline_archive_sha256: String,
    latest_checkpoint: Option<WorkspaceCheckpointView>,
    previous_checkpoint: Option<WorkspaceCheckpointView>,
    workspace_epoch: u64,
    retention_policy_id: String,
    delete_after: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct WorkspaceCheckpointView {
    id: String,
    version: u64,
    workspace_epoch: u64,
    session_id: String,
    runtime_generation: u64,
    archive_sha256: String,
    #[schema(value_type = trigger_schema::WorkspaceManifest)]
    manifest: WorkspaceManifest,
    #[schema(value_type = trigger_schema::CheckpointIntegrity)]
    integrity: String,
    created_at: String,
    delete_after: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ContributionRecordView {
    id: String,
    diff_artifact_id: String,
    revision_digest: String,
    baseline_digest: String,
    checkpoint_digest: String,
    diff_digest: String,
    branch_name: String,
    #[schema(value_type = trigger_schema::PublicationState)]
    state: String,
    expected_head: String,
    base_tree_id: Option<String>,
    tree_id: Option<String>,
    commit_id: Option<String>,
    resulting_head: Option<String>,
    object_ids: Vec<String>,
    #[schema(value_type = trigger_schema::CommitIdentity)]
    author: CommitIdentity,
    #[schema(value_type = trigger_schema::CommitIdentity)]
    committer: CommitIdentity,
    confirmation_snapshot_digest: String,
    confirmed_at: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct PullRequestRecordView {
    #[schema(value_type = trigger_schema::PullRequestOperation)]
    operation: String,
    marker: String,
    upstream_repository_id: u64,
    upstream_repository_node_id: String,
    fork_repository_id: u64,
    fork_repository_node_id: String,
    base_ref: String,
    head_owner: String,
    head_branch: String,
    head_ref: String,
    expected_base_revision: String,
    expected_head_revision: String,
    previous_head_revision: Option<String>,
    target_number: Option<u64>,
    update_metadata: bool,
    expected_previous_title: Option<String>,
    expected_previous_body_digest: Option<String>,
    title: String,
    body: String,
    body_digest: String,
    intent_digest: String,
    provider_id: Option<u64>,
    provider_node_id: Option<String>,
    number: Option<u64>,
    canonical_url: Option<String>,
    #[schema(value_type = trigger_schema::PullRequestState)]
    state: String,
    base_revision: Option<String>,
    head_revision: Option<String>,
    observed_title: Option<String>,
    observed_body_digest: Option<String>,
    #[schema(value_type = trigger_schema::Mergeability)]
    mergeability: String,
    #[schema(value_type = trigger_schema::CheckSummaryState)]
    checks: String,
    confirmation_snapshot_digest: String,
    confirmed_at: String,
    created_at: String,
    updated_at: String,
    provider_created_at: Option<String>,
    provider_updated_at: Option<String>,
    #[schema(value_type = Option<trigger_schema::LaunchFailure>)]
    status_failure: Option<LaunchFailure>,
    provider_request_id: Option<String>,
    head_matches_launch: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PullRequestDraftView {
    launch_id: String,
    #[schema(value_type = trigger_schema::PullRequestOperation)]
    operation: String,
    marker: String,
    upstream_repository_id: u64,
    upstream_repository_node_id: String,
    fork_repository_id: u64,
    fork_repository_node_id: String,
    base_ref: String,
    head_owner: String,
    head_branch: String,
    head_ref: String,
    expected_base_revision: String,
    expected_head_revision: String,
    previous_head_revision: Option<String>,
    target_number: Option<u64>,
    target_provider_id: Option<u64>,
    target_provider_node_id: Option<String>,
    update_metadata: bool,
    expected_previous_title: Option<String>,
    expected_previous_body_digest: Option<String>,
    title: String,
    body: String,
    body_digest: String,
    intent_digest: String,
}

impl From<PullRequestDraft> for PullRequestDraftView {
    fn from(value: PullRequestDraft) -> Self {
        Self {
            launch_id: value.launch_id.0,
            operation: enum_name(value.operation),
            marker: value.marker,
            upstream_repository_id: value.upstream_repository_id,
            upstream_repository_node_id: value.upstream_repository_node_id,
            fork_repository_id: value.fork_repository_id,
            fork_repository_node_id: value.fork_repository_node_id,
            base_ref: value.base_ref,
            head_owner: value.head_owner,
            head_branch: value.head_branch,
            head_ref: value.head_ref,
            expected_base_revision: value.expected_base_revision,
            expected_head_revision: value.expected_head_revision,
            previous_head_revision: value.previous_head_revision,
            target_number: value.target_number,
            target_provider_id: value.target_provider_id,
            target_provider_node_id: value.target_provider_node_id,
            update_metadata: value.update_metadata,
            expected_previous_title: value.expected_previous_title,
            expected_previous_body_digest: value.expected_previous_body_digest,
            title: value.title,
            body: value.body,
            body_digest: value.body_digest,
            intent_digest: value.intent_digest,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DiffReviewView {
    launch_id: String,
    revision_number: u64,
    #[schema(value_type = trigger_schema::LaunchState)]
    state: String,
    source_sha: String,
    workflow_paths: Vec<String>,
    artifact: Option<DiffArtifactView>,
    contribution: Option<ContributionRecordView>,
    pull_request: Option<PullRequestRecordView>,
}

impl DiffReviewView {
    fn from_launch(value: Launch) -> Self {
        let launch_id = value.id.0;
        Self {
            artifact: value
                .diff
                .map(|artifact| DiffArtifactView::from_artifact(&launch_id, artifact)),
            launch_id,
            revision_number: value.revision_number,
            state: enum_name(value.state),
            source_sha: value.source_sha,
            workflow_paths: value.workflow_paths,
            contribution: value.contribution.map(Into::into),
            pull_request: value.pull_request.map(Into::into),
        }
    }
}

impl From<ContributionRecord> for ContributionRecordView {
    fn from(value: ContributionRecord) -> Self {
        Self {
            id: value.id.0,
            diff_artifact_id: value.diff_artifact_id.0,
            revision_digest: value.revision_digest,
            baseline_digest: value.baseline_digest,
            checkpoint_digest: value.checkpoint_digest,
            diff_digest: value.diff_digest,
            branch_name: value.branch_name,
            state: enum_name(value.state),
            expected_head: value.expected_head,
            base_tree_id: value.base_tree_id,
            tree_id: value.tree_id,
            commit_id: value.commit_id,
            resulting_head: value.resulting_head,
            object_ids: value.object_ids,
            author: value.author,
            committer: value.committer,
            confirmation_snapshot_digest: value.confirmation.snapshot_digest,
            confirmed_at: value.confirmation.confirmed_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<PullRequestRecord> for PullRequestRecordView {
    fn from(value: PullRequestRecord) -> Self {
        let status_failure = value.status_sync.last_failure.clone();
        let provider_request_id = value.status_sync.provider_request_id.clone();
        let head_matches_launch = value.status_sync.head_matches_launch;
        Self {
            operation: enum_name(value.intent.operation),
            marker: value.intent.marker,
            upstream_repository_id: value.intent.upstream_repository_id,
            upstream_repository_node_id: value.intent.upstream_repository_node_id,
            fork_repository_id: value.intent.fork_repository_id,
            fork_repository_node_id: value.intent.fork_repository_node_id,
            base_ref: value.intent.base_ref,
            head_owner: value.intent.head_owner,
            head_branch: value.intent.head_branch,
            head_ref: value.intent.head_ref,
            expected_base_revision: value.intent.expected_base_revision,
            expected_head_revision: value.intent.expected_head_revision,
            previous_head_revision: value.intent.previous_head_revision,
            target_number: value.intent.target_number,
            update_metadata: value.intent.update_metadata,
            expected_previous_title: value.intent.expected_previous_title,
            expected_previous_body_digest: value.intent.expected_previous_body_digest,
            title: value.intent.title,
            body: value.intent.body,
            body_digest: value.intent.body_digest,
            intent_digest: value.intent.intent_digest,
            provider_id: value.provider_id,
            provider_node_id: value.provider_node_id,
            number: value.number,
            canonical_url: value.canonical_url,
            state: enum_name(value.state),
            base_revision: value.base_revision,
            head_revision: value.head_revision,
            observed_title: value.observed_title,
            observed_body_digest: value.observed_body_digest,
            mergeability: enum_name(value.mergeability),
            checks: enum_name(value.checks),
            confirmation_snapshot_digest: value.confirmation.snapshot_digest,
            confirmed_at: value.confirmation.confirmed_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            provider_created_at: value.provider_created_at,
            provider_updated_at: value.provider_updated_at,
            status_failure,
            provider_request_id,
            head_matches_launch,
        }
    }
}

impl From<WorkspaceCheckpoint> for WorkspaceCheckpointView {
    fn from(value: WorkspaceCheckpoint) -> Self {
        Self {
            id: value.id.0,
            version: value.version,
            workspace_epoch: value.workspace_epoch,
            session_id: value.session_id.0,
            runtime_generation: value.runtime_generation,
            archive_sha256: value.archive_sha256,
            manifest: value.manifest,
            integrity: enum_name(value.integrity),
            created_at: value.created_at,
            delete_after: value.delete_after,
        }
    }
}

impl From<WorkspaceRecord> for WorkspaceRecordView {
    fn from(value: WorkspaceRecord) -> Self {
        Self {
            baseline: value.baseline,
            baseline_archive_sha256: value.baseline_archive_sha256,
            latest_checkpoint: value.latest_checkpoint.map(Into::into),
            previous_checkpoint: value.previous_checkpoint.map(Into::into),
            workspace_epoch: value.workspace_epoch,
            retention_policy_id: value.retention_policy_id,
            delete_after: value.delete_after,
        }
    }
}

impl DiffArtifactView {
    fn from_artifact(launch_id: &str, value: DiffArtifact) -> Self {
        let artifact_id = value.id.0;
        Self {
            id: artifact_id.clone(),
            checkpoint_id: value.checkpoint_id.0,
            baseline_digest: value.baseline_digest,
            checkpoint_digest: value.checkpoint_digest,
            expected_branch_head: value.expected_branch_head,
            observed_branch_head: value.observed_branch_head,
            changed_file_limit: value.changed_file_limit,
            total_changed_files: value.total_changed_files,
            files: value.files,
            changed_bytes: value.changed_bytes,
            digest: value.digest,
            object_bytes: value.object_bytes,
            author: value.author,
            committer: value.committer,
            workflow_paths: value.workflow_paths,
            validation: enum_name(value.validation),
            checks: value.checks,
            created_at: value.created_at,
            delete_after: value.delete_after,
            download_url: format!(
                "/api/v1/launches/{launch_id}/diff-artifacts/{artifact_id}/download"
            ),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchStepView {
    #[schema(value_type = trigger_schema::LaunchStepKey)]
    key: String,
    sequence: u32,
    #[schema(value_type = trigger_schema::LaunchStepStatus)]
    status: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    attempt: u32,
    safe_detail: String,
    remediation: String,
    external_resource_ids: std::collections::BTreeMap<String, String>,
    correlation_id: String,
}

impl From<LaunchStep> for LaunchStepView {
    fn from(value: LaunchStep) -> Self {
        Self {
            key: enum_name(value.key),
            sequence: value.sequence,
            status: enum_name(value.status),
            started_at: value.started_at,
            completed_at: value.completed_at,
            attempt: value.attempt,
            safe_detail: value.safe_detail,
            remediation: value.remediation,
            external_resource_ids: public_external_resource_ids(value.external_resource_ids),
            correlation_id: value.correlation_id,
        }
    }
}

fn public_external_resource_ids(
    values: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    values
        .into_iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "base_tree_id"
                    | "branch_head"
                    | "branch_ref"
                    | "checkpoint_archive_sha256"
                    | "checkpoint_id"
                    | "checkpoint_manifest_digest"
                    | "checkpoint_quiesced_writers"
                    | "checkpoint_version"
                    | "cleanup_target_count"
                    | "commit_id"
                    | "diff_artifact_digest"
                    | "diff_artifact_id"
                    | "diff_changed_files"
                    | "diff_validation"
                    | "environment_profile_id"
                    | "fork_readiness_deadline"
                    | "fork_repository_id"
                    | "fork_repository_node_id"
                    | "policy_expires_at"
                    | "policy_fence"
                    | "policy_generation"
                    | "pull_request_head"
                    | "pull_request_node_id"
                    | "pull_request_number"
                    | "pull_request_provider_id"
                    | "pull_request_url"
                    | "repository_id"
                    | "resulting_head"
                    | "runtime_audience"
                    | "runtime_gateway_probe_outcome"
                    | "runtime_gateway_probe_version"
                    | "runtime_generation"
                    | "sandbox_id"
                    | "service_id"
                    | "session_id"
                    | "source_sha"
                    | "tree_id"
                    | "workspace_file_count"
                    | "workspace_manifest_digest"
            )
        })
        .collect()
}

const PUBLIC_TRIGGER_KERNEL_ID: &str = "codex-trigger-native-v1";
const LEGACY_PUBLIC_TRIGGER_KERNEL_ID: &str = "codex";

fn classify_public_launch_runtime(
    launch: &Launch,
) -> Result<layer2_core::RuntimeAdmissionRoute, TriggerServiceError> {
    launch
        .validated_runtime_admission_route(
            PUBLIC_TRIGGER_KERNEL_ID,
            LEGACY_PUBLIC_TRIGGER_KERNEL_ID,
            &layer2_core::CMA_TRIGGER_REQUIRED_CAPABILITIES,
        )
        .map_err(|error| TriggerServiceError::RuntimeRouteUnavailable {
            field: error.field.unwrap_or_else(|| "runtime_admission".into()),
        })
}

fn ensure_readable_launch_runtime(launch: &Launch) -> Result<(), TriggerServiceError> {
    match classify_public_launch_runtime(launch)? {
        layer2_core::RuntimeAdmissionRoute::Current
        | layer2_core::RuntimeAdmissionRoute::Historical => Ok(()),
        layer2_core::RuntimeAdmissionRoute::Invalid => {
            Err(TriggerServiceError::RuntimeRouteUnavailable {
                field: "runtime_admission".into(),
            })
        }
    }
}

fn ensure_current_launch_runtime(launch: &Launch) -> Result<(), TriggerServiceError> {
    match classify_public_launch_runtime(launch)? {
        layer2_core::RuntimeAdmissionRoute::Current => Ok(()),
        layer2_core::RuntimeAdmissionRoute::Historical => {
            Err(TriggerServiceError::RuntimeRouteUnavailable {
                field: "runtime_admission.kernel".into(),
            })
        }
        layer2_core::RuntimeAdmissionRoute::Invalid => {
            Err(TriggerServiceError::RuntimeRouteUnavailable {
                field: "runtime_admission".into(),
            })
        }
    }
}

impl TryFrom<Launch> for LaunchView {
    type Error = TriggerServiceError;

    fn try_from(value: Launch) -> Result<Self, Self::Error> {
        ensure_readable_launch_runtime(&value)?;
        let launch_id = value.id.0.clone();
        let diff = value
            .diff
            .map(|diff| DiffArtifactView::from_artifact(&launch_id, diff));
        let connection = value.connection.map(|connection| {
            TriggerConnectionView::from(TriggerConnection {
                service_id: connection.service_id,
                state_version: connection.service_state_version,
                state: connection.state,
                account: Some(connection.account),
                granted_scopes: connection.granted_scopes,
                consent_observed_at: Some(connection.consent_observed_at),
            })
        });
        Ok(Self {
            id: value.id.0,
            trigger_public_id: value.revision.public_id.0,
            revision_digest: value.revision_digest,
            source_sha: value.source_sha,
            state: enum_name(value.state),
            revision_number: value.revision_number,
            revision_content: value.revision.content,
            blocked_reason: value.blocked_reason,
            failure: value.failure,
            resume_state: value.resume_state.map(enum_name),
            overrides: value.overrides,
            effective: value.effective,
            pricing: value.pricing,
            runtime_admission: value.runtime_admission,
            review_quota: value.review_quota,
            start_admission: value.start_admission,
            connection,
            workflow_paths: value.workflow_paths,
            workflow_risk_acknowledged: value.workflow_risk_acknowledged,
            steps: value.steps.into_iter().map(Into::into).collect(),
            fork: value.fork,
            branch: value.branch,
            workspace: value.workspace.map(Into::into),
            session: value.session,
            session_credential_retirement: value.session_credential_retirement,
            reach: value.reach,
            delivery: value.delivery,
            contribution_checkpoint_version: value.contribution_checkpoint_version,
            diff,
            contribution: value.contribution.map(Into::into),
            pull_request: value.pull_request.map(Into::into),
            created_at: value.created_at,
            updated_at: value.updated_at,
            draft_expires_at: value.draft_expires_at,
            started_at: value.started_at,
            terminal_at: value.terminal_at,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchPageView {
    items: Vec<LaunchView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LaunchProgressView {
    pub(crate) launch_id: String,
    pub(crate) events: Vec<LaunchEventView>,
    pub(crate) next_cursor: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LaunchEventView {
    pub(crate) sequence: u64,
    launch_id: String,
    occurred_at: String,
    correlation_id: String,
    progress: LaunchProgressStateView,
    kind: LaunchEventKindView,
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchProgressStateView {
    #[schema(value_type = trigger_schema::LaunchState)]
    state: String,
    #[schema(value_type = Option<trigger_schema::LaunchBlockedReason>)]
    blocked_reason: Option<LaunchBlockedReason>,
    #[schema(value_type = Option<trigger_schema::LaunchFailure>)]
    failure: Option<LaunchFailure>,
    #[schema(value_type = Option<trigger_schema::LaunchState>)]
    resume_state: Option<String>,
    #[schema(value_type = Option<trigger_schema::ReachStatus>)]
    reach: Option<ReachStatus>,
    #[schema(value_type = Option<trigger_schema::PromptDelivery>)]
    delivery: Option<PromptDelivery>,
}

impl From<LaunchProgressState> for LaunchProgressStateView {
    fn from(value: LaunchProgressState) -> Self {
        Self {
            state: enum_name(value.state),
            blocked_reason: value.blocked_reason,
            failure: value.failure,
            resume_state: value.resume_state.map(enum_name),
            reach: value.reach,
            delivery: value.delivery,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LaunchEventKindView {
    Created {
        steps: Vec<LaunchStepView>,
    },
    DraftUpdated,
    ConnectionSelected,
    Started,
    StateChanged {
        #[schema(value_type = trigger_schema::LaunchState)]
        from: String,
        #[schema(value_type = trigger_schema::LaunchState)]
        to: String,
    },
    StepChanged {
        step: LaunchStepView,
    },
    Blocked {
        #[schema(value_type = trigger_schema::LaunchBlockedReason)]
        reason: LaunchBlockedReason,
    },
    Resumed {
        #[schema(value_type = trigger_schema::LaunchBlockedReason)]
        reason: LaunchBlockedReason,
    },
    Failure {
        #[schema(value_type = trigger_schema::LaunchFailure)]
        failure: LaunchFailure,
    },
    EffectReconciled {
        #[schema(value_type = trigger_schema::LaunchEffectKind)]
        effect: String,
    },
    RecoveryAuthorized {
        #[schema(value_type = trigger_schema::LaunchEffectKind)]
        effect: String,
    },
    DeliveryChanged {
        #[schema(value_type = trigger_schema::PromptDeliveryState)]
        state: String,
    },
    ReachChanged {
        #[schema(value_type = trigger_schema::ReachStatus)]
        reach: ReachStatus,
    },
    PullRequestStatusChanged {
        #[schema(value_type = trigger_schema::PullRequestState)]
        state: String,
        #[schema(value_type = trigger_schema::Mergeability)]
        mergeability: String,
        #[schema(value_type = trigger_schema::CheckSummaryState)]
        checks: String,
        #[schema(value_type = Option<trigger_schema::LaunchFailure>)]
        failure: Option<LaunchFailure>,
    },
    PullRequestStatusRefreshRequested,
    ConfirmationRecorded {
        #[schema(value_type = trigger_schema::ConfirmationAction)]
        action: String,
    },
    CleanupPlanned {
        target_count: usize,
    },
    CleanupCompleted {
        target_count: usize,
    },
    Cancelled,
}

impl From<LaunchEvent> for LaunchEventView {
    fn from(value: LaunchEvent) -> Self {
        Self {
            sequence: value.sequence,
            launch_id: value.launch_id.0,
            occurred_at: value.occurred_at,
            correlation_id: value.correlation_id,
            progress: value.progress.into(),
            kind: match value.kind {
                LaunchEventKind::Created { steps } => LaunchEventKindView::Created {
                    steps: steps.into_iter().map(Into::into).collect(),
                },
                LaunchEventKind::DraftUpdated => LaunchEventKindView::DraftUpdated,
                LaunchEventKind::ConnectionSelected => LaunchEventKindView::ConnectionSelected,
                LaunchEventKind::Started => LaunchEventKindView::Started,
                LaunchEventKind::StateChanged { from, to } => LaunchEventKindView::StateChanged {
                    from: enum_name(from),
                    to: enum_name(to),
                },
                LaunchEventKind::StepChanged { step } => {
                    LaunchEventKindView::StepChanged { step: step.into() }
                }
                LaunchEventKind::Blocked { reason } => LaunchEventKindView::Blocked { reason },
                LaunchEventKind::Resumed { reason } => LaunchEventKindView::Resumed { reason },
                LaunchEventKind::Failure { failure } => LaunchEventKindView::Failure { failure },
                LaunchEventKind::EffectReconciled { effect } => {
                    LaunchEventKindView::EffectReconciled {
                        effect: enum_name(effect),
                    }
                }
                LaunchEventKind::RecoveryAuthorized { effect } => {
                    LaunchEventKindView::RecoveryAuthorized {
                        effect: enum_name(effect),
                    }
                }
                LaunchEventKind::DeliveryChanged { state } => {
                    LaunchEventKindView::DeliveryChanged {
                        state: enum_name(state),
                    }
                }
                LaunchEventKind::ReachChanged { reach } => {
                    LaunchEventKindView::ReachChanged { reach }
                }
                LaunchEventKind::PullRequestStatusChanged {
                    state,
                    mergeability,
                    checks,
                    failure,
                } => LaunchEventKindView::PullRequestStatusChanged {
                    state: enum_name(state),
                    mergeability: enum_name(mergeability),
                    checks: enum_name(checks),
                    failure,
                },
                LaunchEventKind::PullRequestStatusRefreshRequested => {
                    LaunchEventKindView::PullRequestStatusRefreshRequested
                }
                LaunchEventKind::ConfirmationRecorded { action } => {
                    LaunchEventKindView::ConfirmationRecorded {
                        action: enum_name(action),
                    }
                }
                LaunchEventKind::CleanupPlanned { target_count } => {
                    LaunchEventKindView::CleanupPlanned { target_count }
                }
                LaunchEventKind::CleanupCompleted { target_count } => {
                    LaunchEventKindView::CleanupCompleted { target_count }
                }
                LaunchEventKind::Cancelled => LaunchEventKindView::Cancelled,
            },
        }
    }
}

fn enum_name(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn now() -> String {
    layer2_core::rfc3339_at(SystemTime::now())
}

fn correlation_id(headers: &HeaderMap) -> String {
    super::request_id_from_headers(headers)
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

use idempotency::required_command as command;

fn parse_trigger_id(value: String) -> Result<TriggerId, TriggerServiceError> {
    TriggerId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_revision_id(value: String) -> Result<TriggerRevisionId, TriggerServiceError> {
    TriggerRevisionId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_public_id(value: String) -> Result<PublicTriggerId, TriggerServiceError> {
    PublicTriggerId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_launch_id(value: String) -> Result<LaunchId, TriggerServiceError> {
    LaunchId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_diff_artifact_id(value: String) -> Result<DiffArtifactId, TriggerServiceError> {
    DiffArtifactId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

// Conceal foreign and missing private resources before reserving an
// idempotency key or consulting an external authority. Resource ownership is
// immutable, so the command can safely reload the same record for its
// optimistic-concurrency check after this admission read.
async fn owned_trigger_command_id(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    value: String,
) -> Result<TriggerId, TriggerProblem> {
    let id = parse_trigger_id(value)?;
    state.triggers.get_draft(owner, &id).await?;
    Ok(id)
}

async fn owned_revision_command_id(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    value: String,
) -> Result<TriggerRevisionId, TriggerProblem> {
    let id = parse_revision_id(value)?;
    state.triggers.get_revision(owner, &id).await?;
    Ok(id)
}

async fn owned_launch_command_id(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    value: String,
) -> Result<LaunchId, TriggerProblem> {
    let id = parse_launch_id(value)?;
    let launch = state.triggers.get_launch(owner, &id).await?;
    ensure_current_launch_runtime(&launch)?;
    Ok(id)
}

async fn owned_readable_launch_command_id(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    value: String,
) -> Result<LaunchId, TriggerProblem> {
    let id = parse_launch_id(value)?;
    let launch = state.triggers.get_launch(owner, &id).await?;
    ensure_readable_launch_runtime(&launch)?;
    Ok(id)
}

async fn public_launch_resume_command_id(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    value: String,
) -> Result<PublicTriggerId, TriggerProblem> {
    let id = parse_public_id(value)?;
    if let Some(current) = state
        .triggers
        .get_current_launch_for_public_trigger(owner, &id)
        .await?
    {
        ensure_readable_launch_runtime(&current)?;
    }
    Ok(id)
}

async fn owned_connection_command_id(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    service_id: String,
) -> Result<String, TriggerProblem> {
    let owned = state
        .connections
        .connection_for_revocation(owner, &service_id)
        .await?
        .is_some();
    if !owned {
        return Err(TriggerServiceError::NotFound.into());
    }
    Ok(service_id)
}

#[utoipa::path(
    get,
    path = "/api/v1/triggers/public/{public_id}",
    tag = "triggers",
    params(("public_id" = String, Path)),
    responses((status = 200, description = "Side-effect-free public Trigger preview", body = TriggerPreviewView)),
    security(())
)]
async fn public_preview(
    State(state): State<TriggersState>,
    Path(public_id): Path<String>,
) -> Result<Response, TriggerProblem> {
    Ok(public_preview_response(
        load_public_preview(&state, public_id).await?,
    ))
}

pub(crate) fn public_preview_response(preview: TriggerPreviewView) -> Response {
    let mut response = Json(preview).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&format!("public, max-age={PUBLIC_PREVIEW_CACHE_SECONDS}"))
            .expect("static cache policy is valid"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

pub(crate) async fn load_public_preview(
    state: &TriggersState,
    public_id: String,
) -> Result<TriggerPreviewView, TriggerServiceError> {
    Ok(state
        .triggers
        .preview(&parse_public_id(public_id)?)
        .await?
        .into())
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers",
    tag = "triggers",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateTriggerRequest,
    responses(
        (status = 201, description = "Trigger draft created", body = TriggerDraftView),
        (status = 422, description = "Trigger policy validation with safe remediation", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn create_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<CreateTriggerRequest>, BodyProblem>,
) -> Response {
    let (guard, body, _) = match command(
        key,
        Arc::clone(&state.operations),
        &caller,
        "trigger-create".into(),
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let content = body
            .content
            .resolve(
                state.connections.as_ref(),
                &caller.subject,
                &body.selected_service_id,
            )
            .await?;
        let draft = state
            .triggers
            .create_draft(
                caller.subject.clone(),
                body.selected_service_id,
                content,
                now(),
            )
            .await?;
        Ok((StatusCode::CREATED, Json(TriggerDraftView::from(draft))).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/triggers",
    tag = "triggers",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Publisher Trigger drafts", body = TriggerDraftPageView))
)]
async fn list_triggers(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<TriggerDraftPageView>, RequestFailure> {
    let Query(params) = params?;
    let page = state
        .triggers
        .list_drafts(
            &caller.subject,
            &TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(TriggerDraftPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/triggers/{trigger_id}",
    tag = "triggers",
    params(("trigger_id" = String, Path)),
    responses((status = 200, description = "Publisher Trigger draft", body = TriggerDraftView))
)]
async fn read_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
) -> Result<Json<TriggerDraftView>, TriggerProblem> {
    Ok(Json(
        state
            .triggers
            .get_draft(&caller.subject, &parse_trigger_id(trigger_id)?)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/triggers/{trigger_id}",
    tag = "triggers",
    params(("trigger_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateTriggerRequest,
    responses(
        (status = 200, description = "Trigger draft replaced", body = TriggerDraftView),
        (status = 422, description = "Trigger policy validation with safe remediation", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn update_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<UpdateTriggerRequest>, BodyProblem>,
) -> Response {
    let trigger_id = match owned_trigger_command_id(&state, &caller.subject, trigger_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-update:{}", trigger_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let content = body
            .content
            .resolve(
                state.connections.as_ref(),
                &caller.subject,
                &body.selected_service_id,
            )
            .await?;
        let draft = state
            .triggers
            .update_draft(
                &caller.subject,
                &trigger_id,
                body.expected_version,
                body.selected_service_id,
                content,
                now(),
            )
            .await?;
        Ok(Json(TriggerDraftView::from(draft)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/{trigger_id}/publish",
    tag = "triggers",
    params(("trigger_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = PublishTriggerRequest,
    responses(
        (status = 201, description = "Immutable Trigger revision published", body = TriggerRevisionView),
        (status = 422, description = "Trigger policy validation with safe remediation", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn publish_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<PublishTriggerRequest>, BodyProblem>,
) -> Response {
    let trigger_id = match owned_trigger_command_id(&state, &caller.subject, trigger_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-publish:{}", trigger_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let published = state
            .triggers
            .publish(
                &caller.subject,
                &trigger_id,
                body.expected_version,
                &body.selected_service_id,
                now(),
            )
            .await?;
        Ok((
            StatusCode::CREATED,
            Json(TriggerRevisionView::from(published.revision)),
        )
            .into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-revisions",
    tag = "triggers",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Publisher immutable revisions", body = TriggerRevisionPageView))
)]
async fn list_revisions(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<TriggerRevisionPageView>, RequestFailure> {
    let Query(params) = params?;
    let page = state
        .triggers
        .list_revisions(
            &caller.subject,
            &TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(TriggerRevisionPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-revisions/{revision_id}",
    tag = "triggers",
    params(("revision_id" = String, Path)),
    responses((status = 200, description = "Publisher immutable revision", body = TriggerRevisionView))
)]
async fn read_revision(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(revision_id): Path<String>,
) -> Result<Json<TriggerRevisionView>, TriggerProblem> {
    Ok(Json(
        state
            .triggers
            .get_revision(&caller.subject, &parse_revision_id(revision_id)?)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/trigger-revisions/{revision_id}/retire",
    tag = "triggers",
    params(("revision_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 200, description = "Revision retired", body = TriggerRevisionView))
)]
async fn retire_revision(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(revision_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let revision_id = match owned_revision_command_id(&state, &caller.subject, revision_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-retire:{}", revision_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        body.map_err(RequestFailure::from)?;
        let revision = state
            .triggers
            .retire(&caller.subject, &revision_id, &now())
            .await?;
        Ok(Json(TriggerRevisionView::from(revision)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-connections",
    tag = "trigger-connections",
    responses((status = 200, description = "Reusable GitHub connections", body = TriggerConnectionsView))
)]
async fn list_connections(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<TriggerConnectionsView>, TriggerProblem> {
    Ok(Json(TriggerConnectionsView {
        connections: state
            .connections
            .list_connections(&caller.subject)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/trigger-connections",
    tag = "trigger-connections",
    params(("Idempotency-Key" = String, Header)),
    request_body = BeginConnectionRequest,
    responses((status = 200, description = "GitHub OAuth handoff", body = ConnectionAuthorizationView))
)]
async fn begin_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<BeginConnectionRequest>, BodyProblem>,
) -> Response {
    let (guard, body, _) = match command(
        key,
        Arc::clone(&state.operations),
        &caller,
        "trigger-connection-create".into(),
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let authorization = state
            .connections
            .begin_connection(&caller.subject, &body.intent_id)
            .await?;
        Ok(Json(ConnectionAuthorizationView::from(authorization)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    delete,
    path = "/api/v1/trigger-connections/{service_id}",
    tag = "trigger-connections",
    params(("service_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 200, description = "GitHub connection revoked; retained Launch resources are preserved", body = TriggerConnectionView))
)]
async fn revoke_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(service_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let service_id = match owned_connection_command_id(&state, &caller.subject, service_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-connection-revoke:{service_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        body.map_err(RequestFailure::from)?;
        let revoked = state
            .connections
            .revoke_connection(&caller.subject, &service_id)
            .await?;
        if revoked.service_id != service_id {
            return Err(RequestFailure::from(TriggerServiceError::from(
                layer2_core::TriggerAuthorityError::AuthorityDrift,
            )));
        }
        Ok(Json(TriggerConnectionView::from(revoked)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/trigger-connections/{service_id}/reconnect",
    tag = "trigger-connections",
    params(("service_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 200, description = "GitHub OAuth reconnection handoff", body = ConnectionAuthorizationView))
)]
async fn reconnect_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(service_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-connection-reconnect:{service_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        body.map_err(RequestFailure::from)?;
        let authorization = state
            .connections
            .reconnect(&caller.subject, &service_id)
            .await?;
        Ok(Json(ConnectionAuthorizationView::from(authorization)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-connections/{service_id}/preflight",
    tag = "trigger-connections",
    params(("service_id" = String, Path)),
    responses((status = 200, description = "Current connection preflight", body = TriggerConnectionView))
)]
async fn preflight_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(service_id): Path<String>,
) -> Result<Json<TriggerConnectionView>, TriggerProblem> {
    Ok(Json(
        state
            .connections
            .preflight_connection(&caller.subject, &service_id)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-connections/{service_id}/repositories/resolve",
    tag = "trigger-connections",
    params(
        ("service_id" = String, Path),
        ("owner" = String, Query),
        ("name" = String, Query)
    ),
    responses((status = 200, description = "Canonical repository identity resolved through the selected connection", body = TriggerRepositorySelectionView))
)]
async fn resolve_connection_repository(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(service_id): Path<String>,
    params: Result<Query<RepositoryLocatorParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<TriggerRepositorySelectionView>, RequestFailure> {
    let Query(params) = params?;
    Ok(Json(
        state
            .connections
            .resolve_repository_locator(&caller.subject, &service_id, &params.owner, &params.name)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/public/{public_id}/launches",
    tag = "launches",
    params(("public_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 201, description = "Current Launch created or resumed", body = LaunchView))
)]
async fn create_or_resume_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(public_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let public_id = match public_launch_resume_command_id(&state, &caller.subject, public_id).await
    {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-create:{}", public_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        caller
            .authorization
            .require(layer2_core::Capability::CreateSession)?;
        body.map_err(RequestFailure::from)?;
        let legacy_launch = match state
            .triggers
            .get_current_launch_for_public_trigger(&caller.subject, &public_id)
            .await?
        {
            Some(current) => match classify_public_launch_runtime(&current)? {
                layer2_core::RuntimeAdmissionRoute::Current => None,
                layer2_core::RuntimeAdmissionRoute::Historical => Some(current),
                layer2_core::RuntimeAdmissionRoute::Invalid => {
                    return Err(TriggerServiceError::RuntimeRouteUnavailable {
                        field: "runtime_admission".into(),
                    }
                    .into());
                }
            },
            None => None,
        };
        // A normal README re-click returns the exact current historical
        // Launch, even when its old draft deadline elapsed. Replacement and
        // fresh admission require the explicit New Launch operation.
        let launch = if let Some(legacy) = legacy_launch {
            legacy
        } else {
            state
                .triggers
                .create_or_resume_launch(
                    caller.subject.clone(),
                    &public_id,
                    now(),
                    correlation_id(&headers),
                )
                .await?
        };
        Ok((StatusCode::CREATED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/public/{public_id}/launches/new",
    tag = "launches",
    params(("public_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = NewLaunchRequest,
    responses((status = 201, description = "Separate Launch created", body = LaunchView))
)]
async fn create_new_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(public_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<NewLaunchRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-new:{public_id}");
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        caller
            .authorization
            .require(layer2_core::Capability::CreateSession)?;
        let body = body.map_err(RequestFailure::from)?;
        if !body.acknowledge_additional_resources {
            return Err(TriggerServiceError::Validation(
                layer2_core::TriggerValidationError::field(
                    "acknowledge_additional_resources",
                    "must be accepted before creating a separate Launch",
                ),
            )
            .into());
        }
        let launch = state
            .triggers
            .create_new_launch(
                caller.subject.clone(),
                &parse_public_id(public_id)?,
                confirmation_hash,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::CREATED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches",
    tag = "launches",
    params(
        ("state" = Option<String>, Query),
        ("trigger_public_id" = Option<String>, Query),
        ("session_id" = Option<String>, Query),
        ("limit" = Option<u32>, Query),
        ("cursor" = Option<String>, Query)
    ),
    responses((status = 200, description = "Contributor Launches", body = LaunchPageView))
)]
async fn list_launches(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<LaunchPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchPageView>, RequestFailure> {
    let Query(params) = params?;
    let public_id = params.trigger_public_id.map(parse_public_id).transpose()?;
    let page = state
        .triggers
        .list_launches(
            &caller.subject,
            public_id,
            params.session_id.map(SessionId),
            params.state.into_iter().collect(),
            TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(LaunchPageView {
        // Exact legacy Launches remain visible for owner inspection and stop;
        // malformed or unknown routes stay quarantined from this inventory.
        items: page
            .items
            .into_iter()
            .filter_map(|launch| LaunchView::try_from(launch).ok())
            .collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}",
    tag = "launches",
    params(("launch_id" = String, Path)),
    responses((status = 200, description = "Launch review and current state", body = LaunchView))
)]
async fn read_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<LaunchView>, TriggerProblem> {
    Ok(Json(load_launch(&state, &caller.subject, launch_id).await?))
}

pub(crate) async fn load_launch(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    launch_id: String,
) -> Result<LaunchView, TriggerServiceError> {
    LaunchView::try_from(
        state
            .triggers
            .get_launch(owner, &parse_launch_id(launch_id)?)
            .await?,
    )
}

#[utoipa::path(
    patch,
    path = "/api/v1/launches/{launch_id}/overrides",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateOverridesRequest,
    responses((status = 200, description = "Launch overrides applied", body = LaunchView))
)]
async fn update_launch_overrides(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<UpdateOverridesRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-overrides:{}", launch_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let launch = state
            .triggers
            .update_overrides(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                body.overrides,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::try_from(launch)?).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    put,
    path = "/api/v1/launches/{launch_id}/connection",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = SelectConnectionRequest,
    responses((status = 200, description = "Exact connection selected", body = LaunchView))
)]
async fn select_launch_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<SelectConnectionRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-connection:{}", launch_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let launch = state
            .triggers
            .select_connection(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                &body.service_id,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::try_from(launch)?).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/start",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = StartLaunchRequest,
    responses((status = 202, description = "Launch orchestration accepted", body = LaunchView))
)]
async fn start_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<StartLaunchRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-start:{}", launch_id.0);
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        caller
            .authorization
            .require(layer2_core::Capability::CreateSession)?;
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .start(
                &caller.subject,
                &launch_id,
                StartLaunchCommand {
                    expected_revision: body.expected_revision,
                    confirmation: Confirmation {
                        action: ConfirmationAction::Start,
                        subject: caller.subject.clone(),
                        snapshot_digest: body.snapshot_digest,
                        confirmed_at: timestamp.clone(),
                        idempotency_key_hash: confirmation_hash,
                    },
                    acknowledge_workflow_risk: body.acknowledge_workflow_risk,
                    now: timestamp,
                    correlation_id: correlation_id(&headers),
                },
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/contributions/review",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ConfirmLaunchRequest,
    responses((status = 202, description = "Read-only contribution review accepted", body = LaunchView))
)]
async fn review_launch_diff(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmLaunchRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-diff-review:{}", launch_id.0);
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .request_diff_review(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                Confirmation {
                    action: ConfirmationAction::ReviewDiff,
                    subject: caller.subject.clone(),
                    snapshot_digest: body.snapshot_digest,
                    confirmed_at: timestamp.clone(),
                    idempotency_key_hash: confirmation_hash,
                },
                timestamp,
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}/diff",
    tag = "launches",
    params(("launch_id" = String, Path)),
    responses((status = 200, description = "Sealed contribution review and pull-request status", body = DiffReviewView))
)]
async fn read_launch_diff(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<DiffReviewView>, TriggerProblem> {
    Ok(Json(
        load_diff_review(&state, &caller.subject, launch_id).await?,
    ))
}

pub(crate) async fn load_diff_review(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    launch_id: String,
) -> Result<DiffReviewView, TriggerServiceError> {
    let launch = state
        .triggers
        .get_launch(owner, &parse_launch_id(launch_id)?)
        .await?;
    ensure_readable_launch_runtime(&launch)?;
    Ok(DiffReviewView::from_launch(launch))
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/contributions/publish",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ConfirmLaunchRequest,
    responses((status = 202, description = "Explicit guarded branch publication accepted", body = LaunchView))
)]
async fn publish_launch_branch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmLaunchRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-branch-publication:{}", launch_id.0);
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .confirm_branch_publication(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                Confirmation {
                    action: ConfirmationAction::PublishBranch,
                    subject: caller.subject.clone(),
                    snapshot_digest: body.snapshot_digest,
                    confirmed_at: timestamp.clone(),
                    idempotency_key_hash: confirmation_hash,
                },
                timestamp,
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/pull-request/review",
    tag = "launches",
    request_body = PullRequestReviewRequest,
    params(("launch_id" = String, Path)),
    responses((status = 200, description = "Read-only sealed Open or Update pull-request draft", body = PullRequestDraftView))
)]
async fn review_pull_request(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    body: Result<idempotency::RequestBody<PullRequestReviewRequest>, BodyProblem>,
) -> Result<Json<PullRequestDraftView>, RequestFailure> {
    let launch_id = parse_launch_id(launch_id)?;
    let launch = state
        .triggers
        .get_launch(&caller.subject, &launch_id)
        .await?;
    ensure_current_launch_runtime(&launch)?;
    let (body, _) = body?.into_parts();
    let body = body.map_err(RequestFailure::from)?;
    let draft = launch
        .pull_request_draft(
            body.operation.into(),
            body.title.as_deref(),
            body.body.as_deref(),
        )
        .map_err(TriggerServiceError::from)?;
    Ok(Json(draft.into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/pull-request/open",
    tag = "launches",
    request_body = ConfirmPullRequestRequest,
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, description = "Explicit Open pull-request confirmation accepted", body = LaunchView))
)]
async fn open_pull_request(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmPullRequestRequest>, BodyProblem>,
) -> Response {
    confirm_pull_request(
        state,
        caller,
        launch_id,
        headers,
        key,
        body,
        PullRequestOperation::Open,
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/pull-request/update",
    tag = "launches",
    request_body = ConfirmPullRequestRequest,
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, description = "Explicit Update pull-request confirmation accepted", body = LaunchView))
)]
async fn update_pull_request(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmPullRequestRequest>, BodyProblem>,
) -> Response {
    confirm_pull_request(
        state,
        caller,
        launch_id,
        headers,
        key,
        body,
        PullRequestOperation::Update,
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/pull-request/refresh",
    tag = "launches",
    request_body = RefreshPullRequestStatusRequest,
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 202, description = "Read-only pull-request status refresh queued", body = LaunchView))
)]
async fn refresh_pull_request_status(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<RefreshPullRequestStatusRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let claim = format!("trigger-launch-pull-request-refresh:{}", launch_id.0);
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, claim, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let launch = state
            .triggers
            .request_pull_request_status_refresh(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[allow(clippy::too_many_arguments)]
async fn confirm_pull_request(
    state: TriggersState,
    caller: layer2_core::Caller,
    launch_id: String,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmPullRequestRequest>, BodyProblem>,
    operation: PullRequestOperation,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation_name = enum_name(operation);
    let claim = format!(
        "trigger-launch-pull-request-{operation_name}:{}",
        launch_id.0
    );
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, claim, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .confirm_pull_request(
                &caller.subject,
                &launch_id,
                ConfirmPullRequestCommand {
                    expected_revision: body.expected_revision,
                    confirmation: Confirmation {
                        action: operation.confirmation_action(),
                        subject: caller.subject.clone(),
                        snapshot_digest: body.snapshot_digest,
                        confirmed_at: timestamp.clone(),
                        idempotency_key_hash: confirmation_hash,
                    },
                    title: body.title,
                    body: body.body,
                    now: timestamp,
                    correlation_id: correlation_id(&headers),
                },
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}/diff-artifacts/{artifact_id}/download",
    tag = "launches",
    params(("launch_id" = String, Path), ("artifact_id" = String, Path)),
    responses(
        (status = 200, description = "Tenant-isolated immutable diff artifact", content_type = "application/octet-stream"),
        (status = 404, description = "Launch or artifact not found")
    )
)]
async fn download_diff_artifact(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((launch_id, artifact_id)): Path<(String, String)>,
) -> Result<Response, TriggerProblem> {
    let launch_id = parse_launch_id(launch_id)?;
    let artifact_id = parse_diff_artifact_id(artifact_id)?;
    let launch = state
        .triggers
        .get_launch(&caller.subject, &launch_id)
        .await?;
    ensure_readable_launch_runtime(&launch)?;
    let artifact = launch
        .diff
        .as_ref()
        .filter(|artifact| artifact.id == artifact_id)
        .ok_or(TriggerServiceError::NotFound)?;
    artifact.validate_for_review(&launch).map_err(|error| {
        TriggerServiceError::Store(TriggerStoreError::Malformed(format!(
            "retained diff review: {error}"
        )))
    })?;
    let store = state.diff_artifacts.as_ref().ok_or_else(|| {
        TriggerServiceError::Store(TriggerStoreError::Unavailable(
            "diff artifact storage is not configured".into(),
        ))
    })?;
    let bytes = store
        .get_diff_artifact(&caller.subject, artifact)
        .await
        .map_err(TriggerServiceError::Store)?
        .ok_or(TriggerServiceError::NotFound)?;
    let retained_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != artifact.object_bytes
        || retained_sha256 != artifact.object_sha256
    {
        return Err(TriggerServiceError::Store(TriggerStoreError::Malformed(
            "diff artifact bytes failed their retained size or SHA-256".into(),
        ))
        .into());
    }

    let filename = format!("cma-trigger-diff-{}.json", artifact.id.0);
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .expect("a parsed artifact ID always produces a valid attachment header");
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_DISPOSITION, disposition);
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/cancel",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ConfirmLaunchRequest,
    responses((status = 200, description = "Launch cancellation recorded", body = LaunchView))
)]
async fn cancel_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmLaunchRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_readable_launch_command_id(&state, &caller.subject, launch_id).await
    {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-cancel:{}", launch_id.0);
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .cancel(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                Confirmation {
                    action: ConfirmationAction::Cancel,
                    subject: caller.subject.clone(),
                    snapshot_digest: body.snapshot_digest,
                    confirmed_at: timestamp.clone(),
                    idempotency_key_hash: confirmation_hash,
                },
                timestamp,
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::try_from(launch)?).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/complete",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ConfirmLaunchRequest,
    responses((status = 200, description = "Open pull-request handoff completed", body = LaunchView))
)]
async fn complete_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmLaunchRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-complete:{}", launch_id.0);
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .complete(
                &caller.subject,
                &launch_id,
                body.expected_revision,
                Confirmation {
                    action: ConfirmationAction::Complete,
                    subject: caller.subject.clone(),
                    snapshot_digest: body.snapshot_digest,
                    confirmed_at: timestamp.clone(),
                    idempotency_key_hash: confirmation_hash,
                },
                timestamp,
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::try_from(launch)?).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/recover-initial-prompt",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = RecoverInitialPromptRequest,
    responses((status = 202, description = "One replacement initial-prompt generation explicitly authorized", body = LaunchView))
)]
async fn recover_initial_prompt(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<RecoverInitialPromptRequest>, BodyProblem>,
) -> Response {
    let launch_id = match owned_launch_command_id(&state, &caller.subject, launch_id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let operation = format!("trigger-launch-prompt-recovery:{}", launch_id.0);
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .recover_initial_prompt(
                &caller.subject,
                &launch_id,
                RecoverInitialPromptCommand {
                    expected_revision: body.expected_revision,
                    confirmation: Confirmation {
                        action: ConfirmationAction::RecoverPrompt,
                        subject: caller.subject.clone(),
                        snapshot_digest: body.snapshot_digest,
                        confirmed_at: timestamp.clone(),
                        idempotency_key_hash: confirmation_hash,
                    },
                    acknowledge_duplicate_execution_risk: body.acknowledge_duplicate_execution_risk,
                    now: timestamp,
                    correlation_id: correlation_id(&headers),
                },
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::try_from(launch)?)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}/events",
    tag = "launches",
    params(("launch_id" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<usize>, Query)),
    responses((status = 200, description = "Replayable Launch progress", body = LaunchProgressView))
)]
async fn launch_progress(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    params: Result<Query<ProgressParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchProgressView>, RequestFailure> {
    let Query(params) = params?;
    Ok(Json(
        load_launch_progress(
            &state,
            &caller.subject,
            launch_id,
            params.after,
            params.limit,
        )
        .await?,
    ))
}

pub(crate) async fn load_launch_progress(
    state: &TriggersState,
    owner: &layer2_core::SubjectId,
    launch_id: String,
    after: u64,
    limit: usize,
) -> Result<LaunchProgressView, TriggerServiceError> {
    let launch_id = parse_launch_id(launch_id)?;
    let launch = state.triggers.get_launch(owner, &launch_id).await?;
    ensure_readable_launch_runtime(&launch)?;
    let events = state
        .triggers
        .progress(owner, &launch_id, after, limit)
        .await?;
    let next_cursor = events.last().map_or(after, |event| event.sequence);
    Ok(LaunchProgressView {
        launch_id: launch_id.0,
        events: events.into_iter().map(Into::into).collect(),
        next_cursor,
    })
}

/// Trigger-domain failure projected without leaking provider bodies or stored
/// persistence diagnostics.
pub(crate) struct TriggerProblem(pub TriggerServiceError);

struct TriggerValidationGuidance {
    category: TriggerValidationCategory,
    remediation: &'static str,
}

fn trigger_validation_guidance(field: Option<&str>) -> TriggerValidationGuidance {
    let field = field.unwrap_or_default();
    if matches!(
        field,
        "upstream.repository_id"
            | "upstream.repository_node_id"
            | "upstream.owner_node_id"
            | "upstream.canonical_owner"
            | "upstream.canonical_name"
    ) {
        return TriggerValidationGuidance {
            category: TriggerValidationCategory::RepositoryIdentity,
            remediation: "Resolve the repository again through the selected GitHub connection and submit the canonical numeric and node identities CMA returns.",
        };
    }
    if field == "upstream" {
        return TriggerValidationGuidance {
            category: TriggerValidationCategory::RepositoryPolicy,
            remediation: "Select a public source repository whose current policy permits forking, then resolve it again through the selected GitHub connection.",
        };
    }
    if matches!(
        field,
        "upstream.default_branch"
            | "upstream.selected_ref"
            | "upstream.contribution_base_ref"
            | "upstream.observed_source_sha"
            | "contribution.branch_prefix"
    ) {
        return TriggerValidationGuidance {
            category: TriggerValidationCategory::RepositoryRef,
            remediation: "Resolve the repository again and select a safe existing ref and contribution base; contribution branches must use CMA's server-owned prefix.",
        };
    }
    if field == "session.required_capabilities"
        || field == "session.sandbox_profile"
        || field.starts_with("repository_access.")
    {
        return TriggerValidationGuidance {
            category: TriggerValidationCategory::ScopeCapability,
            remediation: "Use the contributor connection route, the public-repository scope class, allowed public operation classes, and capabilities supported by the Trigger runtime.",
        };
    }
    if field == "commercial_policy" || field.starts_with("commercial_policy.") {
        return TriggerValidationGuidance {
            category: TriggerValidationCategory::CommercialPolicy,
            remediation: "Choose a supported billing mode and its matching deployment-owned pricing policy; sponsored policy is unavailable in the public MVP.",
        };
    }
    TriggerValidationGuidance {
        category: TriggerValidationCategory::TriggerPolicy,
        remediation: "Review the named field against the CMA Trigger request contract and submit a newly validated request.",
    }
}

impl From<TriggerServiceError> for TriggerProblem {
    fn from(value: TriggerServiceError) -> Self {
        Self(value)
    }
}

impl IntoResponse for TriggerProblem {
    fn into_response(self) -> Response {
        let error = self.0;
        let rollout_reason = match &error {
            TriggerServiceError::Rollout(denial) => Some(denial.code()),
            _ => None,
        };
        let admission_capability = match &error {
            TriggerServiceError::Admission(
                layer2_core::TriggerAdmissionError::CapabilityUnsupported { capability },
            ) => Some(capability.clone()),
            _ => None,
        };
        let mut retry_after = None;
        let mut admission_remediation = None;
        let mut validation_category = None;
        let (status, slug, code, detail, field): (StatusCode, &str, &str, String, Option<String>) =
            match error {
                TriggerServiceError::CursorBeyondEnd { cursor, head } => {
                    let detail = format!(
                        "The Launch progress cursor {cursor} is beyond the durable event head {head}; resume from {head} or earlier."
                    );
                    let mut body = problem_document(
                        StatusCode::BAD_REQUEST,
                        "cursor-beyond-end",
                        &detail,
                        "cursor_beyond_end",
                    );
                    body.as_object_mut()
                        .expect("a problem document is an object")
                        .insert("head".into(), head.into());
                    return problem_response(StatusCode::BAD_REQUEST, body);
                }
                TriggerServiceError::NotFound | TriggerServiceError::Forbidden => (
                    StatusCode::NOT_FOUND,
                    "trigger-resource-not-found",
                    "trigger_resource_not_found",
                    "The requested Trigger resource does not exist.".into(),
                    None,
                ),
                TriggerServiceError::RuntimeRouteUnavailable { field } => {
                    admission_remediation = Some(
                        "Open a New Launch from the published Trigger. The retained Launch remains fenced from fresh work; an exact legacy route remains available for inspection, cancellation, and cleanup.",
                    );
                    (
                        StatusCode::CONFLICT,
                        "trigger-runtime-route-unavailable",
                        "trigger_runtime_route_unavailable",
                        "This retained Launch cannot run on the current Trigger runtime route."
                            .into(),
                        Some(field),
                    )
                }
                TriggerServiceError::Validation(error)
                    if error.field.as_deref() == Some("draft_expires_at") =>
                {
                    admission_remediation = Some(
                        "Open the README link again to create a New Launch with current admission evidence.",
                    );
                    (
                        StatusCode::CONFLICT,
                        "trigger-draft-expired",
                        "trigger_draft_expired",
                        "This Launch draft expired before Start.".into(),
                        error.field,
                    )
                }
                TriggerServiceError::Validation(error) => {
                    let guidance = trigger_validation_guidance(error.field.as_deref());
                    validation_category = Some(guidance.category);
                    admission_remediation = Some(guidance.remediation);
                    (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "invalid-trigger-request",
                        "invalid_trigger_request",
                        "The Trigger request violates the declared policy.".into(),
                        error.field,
                    )
                }
                TriggerServiceError::Authority(error) => {
                    use layer2_core::TriggerAuthorityError as Authority;
                    let (status, slug, code) = match &error {
                        Authority::OwnerCredentialUnavailable => (
                            StatusCode::UNAUTHORIZED,
                            "trigger-owner-credential-unavailable",
                            "trigger_owner_credential_unavailable",
                        ),
                        Authority::ConnectionNotFound => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "trigger-connection-not-found",
                            "trigger_connection_not_found",
                        ),
                        Authority::ConnectionNotPersonal => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "trigger-personal-connection-required",
                            "trigger_personal_connection_required",
                        ),
                        Authority::ConnectionInactive => (
                            StatusCode::CONFLICT,
                            "trigger-connection-inactive",
                            "trigger_connection_inactive",
                        ),
                        Authority::ReauthorizationRequired => (
                            StatusCode::CONFLICT,
                            "trigger-reauthorization-required",
                            "trigger_reauthorization_required",
                        ),
                        Authority::InsufficientScope { .. } => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "trigger-scope-insufficient",
                            "trigger_scope_insufficient",
                        ),
                        Authority::UnapprovedScope { .. } => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "trigger-scope-unapproved",
                            "trigger_scope_unapproved",
                        ),
                        Authority::AuthorityDrift => (
                            StatusCode::CONFLICT,
                            "trigger-authority-drift",
                            "trigger_authority_drift",
                        ),
                        Authority::Forbidden | Authority::ResourceNotFound => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "trigger-provider-resource-unavailable",
                            "trigger_provider_resource_unavailable",
                        ),
                        Authority::RateLimited { retry_after: value } => {
                            retry_after = value.clone();
                            (
                                StatusCode::TOO_MANY_REQUESTS,
                                "trigger-provider-rate-limited",
                                "trigger_provider_rate_limited",
                            )
                        }
                        Authority::Transient => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "trigger-provider-unavailable",
                            "trigger_provider_unavailable",
                        ),
                        Authority::ProviderRejected | Authority::MalformedEvidence => (
                            StatusCode::BAD_GATEWAY,
                            "trigger-provider-response-invalid",
                            "trigger_provider_response_invalid",
                        ),
                        Authority::UpstreamNotForkable => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "upstream-not-forkable",
                            "upstream_not_forkable",
                        ),
                        Authority::ForkNetworkOccupied => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "fork-network-occupied",
                            "fork_network_occupied",
                        ),
                        Authority::ProviderAuthenticationUnavailable => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "trigger-provider-authentication-unavailable",
                            "trigger_provider_authentication_unavailable",
                        ),
                    };
                    (status, slug, code, error.to_string(), None)
                }
                TriggerServiceError::Admission(error) => {
                    use layer2_core::TriggerAdmissionError as Admission;
                    let (status, slug, code, detail, field, remediation) = match &error {
                        Admission::Unavailable(_) => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "trigger-admission-unavailable",
                            "trigger_admission_unavailable",
                            "CMA could not verify Launch admission safely.".into(),
                            None,
                            "Retry later. No fork, sandbox, or session work was admitted.",
                        ),
                        Admission::PricingUnavailable(_) => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "trigger-pricing-unavailable",
                            "trigger_pricing_unavailable",
                            "CMA could not produce a valid server-owned price for this Launch."
                                .into(),
                            Some("pricing".into()),
                            "Retry after deployment pricing is available.",
                        ),
                        Admission::PriceChanged => (
                            StatusCode::CONFLICT,
                            "trigger-price-changed",
                            "trigger_price_changed",
                            error.to_string(),
                            Some("pricing".into()),
                            "Refresh the Launch review and explicitly accept the current price before Start.",
                        ),
                        Admission::QuoteExpired => (
                            StatusCode::CONFLICT,
                            "trigger-quote-expired",
                            "trigger_quote_expired",
                            error.to_string(),
                            Some("pricing.expires_at".into()),
                            "Open a New Launch to review current pricing, quota, and runtime evidence.",
                        ),
                        Admission::SponsoredTermsNotAccepted => (
                            StatusCode::CONFLICT,
                            "trigger-sponsored-terms-not-accepted",
                            "trigger_sponsored_terms_not_accepted",
                            error.to_string(),
                            Some("sponsorship.terms".into()),
                            "Review and explicitly accept the current sponsored scope, budget, and expiry before Start.",
                        ),
                        Admission::QuotaExceeded(_) => (
                            StatusCode::CONFLICT,
                            "trigger-quota-exhausted",
                            "trigger_quota_exhausted",
                            error.to_string(),
                            Some("review_quota".into()),
                            "Release session capacity or wait for deployment capacity, then refresh the review.",
                        ),
                        Admission::CapabilityUnsupported { capability } => (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "trigger-capability-unsupported",
                            "trigger_capability_unsupported",
                            error.to_string(),
                            Some(format!("required_capabilities.{capability}")),
                            "Use a Trigger and deployed runtime that support every published capability.",
                        ),
                        Admission::RuntimeChanged => (
                            StatusCode::CONFLICT,
                            "trigger-runtime-changed",
                            "trigger_runtime_changed",
                            error.to_string(),
                            Some("runtime_admission.runtime_version".into()),
                            "Refresh the Launch review and explicitly accept the current runtime before Start.",
                        ),
                    };
                    admission_remediation = Some(remediation);
                    (status, slug, code, detail, field)
                }
                TriggerServiceError::Rollout(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "trigger-feature-paused",
                    "trigger_feature_paused",
                    "GitHub Trigger orchestration is currently unavailable for this request."
                        .into(),
                    None,
                ),
                TriggerServiceError::Store(error) => match error {
                    TriggerStoreError::Unavailable(_) | TriggerStoreError::Malformed(_) => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "trigger-store-unavailable",
                        "trigger_store_unavailable",
                        "Trigger state is temporarily unavailable.".into(),
                        None,
                    ),
                    TriggerStoreError::AlreadyExists
                    | TriggerStoreError::RevisionConflict { .. }
                    | TriggerStoreError::StaleFence { .. }
                    | TriggerStoreError::LeaseHeld
                    | TriggerStoreError::Superseded => (
                        StatusCode::CONFLICT,
                        "trigger-state-conflict",
                        "trigger_state_conflict",
                        "The Trigger resource changed; read its current state before retrying."
                            .into(),
                        None,
                    ),
                },
            };
        let mut body = problem_document(status, slug, &detail, code);
        if let (Some(field), Some(object)) = (field, body.as_object_mut()) {
            object.insert("field".into(), field.into());
        }
        if let (Some(reason), Some(object)) = (rollout_reason, body.as_object_mut()) {
            object.insert("rollout_reason".into(), reason.into());
        }
        if let (Some(remediation), Some(object)) = (admission_remediation, body.as_object_mut()) {
            object.insert("remediation".into(), remediation.into());
        }
        if let (Some(category), Some(object)) = (validation_category, body.as_object_mut()) {
            object.insert(
                "validation_category".into(),
                serde_json::to_value(category).expect("a validation category is serializable"),
            );
        }
        if let (Some(capability), Some(object)) = (admission_capability, body.as_object_mut()) {
            object.insert("capability".into(), capability.into());
        }
        let mut response = problem_response(status, body);
        if let Some(value) = retry_after.and_then(|value| HeaderValue::from_str(&value).ok()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

// Keep the closed MVP vocabulary named in this interface module. These
// compile-time references make removing one from the core an intentional API
// change rather than an unnoticed schema narrowing.
#[allow(dead_code)]
fn closed_contract_vocabulary() {
    let _ = [
        BillingMode::Contributor,
        BillingMode::ZeroCost,
        BillingMode::Sponsored,
    ];
    let _ = [
        CredentialRoute::ContributorConnection,
        CredentialRoute::SponsoredGrant,
    ];
    let _ = [ExecutorPolicy::Disabled];
    let _ = [
        WorkflowChangePolicy::Refuse,
        WorkflowChangePolicy::RequireScopeAndConfirmation,
    ];
    let _ = [ScopeClass::PublicRepository, ScopeClass::Workflow];
    let _ = [
        EditableField::TaskInput,
        EditableField::WorkLabel,
        EditableField::ResourceSize,
        EditableField::Timeout,
        EditableField::OptionalChecks,
    ];
    let _ = [
        RequiredCapability::Conversation,
        RequiredCapability::DurableConversation,
        RequiredCapability::WorkspaceCheckpoint,
        RequiredCapability::RestrictedServiceGateway,
        RequiredCapability::CredentialRefresh,
        RequiredCapability::CredentialRevocation,
    ];
    let _ = TriggerAvailability::Published;
    let _ = ResourcePolicy {
        default: layer2_core::SessionSize::new(250, 512).unwrap(),
        ceiling: layer2_core::SessionSize::new(250, 512).unwrap(),
    };
    let _ = InputFieldDefinition {
        key: String::new(),
        kind: layer2_core::InputFieldKind::Boolean,
        required: false,
        default: None,
        editable: false,
    };
}

#[cfg(test)]
#[path = "triggers_tests.rs"]
mod tests;
