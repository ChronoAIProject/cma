//! Schema-only mirrors for Trigger domain values exposed by REST.
//!
//! Layer 2 owns validation and Serde wire semantics without depending on an
//! HTTP schema generator. These mirrors keep OpenAPI concrete without moving a
//! protocol dependency into the domain layer.

#![allow(dead_code)]

use utoipa::ToSchema;

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceMode {
    PinnedCommit,
    BranchHeadAtLaunch,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum RepositoryVisibility {
    Public,
    Private,
    Internal,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum TriggerAvailability {
    Published,
    Retired,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum TriggerPreviewAvailability {
    Published,
    Retired,
    Unavailable,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum TriggerPreviewUnavailableReason {
    UpstreamNotForkable,
    UpstreamUnavailable,
    UpstreamIdentityChanged,
    RepositoryPolicyUnavailable,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PublisherRepositoryRole {
    Owner,
    Admin,
    Maintain,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConnectionState {
    Active,
    Inactive,
    Disconnected,
    Revoked,
    Expired,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct RepositoryIdentity {
    repository_id: u64,
    repository_node_id: String,
    owner_id: u64,
    owner_node_id: String,
    canonical_owner: String,
    canonical_name: String,
    visibility: RepositoryVisibility,
    allow_forking: bool,
    default_branch: String,
    selected_ref: String,
    fork_parent_id: Option<u64>,
    fork_source_id: Option<u64>,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct UpstreamRevision {
    repository: RepositoryIdentity,
    source_mode: SourceMode,
    observed_source_sha: Option<String>,
    contribution_base_ref: String,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum InputFieldKind {
    Text { min_bytes: u32, max_bytes: u32 },
    Integer { minimum: i64, maximum: i64 },
    Boolean,
    Choice { values: Vec<String> },
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct InputFieldDefinition {
    key: String,
    kind: InputFieldKind,
    required: bool,
    #[schema(nullable = true)]
    default: Option<serde_json::Value>,
    editable: bool,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct TriggerTask {
    public_summary: String,
    prompt_template: String,
    renderer_version: u32,
    input_fields: Vec<InputFieldDefinition>,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct SessionSize {
    #[schema(minimum = 1)]
    cpu_millis: u32,
    #[schema(minimum = 1)]
    memory_mib: u32,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct ResourcePolicy {
    default: SessionSize,
    ceiling: SessionSize,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum RequiredCapability {
    Conversation,
    DurableConversation,
    WorkspaceCheckpoint,
    RestrictedServiceGateway,
    CredentialRefresh,
    CredentialRevocation,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermissionMode {
    Guarded,
    Autonomous,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct TriggerSessionPolicy {
    required_capabilities: Vec<RequiredCapability>,
    permission_ceiling: PermissionMode,
    sandbox_profile: String,
    resources: ResourcePolicy,
    timeout_default_seconds: u32,
    timeout_ceiling_seconds: u32,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CredentialRoute {
    ContributorConnection,
    SponsoredGrant,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ScopeClass {
    PublicRepository,
    Workflow,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum RepositoryOperationClass {
    RepositoryRead,
    TreeRead,
    ObjectCreate,
    LaunchRefCreate,
    LaunchRefUpdate,
    StatusRead,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkflowChangePolicy {
    Refuse,
    RequireScopeAndConfirmation,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct RepositoryAccessPolicy {
    credential_route: CredentialRoute,
    required_scope_classes: Vec<ScopeClass>,
    allowed_operation_classes: Vec<RepositoryOperationClass>,
    workflow_change_policy: WorkflowChangePolicy,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExecutorPolicy {
    Disabled,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct SetupPolicy {
    executor_policy: ExecutorPolicy,
    steps: Vec<String>,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CommitAttributionPolicy {
    ContributorNoReply,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct PullRequestDefaults {
    title_template: String,
    body_template: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct ContributionPolicy {
    branch_prefix: String,
    commit_attribution: CommitAttributionPolicy,
    pull_request: PullRequestDefaults,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum BillingMode {
    Contributor,
    ZeroCost,
    Sponsored,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct SponsoredPolicyBinding {
    tenant_id: String,
    policy_id: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct CommercialPolicy {
    billing_mode: BillingMode,
    policy_reference: String,
    sponsorship: Option<SponsoredPolicyBinding>,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum EditableField {
    TaskInput,
    WorkLabel,
    ResourceSize,
    Timeout,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct OverridePolicy {
    editable_fields: Vec<EditableField>,
    #[schema(min_items = 0, max_items = 0)]
    optional_checks: Vec<String>,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct PublicProjectionPolicy {
    version: u32,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct TriggerRevisionContent {
    schema_version: u32,
    trigger_id: String,
    revision_number: u32,
    publisher_subject_id: String,
    publisher_display_identity: String,
    upstream: UpstreamRevision,
    task: TriggerTask,
    session: TriggerSessionPolicy,
    repository_access: RepositoryAccessPolicy,
    setup: SetupPolicy,
    contribution: ContributionPolicy,
    commercial_policy: CommercialPolicy,
    override_policy: OverridePolicy,
    public_projection: PublicProjectionPolicy,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct LaunchOverrides {
    task_inputs: std::collections::BTreeMap<String, serde_json::Value>,
    work_label: Option<String>,
    resource_size: Option<SessionSize>,
    timeout_seconds: Option<u32>,
    #[schema(min_items = 0, max_items = 0)]
    optional_checks: Vec<String>,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct EffectiveConfiguration {
    rendered_prompt: String,
    work_label: String,
    permission: PermissionMode,
    resource_size: SessionSize,
    timeout_seconds: u32,
    #[schema(min_items = 0, max_items = 0)]
    optional_checks: Vec<String>,
    digest: String,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum LaunchState {
    Draft,
    AuthRequired,
    Preflight,
    ForkPending,
    ForkReady,
    BranchPending,
    WorkspacePending,
    SandboxPending,
    SessionPending,
    PromptPending,
    Ready,
    Contributing,
    PrPending,
    PrOpen,
    Completed,
    FailedRetryable,
    FailedTerminal,
    Cancelled,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum LaunchStepKey {
    Review,
    Authentication,
    Preflight,
    Fork,
    Branch,
    Workspace,
    Sandbox,
    Session,
    Reach,
    InitialPrompt,
    Checkpoint,
    Diff,
    PublishBranch,
    PullRequest,
    Cleanup,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum LaunchStepStatus {
    Pending,
    Running,
    Completed,
    Blocked,
    Retryable,
    Failed,
    Cancelled,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum LaunchBlockedReason {
    FeaturePaused,
    OwnerCredentialUnavailable,
    ServiceInactive,
    SessionRotating,
    SessionRecovering,
    RecoveryRequired,
    RevocationPending,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum FailureClass {
    AuthRequired,
    OwnerCredentialUnavailable,
    ServiceInactive,
    RetryableTransient,
    AmbiguousExternal,
    ConflictStaleHead,
    ResourceQuota,
    PolicyUserAction,
    RecoveryRequired,
    TerminalValidation,
    TerminalSecurity,
    FeaturePaused,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct LaunchFailure {
    class: FailureClass,
    code: String,
    detail: String,
    remediation: String,
    correlation_id: String,
    retry_at: Option<String>,
    observed_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct PricingSnapshot {
    policy_reference: String,
    billing_mode: BillingMode,
    rate_card: Option<String>,
    rate: Option<Rate>,
    currency: String,
    cpu_millis: u32,
    memory_mib: u32,
    micros_per_hour: u64,
    estimated_micros: u64,
    quoted_at: String,
    expires_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct Rate {
    cpu_micros_per_cpu_hour: u64,
    memory_micros_per_gib_hour: u64,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) enum TriggerKernel {
    /// Historical pre-versioned route retained for inspection, cancellation,
    /// session access, and fenced cleanup only.
    #[serde(rename = "codex")]
    Codex,
    /// Current dedicated Trigger runtime route admitted for new work.
    #[serde(rename = "codex-trigger-native-v1")]
    CodexTriggerNativeV1,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct RuntimeAdmissionSnapshot {
    kernel: TriggerKernel,
    runtime_version: String,
    supported_capabilities: Vec<RequiredCapability>,
    observed_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct QuotaSnapshot {
    live_sessions: u32,
    live_session_limit: Option<u32>,
    reserved_cpu_millis: u64,
    reserved_memory_mib: u64,
    free_cpu_millis: Option<u64>,
    free_memory_mib: Option<u64>,
    checked_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct StartAdmissionSnapshot {
    runtime_version: String,
    pricing_policy_reference: String,
    rate_card: Option<String>,
    quota: QuotaSnapshot,
    checked_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct ForkRecord {
    repository: RepositoryIdentity,
    reused: bool,
    source_revision: String,
    canonical_url: String,
    ready_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct BranchRecord {
    name: String,
    expected_head: String,
    current_head: String,
    canonical_url: String,
    created_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct SessionLink {
    session_id: String,
    environment_profile_id: String,
    runtime_generation: u64,
    created_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct SessionCredentialRetirement {
    session_id: String,
    settled_at: String,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CapabilityRevocationState {
    Active,
    RevocationPending,
    Revoked,
    Expired,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CapabilityRefusal {
    MissingPolicy,
    UnknownVersion,
    Expired,
    StaleGeneration,
    Revoked,
    MalformedRequest,
    ResourceMismatch,
    OperationDenied,
    AudienceMismatch,
    SignatureInvalid,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct ReachStatus {
    service_id: String,
    state: CapabilityRevocationState,
    policy_generation: u64,
    policy_fence: u64,
    runtime_audience: String,
    expires_at_epoch_seconds: u64,
    numeric_binding_observed_at: String,
    refreshed_at: String,
    revocation_requested_at: Option<String>,
    revocation_effective_at: Option<String>,
    last_refusal: Option<CapabilityRefusal>,
    tombstone_acknowledged: bool,
    agent_key_revocation_confirmed: bool,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PromptDeliveryState {
    Queued,
    DispatchFenced,
    WriteConfirmed,
    ExecutionObserved,
    Completed,
    Failed,
    RecoveryRequired,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum LaunchEffectKind {
    Preflight,
    Fork,
    Branch,
    WorkspaceImport,
    Sandbox,
    Session,
    Reach,
    InitialDispatch,
    Checkpoint,
    Diff,
    BranchPublication,
    PullRequest,
    Cleanup,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConfirmationAction {
    Start,
    WorkflowRisk,
    ReviewDiff,
    PublishBranch,
    OpenPullRequest,
    UpdatePullRequest,
    Complete,
    RecoverPrompt,
    NewLaunch,
    Cancel,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct PromptDelivery {
    launch_id: String,
    session_id: String,
    revision_digest: String,
    origin_key: String,
    payload_hash: String,
    state: PromptDeliveryState,
    attempt: u32,
    recovery_generation: u32,
    dispatch_fence_sequence: Option<u64>,
    queued_at: String,
    updated_at: String,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct CommitIdentity {
    name: String,
    email: String,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum DiffChangeKind {
    Added,
    Modified,
    Deleted,
    ModeChanged,
    SymlinkChanged,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum DiffValidationOutcome {
    Valid,
    NoChanges,
    StaleHead,
    WorkflowScopeRequired,
    TooManyChanges,
    TooManyBytes,
    UnsupportedFile,
    MissingBaseline,
    CorruptCheckpoint,
    ArtifactTooLarge,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CheckpointIntegrity {
    Pending,
    Complete,
    Corrupt,
    RecoveryRequired,
    Deleted,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PublicationState {
    IntentRecorded,
    ObjectsWritten,
    RefUpdatePending,
    Published,
    Conflict,
    RecoveryRequired,
    Failed,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PullRequestOperation {
    Open,
    Update,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PullRequestState {
    Creating,
    Updating,
    Open,
    ClosedUnmerged,
    Merged,
    RecoveryRequired,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mergeability {
    Computing,
    Mergeable,
    Conflicting,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CheckSummaryState {
    Pending,
    Passing,
    Failing,
    AwaitingMaintainerApproval,
    Unavailable,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct DiffFile {
    path: String,
    change: DiffChangeKind,
    binary: bool,
    old_mode: Option<u32>,
    new_mode: Option<u32>,
    old_sha256: Option<String>,
    new_sha256: Option<String>,
    old_provider_object_id: Option<String>,
    new_provider_object_id: Option<String>,
    old_bytes: u64,
    new_bytes: u64,
    #[schema(required = true, nullable = true, minimum = 0)]
    additions: Option<u64>,
    #[schema(required = true, nullable = true, minimum = 0)]
    deletions: Option<u64>,
    patch: Option<String>,
    patch_complete: bool,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct CheckResult {
    key: String,
    passed: bool,
    summary: String,
}

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkspaceEntryKind {
    Regular,
    Symlink,
    Submodule,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct WorkspaceEntry {
    path: String,
    kind: WorkspaceEntryKind,
    mode: u32,
    size: u64,
    content_sha256: String,
    provider_object_id: Option<String>,
    symlink_target: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
pub(super) struct WorkspaceManifest {
    version: u32,
    source_revision: String,
    entries: Vec<WorkspaceEntry>,
    total_bytes: u64,
    digest: String,
}
