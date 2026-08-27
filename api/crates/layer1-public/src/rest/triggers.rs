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
    LaunchStep, OverridePolicy, PricingSnapshot, PromptDelivery, PublicProjectionPolicy,
    PublicTriggerId, PullRequestDraft, PullRequestOperation, PullRequestRecord, ReachStatus,
    RecoverInitialPromptCommand, RepositoryAccessPolicy, RequiredCapability, ResourcePolicy,
    ScopeClass, SessionLink, SetupPolicy, SourceMode, StartLaunchCommand, TriggerAvailability,
    TriggerConnection, TriggerConnectionAuthorization, TriggerConnectionManager, TriggerDraft,
    TriggerId, TriggerPageRequest, TriggerPreview, TriggerRepositorySelection, TriggerRevision,
    TriggerRevisionContent, TriggerRevisionId, TriggerServiceError, TriggerSessionPolicy,
    TriggerStoreError, TriggerTask, UpstreamRevision, WorkflowChangePolicy, WorkspaceCheckpoint,
    WorkspaceManifest, WorkspaceRecord,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::trigger_openapi_shapes as trigger_schema;
use super::{
    idempotency, problem_document, problem_response, BodyProblem, RequestFailure, TriggersState,
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
        .routes(routes!(download_diff_artifact))
        .routes(routes!(recover_initial_prompt))
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
    availability: String,
    publisher_display_identity: String,
    repository_id: u64,
    repository_owner: String,
    repository_name: String,
    visibility: String,
    allow_forking: bool,
    public_summary: String,
    permission_ceiling: String,
    required_capabilities: Vec<String>,
    resource_default: SessionSizeView,
    resource_ceiling: SessionSizeView,
    timeout_default_seconds: u32,
    timeout_ceiling_seconds: u32,
    billing_mode: String,
    pricing_policy_reference: String,
    revision_digest: String,
}

impl From<TriggerPreview> for TriggerPreviewView {
    fn from(value: TriggerPreview) -> Self {
        Self {
            public_id: value.public_id.0,
            availability: enum_name(value.availability),
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
    role: String,
    verified_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerRevisionView {
    id: String,
    public_id: String,
    digest: String,
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
    state: String,
    revision_number: u64,
    #[schema(value_type = trigger_schema::TriggerRevisionContent)]
    revision_content: TriggerRevisionContent,
    #[schema(value_type = Option<trigger_schema::LaunchBlockedReason>)]
    blocked_reason: Option<LaunchBlockedReason>,
    #[schema(value_type = Option<trigger_schema::LaunchFailure>)]
    failure: Option<LaunchFailure>,
    #[schema(value_type = trigger_schema::LaunchOverrides)]
    overrides: LaunchOverrides,
    #[schema(value_type = trigger_schema::EffectiveConfiguration)]
    effective: EffectiveConfiguration,
    #[schema(value_type = Option<trigger_schema::PricingSnapshot>)]
    pricing: Option<PricingSnapshot>,
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
    state: String,
    base_revision: Option<String>,
    head_revision: Option<String>,
    observed_title: Option<String>,
    observed_body_digest: Option<String>,
    mergeability: String,
    checks: String,
    confirmation_snapshot_digest: String,
    confirmed_at: String,
    created_at: String,
    updated_at: String,
    provider_created_at: Option<String>,
    provider_updated_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PullRequestDraftView {
    launch_id: String,
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
    key: String,
    sequence: u32,
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
            external_resource_ids: value.external_resource_ids,
            correlation_id: value.correlation_id,
        }
    }
}

impl From<Launch> for LaunchView {
    fn from(value: Launch) -> Self {
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
        Self {
            id: value.id.0,
            trigger_public_id: value.revision.public_id.0,
            revision_digest: value.revision_digest,
            source_sha: value.source_sha,
            state: enum_name(value.state),
            revision_number: value.revision_number,
            revision_content: value.revision.content,
            blocked_reason: value.blocked_reason,
            failure: value.failure,
            overrides: value.overrides,
            effective: value.effective,
            pricing: value.pricing,
            connection,
            workflow_paths: value.workflow_paths,
            workflow_risk_acknowledged: value.workflow_risk_acknowledged,
            steps: value.steps.into_iter().map(Into::into).collect(),
            fork: value.fork,
            branch: value.branch,
            workspace: value.workspace.map(Into::into),
            session: value.session,
            reach: value.reach,
            delivery: value.delivery,
            contribution_checkpoint_version: value.contribution_checkpoint_version,
            diff,
            contribution: value.contribution.map(Into::into),
            pull_request: value.pull_request.map(Into::into),
            created_at: value.created_at,
            updated_at: value.updated_at,
            started_at: value.started_at,
            terminal_at: value.terminal_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchPageView {
    items: Vec<LaunchView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LaunchProgressView {
    launch_id: String,
    events: Vec<LaunchEventView>,
    next_cursor: u64,
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchEventView {
    sequence: u64,
    launch_id: String,
    occurred_at: String,
    correlation_id: String,
    kind: LaunchEventKindView,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LaunchEventKindView {
    Created,
    DraftUpdated,
    ConnectionSelected,
    Started,
    StateChanged {
        from: String,
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
        effect: String,
    },
    RecoveryAuthorized {
        effect: String,
    },
    DeliveryChanged {
        state: String,
    },
    ReachChanged {
        #[schema(value_type = trigger_schema::ReachStatus)]
        reach: ReachStatus,
    },
    ConfirmationRecorded {
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
            kind: match value.kind {
                LaunchEventKind::Created => LaunchEventKindView::Created,
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
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
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
    let mut response = Json(load_public_preview(&state, public_id).await?).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&format!("public, max-age={PUBLIC_PREVIEW_CACHE_SECONDS}"))
            .expect("static cache policy is valid"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
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
    responses((status = 201, description = "Trigger draft created", body = TriggerDraftView))
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
    responses((status = 200, description = "Trigger draft replaced", body = TriggerDraftView))
)]
async fn update_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<UpdateTriggerRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-update:{trigger_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let trigger_id = parse_trigger_id(trigger_id)?;
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
    responses((status = 201, description = "Immutable Trigger revision published", body = TriggerRevisionView))
)]
async fn publish_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<PublishTriggerRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-publish:{trigger_id}");
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
                &parse_trigger_id(trigger_id)?,
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
    let operation = format!("trigger-retire:{revision_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        body.map_err(RequestFailure::from)?;
        let revision = state
            .triggers
            .retire(&caller.subject, &parse_revision_id(revision_id)?, &now())
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
    let operation = format!("trigger-launch-create:{public_id}");
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
        let launch = state
            .triggers
            .create_or_resume_launch(
                caller.subject.clone(),
                &parse_public_id(public_id)?,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::CREATED, Json(LaunchView::from(launch))).into_response())
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
        Ok((StatusCode::CREATED, Json(LaunchView::from(launch))).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches",
    tag = "launches",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Contributor Launches", body = LaunchPageView))
)]
async fn list_launches(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchPageView>, RequestFailure> {
    let Query(params) = params?;
    let page = state
        .triggers
        .list_launches(
            &caller.subject,
            TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(LaunchPageView {
        items: page.items.into_iter().map(Into::into).collect(),
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
    Ok(state
        .triggers
        .get_launch(owner, &parse_launch_id(launch_id)?)
        .await?
        .into())
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
    let operation = format!("trigger-launch-overrides:{launch_id}");
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
                &parse_launch_id(launch_id)?,
                body.expected_revision,
                body.overrides,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::from(launch)).into_response())
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
    let operation = format!("trigger-launch-connection:{launch_id}");
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
                &parse_launch_id(launch_id)?,
                body.expected_revision,
                &body.service_id,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::from(launch)).into_response())
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
    let operation = format!("trigger-launch-start:{launch_id}");
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
                &parse_launch_id(launch_id)?,
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
        Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
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
    let operation = format!("trigger-launch-diff-review:{launch_id}");
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
                &parse_launch_id(launch_id)?,
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
        Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
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
    Ok(DiffReviewView::from_launch(
        state
            .triggers
            .get_launch(owner, &parse_launch_id(launch_id)?)
            .await?,
    ))
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
    let operation = format!("trigger-launch-branch-publication:{launch_id}");
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
                &parse_launch_id(launch_id)?,
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
        Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
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
    let (body, _) = body?.into_parts();
    let body = body.map_err(RequestFailure::from)?;
    let launch = state
        .triggers
        .get_launch(&caller.subject, &parse_launch_id(launch_id)?)
        .await?;
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
    let operation_name = enum_name(operation);
    let claim = format!("trigger-launch-pull-request-{operation_name}:{launch_id}");
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
                &parse_launch_id(launch_id)?,
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
        Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
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
    let operation = format!("trigger-launch-cancel:{launch_id}");
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
                &parse_launch_id(launch_id)?,
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
        Ok(Json(LaunchView::from(launch)).into_response())
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
    let operation = format!("trigger-launch-prompt-recovery:{launch_id}");
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
                &parse_launch_id(launch_id)?,
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
        Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
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

impl From<TriggerServiceError> for TriggerProblem {
    fn from(value: TriggerServiceError) -> Self {
        Self(value)
    }
}

impl IntoResponse for TriggerProblem {
    fn into_response(self) -> Response {
        let rollout_reason = match &self.0 {
            TriggerServiceError::Rollout(denial) => Some(denial.code()),
            _ => None,
        };
        let mut retry_after = None;
        let (status, slug, code, detail, field): (StatusCode, &str, &str, String, Option<String>) =
            match self.0 {
                TriggerServiceError::NotFound | TriggerServiceError::Forbidden => (
                    StatusCode::NOT_FOUND,
                    "trigger-resource-not-found",
                    "trigger_resource_not_found",
                    "The requested Trigger resource does not exist.".into(),
                    None,
                ),
                TriggerServiceError::Validation(error) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid-trigger-request",
                    "invalid_trigger_request",
                    "The Trigger request violates the declared policy.".into(),
                    error.field,
                ),
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
        EditableField::BranchSuffix,
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
