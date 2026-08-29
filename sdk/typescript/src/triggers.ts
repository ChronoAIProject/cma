/** Public CMA Trigger and Launch REST contract. */

import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, requestBytes, type OperationOptions } from './http.js';

const TRIGGER_AVAILABILITIES = ['published', 'retired'] as const;
const TRIGGER_PREVIEW_AVAILABILITIES = ['published', 'retired', 'unavailable'] as const;
const TRIGGER_PREVIEW_UNAVAILABLE_REASONS = [
  'upstream_not_forkable',
  'upstream_unavailable',
  'upstream_identity_changed',
  'repository_policy_unavailable',
] as const;
const REPOSITORY_VISIBILITIES = ['public', 'private', 'internal'] as const;
const REQUIRED_CAPABILITIES = [
  'conversation',
  'durable_conversation',
  'workspace_checkpoint',
  'restricted_service_gateway',
  'credential_refresh',
  'credential_revocation',
] as const;
const TRIGGER_KERNELS = ['codex', 'codex-trigger-native-v1'] as const;
const PERMISSION_CEILINGS = ['guarded', 'autonomous'] as const;
const BILLING_MODES = ['contributor', 'zero_cost', 'sponsored'] as const;
const TRIGGER_PREVIEW_COST_KINDS = [
  'exact_zero',
  'contributor_estimate',
  'quote_unavailable',
] as const;
const PUBLISHER_ROLES = ['owner', 'admin', 'maintain'] as const;
const CONNECTION_STATES = ['active', 'inactive', 'disconnected', 'revoked', 'expired'] as const;
const SOURCE_MODES = ['pinned_commit', 'branch_head_at_launch'] as const;
const SCOPE_CLASSES = ['public_repository', 'workflow'] as const;
const REPOSITORY_OPERATION_CLASSES = [
  'repository_read',
  'tree_read',
  'object_create',
  'launch_ref_create',
  'launch_ref_update',
  'status_read',
] as const;
const CREDENTIAL_ROUTES = ['contributor_connection', 'sponsored_grant'] as const;
const WORKFLOW_CHANGE_POLICIES = ['refuse', 'require_scope_and_confirmation'] as const;
const EDITABLE_FIELDS = ['task_input', 'work_label', 'resource_size', 'timeout'] as const;
const LAUNCH_STATES = [
  'draft',
  'auth_required',
  'preflight',
  'fork_pending',
  'fork_ready',
  'branch_pending',
  'workspace_pending',
  'sandbox_pending',
  'session_pending',
  'prompt_pending',
  'ready',
  'contributing',
  'pr_pending',
  'pr_open',
  'completed',
  'failed_retryable',
  'failed_terminal',
  'cancelled',
] as const;
const UTC_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/;
const BLOCKED_REASONS = [
  'feature_paused',
  'owner_credential_unavailable',
  'service_inactive',
  'session_rotating',
  'session_recovering',
  'recovery_required',
  'revocation_pending',
] as const;
const STEP_KEYS = [
  'review',
  'authentication',
  'preflight',
  'fork',
  'branch',
  'workspace',
  'sandbox',
  'session',
  'reach',
  'initial_prompt',
  'checkpoint',
  'diff',
  'publish_branch',
  'pull_request',
  'cleanup',
] as const;
const STEP_STATUSES = [
  'pending',
  'running',
  'completed',
  'blocked',
  'retryable',
  'failed',
  'cancelled',
] as const;
const FAILURE_CLASSES = [
  'auth_required',
  'owner_credential_unavailable',
  'service_inactive',
  'retryable_transient',
  'ambiguous_external',
  'conflict_stale_head',
  'resource_quota',
  'policy_user_action',
  'recovery_required',
  'terminal_validation',
  'terminal_security',
  'feature_paused',
] as const;
const PROMPT_DELIVERY_STATES = [
  'queued',
  'dispatch_fenced',
  'write_confirmed',
  'execution_observed',
  'completed',
  'failed',
  'recovery_required',
] as const;
const CAPABILITY_REVOCATION_STATES = [
  'active',
  'revocation_pending',
  'revoked',
  'expired',
] as const;
const CAPABILITY_REFUSALS = [
  'missing_policy',
  'unknown_version',
  'expired',
  'stale_generation',
  'revoked',
  'malformed_request',
  'resource_mismatch',
  'operation_denied',
  'audience_mismatch',
  'signature_invalid',
] as const;
const DIFF_CHANGE_KINDS = [
  'added',
  'modified',
  'deleted',
  'mode_changed',
  'symlink_changed',
] as const;
const DIFF_VALIDATION_OUTCOMES = [
  'valid',
  'no_changes',
  'stale_head',
  'workflow_scope_required',
  'too_many_changes',
  'too_many_bytes',
  'unsupported_file',
  'missing_baseline',
  'corrupt_checkpoint',
  'artifact_too_large',
] as const;
const PUBLICATION_STATES = [
  'intent_recorded',
  'objects_written',
  'ref_update_pending',
  'published',
  'conflict',
  'recovery_required',
  'failed',
] as const;
const PULL_REQUEST_OPERATIONS = ['open', 'update'] as const;
const PULL_REQUEST_STATES = [
  'creating',
  'updating',
  'open',
  'closed_unmerged',
  'merged',
  'recovery_required',
] as const;
const MERGEABILITIES = ['computing', 'mergeable', 'conflicting'] as const;
const CHECK_SUMMARY_STATES = [
  'pending',
  'passing',
  'failing',
  'awaiting_maintainer_approval',
  'unavailable',
] as const;
const LAUNCH_EFFECT_KINDS = [
  'preflight',
  'fork',
  'branch',
  'workspace_import',
  'sandbox',
  'session',
  'reach',
  'initial_dispatch',
  'checkpoint',
  'diff',
  'branch_publication',
  'pull_request',
  'cleanup',
] as const;
const CONFIRMATION_ACTIONS = [
  'start',
  'workflow_risk',
  'review_diff',
  'publish_branch',
  'open_pull_request',
  'update_pull_request',
  'recover_prompt',
  'new_launch',
  'cancel',
  'complete',
] as const;

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonObject | readonly JsonValue[];
export interface JsonObject {
  readonly [key: string]: JsonValue;
}

export interface TriggerCommandOptions extends OperationOptions {
  /** Stable key reused only while retrying this exact semantic command. */
  readonly idempotencyKey: string;
}

export interface CursorPage<T> {
  readonly items: readonly T[];
  readonly next_cursor: string | null;
}

export interface SessionSize {
  readonly cpu_millis: number;
  readonly memory_mib: number;
}

/** Public cost of the immutable Trigger defaults, before authenticated review. */
export type TriggerPreviewCost =
  | {
      readonly kind: 'exact_zero';
      readonly currency: 'USD';
      readonly amount_micros: 0;
      readonly display: string;
    }
  | {
      readonly kind: 'contributor_estimate';
      readonly currency: 'USD';
      readonly amount_micros: number;
      readonly display: string;
      readonly quoted_at: string;
    }
  | {
      readonly kind: 'quote_unavailable';
    };

export type TriggerPreviewUnavailableReason = (typeof TRIGGER_PREVIEW_UNAVAILABLE_REASONS)[number];

interface TriggerPreviewFields {
  readonly public_id: string;
  readonly publisher_display_identity: string;
  readonly repository_id: number;
  readonly repository_owner: string;
  readonly repository_name: string;
  readonly visibility: 'public' | 'private' | 'internal';
  readonly allow_forking: boolean;
  readonly public_summary: string;
  readonly permission_ceiling: TriggerPermissionCeiling;
  readonly required_capabilities: readonly TriggerRequiredCapability[];
  readonly resource_default: SessionSize;
  readonly resource_ceiling: SessionSize;
  readonly timeout_default_seconds: number;
  readonly timeout_ceiling_seconds: number;
  readonly billing_mode: 'contributor' | 'zero_cost' | 'sponsored';
  readonly pricing_policy_reference: string;
  readonly estimated_cost: TriggerPreviewCost;
  readonly revision_digest: string;
}

export type TriggerPreview = TriggerPreviewFields &
  (
    | {
        readonly availability: 'published' | 'retired';
        readonly unavailable_reason: null;
      }
    | {
        readonly availability: 'unavailable';
        readonly unavailable_reason: TriggerPreviewUnavailableReason;
      }
  );

export interface RepositoryIdentity {
  readonly repository_id: number;
  readonly repository_node_id: string;
  readonly owner_id: number;
  readonly owner_node_id: string;
  readonly canonical_owner: string;
  readonly canonical_name: string;
  readonly visibility: 'public' | 'private' | 'internal';
  readonly allow_forking: boolean;
  readonly default_branch: string;
  readonly selected_ref: string;
  readonly fork_parent_id?: number;
  readonly fork_source_id?: number;
}

/** Canonical repository identity resolved through one exact reusable connection. */
export interface TriggerRepositorySelection {
  readonly repository: RepositoryIdentity;
  readonly source_sha: string;
}

export type TriggerSourceMode = 'pinned_commit' | 'branch_head_at_launch';

export interface TriggerTextInputBounds {
  readonly min_bytes: number;
  readonly max_bytes: number;
}

export interface TriggerIntegerInputBounds {
  readonly minimum: number;
  readonly maximum: number;
}

export interface TriggerChoiceInputValues {
  readonly values: readonly string[];
}

/** Serde's externally tagged wire shape for one publisher input kind. */
export type TriggerInputFieldKind =
  | { readonly text: TriggerTextInputBounds }
  | { readonly integer: TriggerIntegerInputBounds }
  | 'boolean'
  | { readonly choice: TriggerChoiceInputValues };

export interface TriggerInputFieldDefinition {
  readonly key: string;
  readonly kind: TriggerInputFieldKind;
  readonly required: boolean;
  readonly default?: JsonValue;
  readonly editable: boolean;
}

export interface TriggerTask {
  readonly public_summary: string;
  readonly prompt_template: string;
  readonly renderer_version: number;
  readonly input_fields: readonly TriggerInputFieldDefinition[];
}

export type TriggerRequiredCapability =
  | 'conversation'
  | 'durable_conversation'
  | 'workspace_checkpoint'
  | 'restricted_service_gateway'
  | 'credential_refresh'
  | 'credential_revocation';

/**
 * Closed kernel identity for a Trigger-owned session. `codex-trigger-native-v1`
 * is current; `codex` is a historical, read-only Launch route that may be
 * inspected, cancelled, followed to its session, or used to download a
 * retained artifact, but may not authorize new Trigger work.
 */
export type TriggerKernel = (typeof TRIGGER_KERNELS)[number];

/** The public Trigger contract deliberately excludes unrestricted sessions. */
export type TriggerPermissionCeiling = 'guarded' | 'autonomous';

export interface TriggerResourcePolicy {
  readonly default: SessionSize;
  readonly ceiling: SessionSize;
}

export interface TriggerSessionPolicy {
  readonly required_capabilities: readonly TriggerRequiredCapability[];
  readonly permission_ceiling: TriggerPermissionCeiling;
  readonly sandbox_profile: string;
  readonly resources: TriggerResourcePolicy;
  readonly timeout_default_seconds: number;
  readonly timeout_ceiling_seconds: number;
}

export type TriggerScopeClass = 'public_repository' | 'workflow';

export type TriggerRepositoryOperationClass =
  | 'repository_read'
  | 'tree_read'
  | 'object_create'
  | 'launch_ref_create'
  | 'launch_ref_update'
  | 'status_read';

export type TriggerCredentialRoute = 'contributor_connection' | 'sponsored_grant';
export type TriggerWorkflowChangePolicy = 'refuse' | 'require_scope_and_confirmation';

export interface TriggerRepositoryAccessPolicy {
  readonly credential_route: TriggerCredentialRoute;
  readonly required_scope_classes: readonly TriggerScopeClass[];
  readonly allowed_operation_classes: readonly TriggerRepositoryOperationClass[];
  readonly workflow_change_policy: TriggerWorkflowChangePolicy;
}

export interface TriggerSetupPolicy {
  readonly executor_policy: 'disabled';
  readonly steps: readonly string[];
}

export interface TriggerPullRequestDefaults {
  readonly title_template: string;
  readonly body_template: string;
}

export interface TriggerContributionPolicy {
  readonly branch_prefix: string;
  readonly commit_attribution: 'contributor_no_reply';
  readonly pull_request: TriggerPullRequestDefaults;
}

export type TriggerBillingMode = 'contributor' | 'zero_cost' | 'sponsored';

export interface TriggerCommercialPolicy {
  readonly billing_mode: TriggerBillingMode;
  readonly policy_reference: string;
}

export type TriggerEditableField = 'task_input' | 'work_label' | 'resource_size' | 'timeout';

export interface TriggerOverridePolicy {
  readonly editable_fields: readonly TriggerEditableField[];
  readonly optional_checks: readonly [];
}

export interface TriggerPublicProjectionPolicy {
  readonly version: number;
}

export interface TriggerUpstreamRevision {
  readonly repository: RepositoryIdentity;
  readonly source_mode: TriggerSourceMode;
  readonly observed_source_sha?: string;
  readonly contribution_base_ref: string;
}

export interface TriggerRevisionContent {
  readonly schema_version: number;
  readonly trigger_id: string;
  readonly revision_number: number;
  readonly publisher_subject_id: string;
  readonly publisher_display_identity: string;
  readonly upstream: TriggerUpstreamRevision;
  readonly task: TriggerTask;
  readonly session: TriggerSessionPolicy;
  readonly repository_access: TriggerRepositoryAccessPolicy;
  readonly setup: TriggerSetupPolicy;
  readonly contribution: TriggerContributionPolicy;
  readonly commercial_policy: TriggerCommercialPolicy;
  readonly override_policy: TriggerOverridePolicy;
  readonly public_projection: TriggerPublicProjectionPolicy;
}

export interface RepositorySelectionDraft {
  readonly repository_id: number;
  readonly selected_ref: string;
  readonly contribution_base_ref: string;
  readonly source_mode: TriggerSourceMode;
}

export interface TriggerContentDraft {
  readonly schema_version: number;
  readonly publisher_display_identity: string;
  readonly repository: RepositorySelectionDraft;
  readonly task: TriggerTask;
  readonly session: TriggerSessionPolicy;
  readonly repository_access: TriggerRepositoryAccessPolicy;
  readonly setup: TriggerSetupPolicy;
  readonly contribution: TriggerContributionPolicy;
  readonly commercial_policy: TriggerCommercialPolicy;
  readonly override_policy: TriggerOverridePolicy;
  readonly public_projection: TriggerPublicProjectionPolicy;
}

export interface TriggerDraft {
  readonly id: string;
  readonly selected_service_id: string;
  readonly version: number;
  readonly next_revision_number: number;
  readonly content: TriggerRevisionContent;
  readonly created_at: string;
  readonly updated_at: string;
}

export interface TriggerRevision {
  readonly id: string;
  readonly public_id: string;
  readonly digest: string;
  readonly availability: 'published' | 'retired';
  readonly content: TriggerRevisionContent;
  readonly authority: {
    readonly service_id: string;
    readonly service_state_version: number;
    readonly account_id: number;
    readonly account_login: string;
    readonly role: TriggerPublisherRepositoryRole;
    readonly verified_at: string;
  };
  readonly published_at: string;
  readonly retired_at: string | null;
}

export type TriggerPublisherRepositoryRole = 'owner' | 'admin' | 'maintain';

export type TriggerConnectionState = 'active' | 'inactive' | 'disconnected' | 'revoked' | 'expired';

export interface TriggerConnection {
  readonly service_id: string;
  readonly state_version: number;
  readonly state: TriggerConnectionState;
  readonly account_id: number | null;
  readonly account_node_id: string | null;
  readonly account_login: string | null;
  readonly avatar_url: string | null;
  readonly granted_scopes: readonly string[];
  readonly consent_observed_at: string | null;
}

export interface ConnectionAuthorization {
  readonly service_id: string;
  readonly authorization_url: string;
  readonly attempt_nonce: string;
}

export type LaunchState =
  | 'draft'
  | 'auth_required'
  | 'preflight'
  | 'fork_pending'
  | 'fork_ready'
  | 'branch_pending'
  | 'workspace_pending'
  | 'sandbox_pending'
  | 'session_pending'
  | 'prompt_pending'
  | 'ready'
  | 'contributing'
  | 'pr_pending'
  | 'pr_open'
  | 'completed'
  | 'failed_retryable'
  | 'failed_terminal'
  | 'cancelled';

export type LaunchBlockedReason =
  | 'feature_paused'
  | 'owner_credential_unavailable'
  | 'service_inactive'
  | 'session_rotating'
  | 'session_recovering'
  | 'recovery_required'
  | 'revocation_pending';

export type LaunchStepStatus =
  'pending' | 'running' | 'completed' | 'blocked' | 'retryable' | 'failed' | 'cancelled';

export type LaunchStepKey =
  | 'review'
  | 'authentication'
  | 'preflight'
  | 'fork'
  | 'branch'
  | 'workspace'
  | 'sandbox'
  | 'session'
  | 'reach'
  | 'initial_prompt'
  | 'checkpoint'
  | 'diff'
  | 'publish_branch'
  | 'pull_request'
  | 'cleanup';

export interface LaunchStep {
  readonly key: LaunchStepKey;
  readonly sequence: number;
  readonly status: LaunchStepStatus;
  readonly started_at: string | null;
  readonly completed_at: string | null;
  readonly attempt: number;
  readonly safe_detail: string;
  readonly remediation: string;
  readonly external_resource_ids: Readonly<Record<string, string>>;
  readonly correlation_id: string;
}

export type LaunchFailureClass =
  | 'auth_required'
  | 'owner_credential_unavailable'
  | 'service_inactive'
  | 'retryable_transient'
  | 'ambiguous_external'
  | 'conflict_stale_head'
  | 'resource_quota'
  | 'policy_user_action'
  | 'recovery_required'
  | 'terminal_validation'
  | 'terminal_security'
  | 'feature_paused';

export interface LaunchFailure {
  readonly class: LaunchFailureClass;
  readonly code: string;
  readonly detail: string;
  readonly remediation: string;
  readonly correlation_id: string;
  readonly retry_at?: string;
  readonly observed_at: string;
}

export type PromptDeliveryState =
  | 'queued'
  | 'dispatch_fenced'
  | 'write_confirmed'
  | 'execution_observed'
  | 'completed'
  | 'failed'
  | 'recovery_required';

export interface PromptDelivery {
  readonly launch_id: string;
  readonly session_id: string;
  readonly revision_digest: string;
  readonly origin_key: string;
  readonly payload_hash: string;
  readonly state: PromptDeliveryState;
  readonly attempt: number;
  readonly recovery_generation: number;
  readonly dispatch_fence_sequence: number | null;
  readonly queued_at: string;
  readonly updated_at: string;
}

export type LaunchCapabilityRevocationState =
  'active' | 'revocation_pending' | 'revoked' | 'expired';

export type LaunchCapabilityRefusal =
  | 'missing_policy'
  | 'unknown_version'
  | 'expired'
  | 'stale_generation'
  | 'revoked'
  | 'malformed_request'
  | 'resource_mismatch'
  | 'operation_denied'
  | 'audience_mismatch'
  | 'signature_invalid';

export interface LaunchReachStatus {
  readonly service_id: string;
  readonly state: LaunchCapabilityRevocationState;
  readonly policy_generation: number;
  readonly policy_fence: number;
  readonly runtime_audience: string;
  readonly expires_at_epoch_seconds: number;
  readonly numeric_binding_observed_at: string;
  readonly refreshed_at: string;
  readonly revocation_requested_at?: string;
  readonly revocation_effective_at?: string;
  readonly last_refusal?: LaunchCapabilityRefusal;
  readonly tombstone_acknowledged: boolean;
  readonly agent_key_revocation_confirmed: boolean;
}

/** Positive durable evidence that CMA retired credentials bound to this Launch session. */
export interface LaunchSessionCredentialRetirement {
  readonly session_id: string;
  readonly settled_at: string;
}

export interface LaunchOverrides {
  readonly task_inputs: Readonly<Record<string, JsonValue>>;
  readonly work_label?: string;
  readonly resource_size?: SessionSize;
  readonly timeout_seconds?: number;
  readonly optional_checks: readonly [];
}

export interface EffectiveConfiguration {
  readonly rendered_prompt: string;
  readonly work_label: string;
  readonly permission: TriggerPermissionCeiling;
  readonly resource_size: SessionSize;
  readonly timeout_seconds: number;
  readonly optional_checks: readonly [];
  readonly digest: string;
}

export interface TriggerRate {
  readonly cpu_micros_per_cpu_hour: number;
  readonly memory_micros_per_gib_hour: number;
}

/** Server-owned price or explicit zero-cost promise sealed into the Launch review. */
export interface LaunchPricingSnapshot {
  readonly policy_reference: string;
  readonly billing_mode: TriggerBillingMode;
  readonly rate_card?: string;
  readonly rate?: TriggerRate;
  readonly currency: string;
  readonly cpu_millis: number;
  readonly memory_mib: number;
  readonly micros_per_hour: number;
  readonly estimated_micros: number;
  readonly quoted_at: string;
  readonly expires_at: string;
}

/** Locked kernel, pinned runtime identity, and demonstrated capabilities shown during review. */
export interface LaunchRuntimeAdmission {
  readonly kernel: TriggerKernel;
  readonly runtime_version: string;
  readonly supported_capabilities: readonly TriggerRequiredCapability[];
  readonly observed_at: string;
}

/** Review- or Start-time observation of owner and deployment capacity. */
export interface LaunchQuotaSnapshot {
  readonly live_sessions: number;
  readonly live_session_limit?: number;
  readonly reserved_cpu_millis: number;
  readonly reserved_memory_mib: number;
  readonly free_cpu_millis?: number;
  readonly free_memory_mib?: number;
  readonly checked_at: string;
}

/** Fresh server admission recorded immediately before Start is accepted. */
export interface LaunchStartAdmission {
  readonly runtime_version: string;
  readonly pricing_policy_reference: string;
  readonly rate_card?: string;
  readonly quota: LaunchQuotaSnapshot;
  readonly checked_at: string;
}

export interface ForkRecord {
  readonly repository: RepositoryIdentity;
  readonly reused: boolean;
  readonly source_revision: string;
  readonly canonical_url: string;
  readonly ready_at: string;
}

export interface BranchRecord {
  readonly name: string;
  readonly expected_head: string;
  readonly current_head: string;
  readonly canonical_url: string;
  readonly created_at: string;
}

export interface CommitIdentity {
  readonly name: string;
  readonly email: string;
}

export interface DiffFile {
  readonly path: string;
  readonly change: 'added' | 'modified' | 'deleted' | 'mode_changed' | 'symlink_changed';
  readonly binary: boolean;
  readonly old_mode: number | null;
  readonly new_mode: number | null;
  readonly old_sha256: string | null;
  readonly new_sha256: string | null;
  readonly old_provider_object_id?: string;
  readonly new_provider_object_id?: string;
  readonly old_bytes: number;
  readonly new_bytes: number;
  /** Exact inserted text lines, or null when binary line counts are unavailable. */
  readonly additions: number | null;
  /** Exact deleted text lines, with availability paired to additions. */
  readonly deletions: number | null;
  readonly patch?: string;
  readonly patch_complete: boolean;
}

export interface DiffArtifact {
  readonly id: string;
  readonly checkpoint_id: string;
  readonly baseline_digest: string;
  readonly checkpoint_digest: string;
  readonly expected_branch_head: string;
  readonly observed_branch_head: string;
  readonly changed_file_limit: number;
  readonly total_changed_files: number;
  readonly files: readonly DiffFile[];
  readonly changed_bytes: number;
  readonly digest: string;
  readonly object_bytes: number;
  readonly author: CommitIdentity;
  readonly committer: CommitIdentity;
  readonly workflow_paths: readonly string[];
  readonly validation: DiffValidationOutcome;
  readonly checks: readonly {
    readonly key: string;
    readonly passed: boolean;
    readonly summary: string;
  }[];
  readonly created_at: string;
  readonly delete_after: string;
  readonly download_url: string;
}

export type DiffValidationOutcome =
  | 'valid'
  | 'no_changes'
  | 'stale_head'
  | 'workflow_scope_required'
  | 'too_many_changes'
  | 'too_many_bytes'
  | 'unsupported_file'
  | 'missing_baseline'
  | 'corrupt_checkpoint'
  | 'artifact_too_large';

export type PublicationState =
  | 'intent_recorded'
  | 'objects_written'
  | 'ref_update_pending'
  | 'published'
  | 'conflict'
  | 'recovery_required'
  | 'failed';

export interface ContributionRecord {
  readonly id: string;
  readonly diff_artifact_id: string;
  readonly revision_digest: string;
  readonly baseline_digest: string;
  readonly checkpoint_digest: string;
  readonly diff_digest: string;
  readonly branch_name: string;
  readonly state: PublicationState;
  readonly expected_head: string;
  readonly base_tree_id: string | null;
  readonly tree_id: string | null;
  readonly commit_id: string | null;
  readonly resulting_head: string | null;
  readonly object_ids: readonly string[];
  readonly author: CommitIdentity;
  readonly committer: CommitIdentity;
  readonly confirmation_snapshot_digest: string;
  readonly confirmed_at: string;
  readonly created_at: string;
  readonly updated_at: string;
}

export type Mergeability = 'computing' | 'mergeable' | 'conflicting';
export type PullRequestState =
  'creating' | 'updating' | 'open' | 'closed_unmerged' | 'merged' | 'recovery_required';
export type PullRequestCheckSummaryState =
  'pending' | 'passing' | 'failing' | 'awaiting_maintainer_approval' | 'unavailable';

export interface PullRequestRecord {
  readonly operation: 'open' | 'update';
  readonly marker: string;
  readonly upstream_repository_id: number;
  readonly upstream_repository_node_id: string;
  readonly fork_repository_id: number;
  readonly fork_repository_node_id: string;
  readonly base_ref: string;
  readonly head_owner: string;
  readonly head_branch: string;
  readonly head_ref: string;
  readonly expected_base_revision: string;
  readonly expected_head_revision: string;
  readonly previous_head_revision: string | null;
  readonly target_number: number | null;
  readonly update_metadata: boolean;
  readonly expected_previous_title: string | null;
  readonly expected_previous_body_digest: string | null;
  readonly title: string;
  readonly body: string;
  readonly body_digest: string;
  readonly intent_digest: string;
  readonly provider_id: number | null;
  readonly provider_node_id: string | null;
  readonly number: number | null;
  readonly canonical_url: string | null;
  readonly state: PullRequestState;
  readonly base_revision: string | null;
  readonly head_revision: string | null;
  readonly observed_title: string | null;
  readonly observed_body_digest: string | null;
  readonly mergeability: Mergeability;
  readonly checks: PullRequestCheckSummaryState;
  readonly confirmation_snapshot_digest: string;
  readonly confirmed_at: string;
  readonly created_at: string;
  readonly updated_at: string;
  readonly provider_created_at: string | null;
  readonly provider_updated_at: string | null;
  /** Safe failure from the latest read-only provider status observation. */
  readonly status_failure: LaunchFailure | null;
  /** Provider correlation identifier for the latest status observation. */
  readonly provider_request_id: string | null;
  /** Whether the observed provider head still equals CMA's retained Launch head. */
  readonly head_matches_launch: boolean | null;
}

export interface PullRequestDraft extends Omit<
  PullRequestRecord,
  | 'provider_id'
  | 'provider_node_id'
  | 'number'
  | 'canonical_url'
  | 'state'
  | 'base_revision'
  | 'head_revision'
  | 'observed_title'
  | 'observed_body_digest'
  | 'mergeability'
  | 'checks'
  | 'confirmation_snapshot_digest'
  | 'confirmed_at'
  | 'created_at'
  | 'updated_at'
  | 'provider_created_at'
  | 'provider_updated_at'
  | 'status_failure'
  | 'provider_request_id'
  | 'head_matches_launch'
> {
  readonly launch_id: string;
  readonly target_provider_id: number | null;
  readonly target_provider_node_id: string | null;
}

export interface Launch {
  readonly id: string;
  readonly trigger_public_id: string;
  readonly revision_digest: string;
  readonly source_sha: string;
  readonly state: LaunchState;
  readonly revision_number: number;
  readonly revision_content: TriggerRevisionContent;
  readonly blocked_reason: LaunchBlockedReason | null;
  readonly failure: LaunchFailure | null;
  readonly resume_state: LaunchState | null;
  readonly overrides: LaunchOverrides;
  readonly effective: EffectiveConfiguration;
  readonly pricing: LaunchPricingSnapshot | null;
  readonly runtime_admission: LaunchRuntimeAdmission | null;
  readonly review_quota: LaunchQuotaSnapshot | null;
  readonly start_admission: LaunchStartAdmission | null;
  readonly connection: TriggerConnection | null;
  readonly workflow_paths: readonly string[];
  readonly workflow_risk_acknowledged: boolean;
  readonly steps: readonly LaunchStep[];
  readonly fork: ForkRecord | null;
  readonly branch: BranchRecord | null;
  readonly workspace: JsonObject | null;
  readonly session: JsonObject | null;
  readonly session_credential_retirement: LaunchSessionCredentialRetirement | null;
  readonly reach: LaunchReachStatus | null;
  readonly delivery: PromptDelivery | null;
  readonly contribution_checkpoint_version: number | null;
  readonly diff: DiffArtifact | null;
  readonly contribution: ContributionRecord | null;
  readonly pull_request: PullRequestRecord | null;
  readonly created_at: string;
  readonly updated_at: string;
  readonly draft_expires_at: string;
  readonly started_at: string | null;
  readonly terminal_at: string | null;
}

export interface LaunchProgressState {
  readonly state: LaunchState;
  readonly blocked_reason: LaunchBlockedReason | null;
  readonly failure: LaunchFailure | null;
  readonly resume_state: LaunchState | null;
  readonly reach: LaunchReachStatus | null;
  readonly delivery: PromptDelivery | null;
}

export type LaunchEventKind =
  | { readonly type: 'created'; readonly steps: readonly LaunchStep[] }
  | { readonly type: 'draft_updated' }
  | { readonly type: 'connection_selected' }
  | { readonly type: 'started' }
  | { readonly type: 'state_changed'; readonly from: LaunchState; readonly to: LaunchState }
  | { readonly type: 'step_changed'; readonly step: LaunchStep }
  | { readonly type: 'blocked'; readonly reason: LaunchBlockedReason }
  | { readonly type: 'resumed'; readonly reason: LaunchBlockedReason }
  | { readonly type: 'failure'; readonly failure: LaunchFailure }
  | { readonly type: 'effect_reconciled'; readonly effect: LaunchEffectKind }
  | { readonly type: 'recovery_authorized'; readonly effect: LaunchEffectKind }
  | { readonly type: 'delivery_changed'; readonly state: PromptDeliveryState }
  | { readonly type: 'reach_changed'; readonly reach: LaunchReachStatus }
  | {
      readonly type: 'pull_request_status_changed';
      readonly state: PullRequestState;
      readonly mergeability: Mergeability;
      readonly checks: PullRequestCheckSummaryState;
      readonly failure: LaunchFailure | null;
    }
  | { readonly type: 'pull_request_status_refresh_requested' }
  | { readonly type: 'confirmation_recorded'; readonly action: LaunchConfirmationAction }
  | { readonly type: 'cleanup_planned'; readonly target_count: number }
  | { readonly type: 'cleanup_completed'; readonly target_count: number }
  | { readonly type: 'cancelled' };

export type LaunchEffectKind =
  | 'preflight'
  | 'fork'
  | 'branch'
  | 'workspace_import'
  | 'sandbox'
  | 'session'
  | 'reach'
  | 'initial_dispatch'
  | 'checkpoint'
  | 'diff'
  | 'branch_publication'
  | 'pull_request'
  | 'cleanup';

export type LaunchConfirmationAction =
  | 'start'
  | 'workflow_risk'
  | 'review_diff'
  | 'publish_branch'
  | 'open_pull_request'
  | 'update_pull_request'
  | 'recover_prompt'
  | 'new_launch'
  | 'cancel'
  | 'complete';

export interface LaunchEvent {
  readonly sequence: number;
  readonly launch_id: string;
  readonly occurred_at: string;
  readonly correlation_id: string;
  readonly progress: LaunchProgressState;
  readonly kind: LaunchEventKind;
}

export interface LaunchProgress {
  readonly launch_id: string;
  readonly events: readonly LaunchEvent[];
  readonly next_cursor: number;
}

export interface DiffReview {
  readonly launch_id: string;
  readonly revision_number: number;
  readonly state: LaunchState;
  readonly source_sha: string;
  readonly workflow_paths: readonly string[];
  readonly artifact: DiffArtifact | null;
  readonly contribution: ContributionRecord | null;
  readonly pull_request: PullRequestRecord | null;
}

export interface LaunchListRequest {
  readonly state?: LaunchState;
  readonly trigger_public_id?: string;
  readonly session_id?: string;
  readonly limit?: number;
  readonly cursor?: string;
}

export async function readTriggerPreview(
  config: ResolvedConfig,
  publicId: string,
  options: OperationOptions = {},
): Promise<TriggerPreview> {
  const anonymous = { ...config, credential: undefined, cookies: false };
  return decodeTriggerPreview(
    await request(
      anonymous,
      `/api/v1/triggers/public/${identifier(publicId, 'public Trigger id')}`,
      { ...options, expectedStatus: 200 },
    ),
  );
}

export async function createTrigger(
  config: ResolvedConfig,
  selectedServiceId: string,
  content: TriggerContentDraft,
  options: TriggerCommandOptions,
): Promise<TriggerDraft> {
  return decodeTriggerDraft(
    await command(
      config,
      '/api/v1/triggers',
      'POST',
      { selected_service_id: selectedServiceId, content },
      201,
      options,
    ),
  );
}

export async function listTriggers(
  config: ResolvedConfig,
  limit = 50,
  cursor?: string,
  options: OperationOptions = {},
): Promise<CursorPage<TriggerDraft>> {
  return decodeCursorPage(
    await request(config, cursorTarget('/api/v1/triggers', limit, cursor), {
      ...options,
      expectedStatus: 200,
    }),
    decodeTriggerDraft,
    'Trigger drafts',
  );
}

export async function readTrigger(
  config: ResolvedConfig,
  triggerId: string,
  options: OperationOptions = {},
): Promise<TriggerDraft> {
  return decodeTriggerDraft(
    await request(config, `/api/v1/triggers/${identifier(triggerId, 'Trigger id')}`, {
      ...options,
      expectedStatus: 200,
    }),
  );
}

export async function updateTrigger(
  config: ResolvedConfig,
  triggerId: string,
  expectedVersion: number,
  selectedServiceId: string,
  content: TriggerContentDraft,
  options: TriggerCommandOptions,
): Promise<TriggerDraft> {
  return decodeTriggerDraft(
    await command(
      config,
      `/api/v1/triggers/${identifier(triggerId, 'Trigger id')}`,
      'PUT',
      { expected_version: expectedVersion, selected_service_id: selectedServiceId, content },
      200,
      options,
    ),
  );
}

export async function publishTrigger(
  config: ResolvedConfig,
  triggerId: string,
  expectedVersion: number,
  selectedServiceId: string,
  options: TriggerCommandOptions,
): Promise<TriggerRevision> {
  return decodeTriggerRevision(
    await command(
      config,
      `/api/v1/triggers/${identifier(triggerId, 'Trigger id')}/publish`,
      'POST',
      { expected_version: expectedVersion, selected_service_id: selectedServiceId },
      201,
      options,
    ),
  );
}

export async function listTriggerRevisions(
  config: ResolvedConfig,
  limit = 50,
  cursor?: string,
  options: OperationOptions = {},
): Promise<CursorPage<TriggerRevision>> {
  return decodeCursorPage(
    await request(config, cursorTarget('/api/v1/trigger-revisions', limit, cursor), {
      ...options,
      expectedStatus: 200,
    }),
    decodeTriggerRevision,
    'Trigger revisions',
  );
}

export async function readTriggerRevision(
  config: ResolvedConfig,
  revisionId: string,
  options: OperationOptions = {},
): Promise<TriggerRevision> {
  return decodeTriggerRevision(
    await request(
      config,
      `/api/v1/trigger-revisions/${identifier(revisionId, 'Trigger revision id')}`,
      { ...options, expectedStatus: 200 },
    ),
  );
}

export async function retireTriggerRevision(
  config: ResolvedConfig,
  revisionId: string,
  options: TriggerCommandOptions,
): Promise<TriggerRevision> {
  return decodeTriggerRevision(
    await command(
      config,
      `/api/v1/trigger-revisions/${identifier(revisionId, 'Trigger revision id')}/retire`,
      'POST',
      {},
      200,
      options,
    ),
  );
}

export async function listTriggerConnections(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<readonly TriggerConnection[]> {
  const body = safeObject(
    await request(config, '/api/v1/trigger-connections', { ...options, expectedStatus: 200 }),
    'Trigger connections',
  );
  if (!Array.isArray(body.connections))
    throw new ProtocolError('Trigger connections omitted connections');
  return body.connections.map(decodeConnection);
}

export async function beginTriggerConnection(
  config: ResolvedConfig,
  intentId: string,
  options: TriggerCommandOptions,
): Promise<ConnectionAuthorization> {
  return decodeConnectionAuthorization(
    await command(
      config,
      '/api/v1/trigger-connections',
      'POST',
      { intent_id: intentId },
      200,
      options,
    ),
  );
}

export async function reconnectTriggerConnection(
  config: ResolvedConfig,
  serviceId: string,
  options: TriggerCommandOptions,
): Promise<ConnectionAuthorization> {
  return decodeConnectionAuthorization(
    await command(
      config,
      `/api/v1/trigger-connections/${identifier(serviceId, 'service id')}/reconnect`,
      'POST',
      {},
      200,
      options,
    ),
  );
}

/** Revoke one exact reusable NyxID service without deleting Launch history. */
export async function revokeTriggerConnection(
  config: ResolvedConfig,
  serviceId: string,
  options: TriggerCommandOptions,
): Promise<TriggerConnection> {
  const connection = decodeConnection(
    await command(
      config,
      `/api/v1/trigger-connections/${identifier(serviceId, 'service id')}`,
      'DELETE',
      {},
      200,
      options,
    ),
  );
  if (connection.service_id !== serviceId) {
    throw new ProtocolError('CMA returned revocation state for a different GitHub connection.');
  }
  return connection;
}

export async function preflightTriggerConnection(
  config: ResolvedConfig,
  serviceId: string,
  options: OperationOptions = {},
): Promise<TriggerConnection> {
  return decodeConnection(
    await request(
      config,
      `/api/v1/trigger-connections/${identifier(serviceId, 'service id')}/preflight`,
      { ...options, expectedStatus: 200 },
    ),
  );
}

export async function resolveTriggerRepository(
  config: ResolvedConfig,
  serviceId: string,
  owner: string,
  name: string,
  options: OperationOptions = {},
): Promise<TriggerRepositorySelection> {
  const params = new URLSearchParams({
    owner: repositoryLocator(owner, 'repository owner'),
    name: repositoryLocator(name, 'repository name'),
  });
  return decodeRepositorySelection(
    await request(
      config,
      `/api/v1/trigger-connections/${identifier(serviceId, 'service id')}/repositories/resolve?${params.toString()}`,
      { ...options, expectedStatus: 200 },
    ),
  );
}

export async function createOrResumeLaunch(
  config: ResolvedConfig,
  publicId: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `/api/v1/triggers/public/${identifier(publicId, 'public Trigger id')}/launches`,
      'POST',
      {},
      201,
      options,
    ),
  );
}

export async function createNewLaunch(
  config: ResolvedConfig,
  publicId: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `/api/v1/triggers/public/${identifier(publicId, 'public Trigger id')}/launches/new`,
      'POST',
      { acknowledge_additional_resources: true },
      201,
      options,
    ),
  );
}

export async function listLaunches(
  config: ResolvedConfig,
  page: LaunchListRequest = {},
  options: OperationOptions = {},
): Promise<CursorPage<Launch>> {
  const params = new URLSearchParams();
  if (page.state !== undefined) params.set('state', page.state);
  if (page.trigger_public_id !== undefined) params.set('trigger_public_id', page.trigger_public_id);
  if (page.session_id !== undefined) {
    identifier(page.session_id, 'session id');
    params.set('session_id', page.session_id);
  }
  params.set('limit', String(pageLimit(page.limit ?? 50, 'Launch')));
  if (page.cursor !== undefined) params.set('cursor', page.cursor);
  return decodeCursorPage(
    await request(config, `/api/v1/launches?${params.toString()}`, {
      ...options,
      expectedStatus: 200,
    }),
    decodeLaunch,
    'Launches',
    [['items', '*', 'pull_request', 'provider_request_id']],
  );
}

export async function readLaunch(
  config: ResolvedConfig,
  launchId: string,
  options: OperationOptions = {},
): Promise<Launch> {
  return decodeLaunch(
    await request(config, launchPath(launchId), { ...options, expectedStatus: 200 }),
  );
}

export async function updateLaunchOverrides(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  overrides: LaunchOverrides,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/overrides`,
      'PATCH',
      { expected_revision: expectedRevision, overrides },
      200,
      options,
    ),
  );
}

export async function selectLaunchConnection(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  serviceId: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/connection`,
      'PUT',
      { expected_revision: expectedRevision, service_id: serviceId },
      200,
      options,
    ),
  );
}

export async function startLaunch(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  snapshotDigest: string,
  acknowledgeWorkflowRisk: boolean,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/start`,
      'POST',
      {
        expected_revision: expectedRevision,
        snapshot_digest: snapshotDigest,
        acknowledge_workflow_risk: acknowledgeWorkflowRisk,
      },
      202,
      options,
    ),
  );
}

export async function recoverInitialPrompt(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  snapshotDigest: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/recover-initial-prompt`,
      'POST',
      {
        expected_revision: expectedRevision,
        snapshot_digest: snapshotDigest,
        acknowledge_duplicate_execution_risk: true,
      },
      202,
      options,
    ),
  );
}

export async function requestLaunchDiff(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  snapshotDigest: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/contributions/review`,
      'POST',
      { expected_revision: expectedRevision, snapshot_digest: snapshotDigest },
      202,
      options,
    ),
  );
}

export async function readLaunchDiff(
  config: ResolvedConfig,
  launchId: string,
  options: OperationOptions = {},
): Promise<DiffReview> {
  return decodeDiffReview(
    await request(config, `${launchPath(launchId)}/diff`, { ...options, expectedStatus: 200 }),
  );
}

export async function publishLaunchBranch(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  snapshotDigest: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/contributions/publish`,
      'POST',
      { expected_revision: expectedRevision, snapshot_digest: snapshotDigest },
      202,
      options,
    ),
  );
}

export async function reviewPullRequest(
  config: ResolvedConfig,
  launchId: string,
  operation: 'open' | 'update',
  edits: { readonly title?: string; readonly body?: string } = {},
  options: OperationOptions = {},
): Promise<PullRequestDraft> {
  return decodePullRequestDraft(
    await request(config, `${launchPath(launchId)}/pull-request/review`, {
      ...options,
      method: 'POST',
      body: { operation, ...edits },
      expectedStatus: 200,
    }),
  );
}

export async function confirmPullRequest(
  config: ResolvedConfig,
  launchId: string,
  operation: 'open' | 'update',
  expectedRevision: number,
  snapshotDigest: string,
  edits: { readonly title?: string; readonly body?: string } = {},
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/pull-request/${operation}`,
      'POST',
      { expected_revision: expectedRevision, snapshot_digest: snapshotDigest, ...edits },
      202,
      options,
    ),
  );
}

/** Queue an immediate read-only provider status observation for an open pull request. */
export async function refreshPullRequestStatus(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/pull-request/refresh`,
      'POST',
      { expected_revision: expectedRevision },
      202,
      options,
    ),
  );
}

export async function cancelLaunch(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  snapshotDigest: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/cancel`,
      'POST',
      { expected_revision: expectedRevision, snapshot_digest: snapshotDigest },
      200,
      options,
    ),
  );
}

/** Finish CMA orchestration after handing an open pull request to its maintainers. */
export async function completeLaunch(
  config: ResolvedConfig,
  launchId: string,
  expectedRevision: number,
  snapshotDigest: string,
  options: TriggerCommandOptions,
): Promise<Launch> {
  return decodeLaunch(
    await command(
      config,
      `${launchPath(launchId)}/complete`,
      'POST',
      { expected_revision: expectedRevision, snapshot_digest: snapshotDigest },
      200,
      options,
    ),
  );
}

export async function readLaunchProgress(
  config: ResolvedConfig,
  launchId: string,
  after = 0,
  limit = 100,
  options: OperationOptions = {},
): Promise<LaunchProgress> {
  if (
    !Number.isSafeInteger(after) ||
    after < 0 ||
    !Number.isSafeInteger(limit) ||
    limit < 1 ||
    limit > 500
  )
    throw new ValidationError(
      'Launch progress cursor must be non-negative and limit must be from 1 through 500',
    );
  return decodeLaunchProgress(
    await request(
      config,
      `${launchPath(launchId)}/events?after=${String(after)}&limit=${String(limit)}`,
      { ...options, expectedStatus: 200 },
    ),
    launchId,
    after,
  );
}

export async function downloadDiffArtifact(
  config: ResolvedConfig,
  launchId: string,
  artifactId: string,
  options: OperationOptions = {},
): Promise<Uint8Array> {
  return requestBytes(
    config,
    `${launchPath(launchId)}/diff-artifacts/${identifier(artifactId, 'diff artifact id')}/download`,
    { ...options, expectedStatus: 200 },
  );
}

async function command(
  config: ResolvedConfig,
  path: string,
  method: string,
  body: unknown,
  expectedStatus: number,
  options: TriggerCommandOptions,
): Promise<unknown> {
  const { idempotencyKey, ...operation } = options;
  validateIdempotencyKey(idempotencyKey);
  return request(config, path, {
    ...operation,
    method,
    body,
    expectedStatus,
    headers: { 'Idempotency-Key': idempotencyKey },
  });
}

function decodeTriggerPreview(raw: unknown): TriggerPreview {
  const body = safeObject(raw, 'Trigger preview');
  requireText(body, 'public_id', 'Trigger preview');
  const availability = requireEnum(
    body,
    'availability',
    TRIGGER_PREVIEW_AVAILABILITIES,
    'Trigger preview',
  );
  if (!('unavailable_reason' in body)) {
    throw new ProtocolError('Trigger preview omitted unavailable_reason');
  }
  if (availability === 'unavailable') {
    requireEnum(body, 'unavailable_reason', TRIGGER_PREVIEW_UNAVAILABLE_REASONS, 'Trigger preview');
  } else if (body.unavailable_reason !== null) {
    throw new ProtocolError(
      'Trigger preview carried an unavailable_reason while it was available or retired',
    );
  }
  requireText(body, 'publisher_display_identity', 'Trigger preview');
  requireInteger(body, 'repository_id', 'Trigger preview');
  requireText(body, 'repository_owner', 'Trigger preview');
  requireText(body, 'repository_name', 'Trigger preview');
  requireEnum(body, 'visibility', REPOSITORY_VISIBILITIES, 'Trigger preview');
  requireBoolean(body, 'allow_forking', 'Trigger preview');
  requireText(body, 'public_summary', 'Trigger preview');
  requireEnum(body, 'permission_ceiling', PERMISSION_CEILINGS, 'Trigger preview');
  requireEnumArray(body, 'required_capabilities', REQUIRED_CAPABILITIES, 'Trigger preview');
  decodeSessionSize(body.resource_default, 'Trigger preview resource_default');
  decodeSessionSize(body.resource_ceiling, 'Trigger preview resource_ceiling');
  requireInteger(body, 'timeout_default_seconds', 'Trigger preview');
  requireInteger(body, 'timeout_ceiling_seconds', 'Trigger preview');
  requireEnum(body, 'billing_mode', BILLING_MODES, 'Trigger preview');
  requireText(body, 'pricing_policy_reference', 'Trigger preview');
  decodeTriggerPreviewCost(body.estimated_cost);
  requireText(body, 'revision_digest', 'Trigger preview');
  return body as unknown as TriggerPreview;
}

function decodeTriggerPreviewCost(raw: unknown): TriggerPreviewCost {
  const cost = safeObject(raw, 'Trigger preview estimated cost');
  const kind = requireEnum(
    cost,
    'kind',
    TRIGGER_PREVIEW_COST_KINDS,
    'Trigger preview estimated cost',
  );
  if (kind === 'quote_unavailable') return cost as unknown as TriggerPreviewCost;

  const currency = requireText(cost, 'currency', 'Trigger preview estimated cost');
  if (currency !== 'USD') {
    throw new ProtocolError('Trigger preview estimated cost carried an invalid currency');
  }
  const amount = requireInteger(cost, 'amount_micros', 'Trigger preview estimated cost');
  requireText(cost, 'display', 'Trigger preview estimated cost');
  if (kind === 'exact_zero') {
    if (amount !== 0) {
      throw new ProtocolError('Trigger preview exact-zero cost carried a non-zero amount');
    }
    return cost as unknown as TriggerPreviewCost;
  }
  requireText(cost, 'quoted_at', 'Trigger preview estimated cost');
  return cost as unknown as TriggerPreviewCost;
}

function decodeTriggerDraft(raw: unknown): TriggerDraft {
  const body = safeObject(raw, 'Trigger draft');
  requireText(body, 'id', 'Trigger draft');
  requireText(body, 'selected_service_id', 'Trigger draft');
  requireInteger(body, 'version', 'Trigger draft');
  requireInteger(body, 'next_revision_number', 'Trigger draft');
  decodeTriggerRevisionContent(body.content, 'Trigger draft content');
  requireText(body, 'created_at', 'Trigger draft');
  requireText(body, 'updated_at', 'Trigger draft');
  return body as unknown as TriggerDraft;
}

function decodeTriggerRevision(raw: unknown): TriggerRevision {
  const body = safeObject(raw, 'Trigger revision');
  requireText(body, 'id', 'Trigger revision');
  requireText(body, 'public_id', 'Trigger revision');
  requireText(body, 'digest', 'Trigger revision');
  requireEnum(body, 'availability', TRIGGER_AVAILABILITIES, 'Trigger revision');
  decodeTriggerRevisionContent(body.content, 'Trigger revision content');
  const authority = safeObject(body.authority, 'Trigger publisher authority');
  requireText(authority, 'service_id', 'Trigger publisher authority');
  requireInteger(authority, 'service_state_version', 'Trigger publisher authority');
  requireInteger(authority, 'account_id', 'Trigger publisher authority');
  requireText(authority, 'account_login', 'Trigger publisher authority');
  requireEnum(authority, 'role', PUBLISHER_ROLES, 'Trigger publisher authority');
  requireText(authority, 'verified_at', 'Trigger publisher authority');
  requireText(body, 'published_at', 'Trigger revision');
  requireNullableText(body, 'retired_at', 'Trigger revision');
  return body as unknown as TriggerRevision;
}

function decodeConnection(raw: unknown): TriggerConnection {
  const body = safeObject(raw, 'Trigger connection');
  requireText(body, 'service_id', 'Trigger connection');
  requireInteger(body, 'state_version', 'Trigger connection');
  requireEnum(body, 'state', CONNECTION_STATES, 'Trigger connection');
  requireNullableInteger(body, 'account_id', 'Trigger connection');
  requireNullableText(body, 'account_node_id', 'Trigger connection');
  requireNullableText(body, 'account_login', 'Trigger connection');
  requireNullableText(body, 'avatar_url', 'Trigger connection');
  requireTextArray(body, 'granted_scopes', 'Trigger connection');
  requireNullableText(body, 'consent_observed_at', 'Trigger connection');
  return body as unknown as TriggerConnection;
}

function decodeConnectionAuthorization(raw: unknown): ConnectionAuthorization {
  const body = safeObject(raw, 'connection authorization');
  requireText(body, 'service_id', 'connection authorization');
  requireText(body, 'authorization_url', 'connection authorization');
  requireText(body, 'attempt_nonce', 'connection authorization');
  return body as unknown as ConnectionAuthorization;
}

function decodeRepositorySelection(raw: unknown): TriggerRepositorySelection {
  const body = safeObject(raw, 'Trigger repository selection');
  requireText(body, 'source_sha', 'Trigger repository selection');
  const repository = safeObject(body.repository, 'Trigger repository identity');
  requireInteger(repository, 'repository_id', 'Trigger repository identity');
  requireInteger(repository, 'owner_id', 'Trigger repository identity');
  requireText(repository, 'repository_node_id', 'Trigger repository identity');
  requireText(repository, 'owner_node_id', 'Trigger repository identity');
  requireText(repository, 'canonical_owner', 'Trigger repository identity');
  requireText(repository, 'canonical_name', 'Trigger repository identity');
  requireEnum(repository, 'visibility', REPOSITORY_VISIBILITIES, 'Trigger repository identity');
  requireBoolean(repository, 'allow_forking', 'Trigger repository identity');
  requireText(repository, 'default_branch', 'Trigger repository identity');
  requireText(repository, 'selected_ref', 'Trigger repository identity');
  for (const optional of ['fork_parent_id', 'fork_source_id']) {
    if (repository[optional] !== undefined)
      requireInteger(repository, optional, 'Trigger repository identity');
  }
  return body as unknown as TriggerRepositorySelection;
}

export function decodeLaunch(raw: unknown): Launch {
  const body = safeObject(raw, 'Launch', [['pull_request', 'provider_request_id']]);
  requireText(body, 'id', 'Launch');
  requireText(body, 'trigger_public_id', 'Launch');
  requireText(body, 'revision_digest', 'Launch');
  requireEnum(body, 'state', LAUNCH_STATES, 'Launch');
  requireInteger(body, 'revision_number', 'Launch');
  requireText(body, 'draft_expires_at', 'Launch');
  if (!Array.isArray(body.steps)) throw new ProtocolError('Launch steps was not an array');
  for (const step of body.steps) decodeLaunchStep(step);
  decodeTriggerRevisionContent(body.revision_content, 'Launch revision content');
  const effective = safeObject(body.effective, 'Launch effective configuration');
  requireEnum(effective, 'permission', PERMISSION_CEILINGS, 'Launch effective configuration');
  if (body.blocked_reason !== undefined && body.blocked_reason !== null) {
    requireEnumValue(body.blocked_reason, BLOCKED_REASONS, 'Launch blocked_reason');
  }
  if (body.failure !== undefined && body.failure !== null) decodeLaunchFailure(body.failure);
  if (body.resume_state !== undefined && body.resume_state !== null) {
    requireEnumValue(body.resume_state, LAUNCH_STATES, 'Launch resume_state');
  }
  if (body.pricing !== null) {
    const pricing = safeObject(body.pricing, 'Launch pricing');
    requireText(pricing, 'policy_reference', 'Launch pricing');
    requireEnum(pricing, 'billing_mode', BILLING_MODES, 'Launch pricing');
    requireText(pricing, 'currency', 'Launch pricing');
    requireInteger(pricing, 'cpu_millis', 'Launch pricing');
    requireInteger(pricing, 'memory_mib', 'Launch pricing');
    requireInteger(pricing, 'micros_per_hour', 'Launch pricing');
    requireInteger(pricing, 'estimated_micros', 'Launch pricing');
    requireText(pricing, 'quoted_at', 'Launch pricing');
    requireText(pricing, 'expires_at', 'Launch pricing');
    if (pricing.rate !== undefined) {
      const rate = safeObject(pricing.rate, 'Launch pricing rate');
      requireInteger(rate, 'cpu_micros_per_cpu_hour', 'Launch pricing rate');
      requireInteger(rate, 'memory_micros_per_gib_hour', 'Launch pricing rate');
    }
  }
  if (body.runtime_admission !== null) {
    const runtime = safeObject(body.runtime_admission, 'Launch runtime admission');
    requireEnum(runtime, 'kernel', TRIGGER_KERNELS, 'Launch runtime admission');
    requireText(runtime, 'runtime_version', 'Launch runtime admission');
    requireText(runtime, 'observed_at', 'Launch runtime admission');
    if (!Array.isArray(runtime.supported_capabilities))
      throw new ProtocolError('Launch runtime supported_capabilities was not an array');
    for (const capability of runtime.supported_capabilities) {
      requireEnumValue(capability, REQUIRED_CAPABILITIES, 'Launch runtime capability');
    }
  }
  if (body.review_quota !== null) decodeLaunchQuota(body.review_quota, 'Launch review quota');
  if (body.start_admission !== null) {
    const start = safeObject(body.start_admission, 'Launch Start admission');
    requireText(start, 'runtime_version', 'Launch Start admission');
    requireText(start, 'pricing_policy_reference', 'Launch Start admission');
    requireText(start, 'checked_at', 'Launch Start admission');
    decodeLaunchQuota(start.quota, 'Launch Start quota');
  }
  if (body.delivery !== undefined && body.delivery !== null) decodePromptDelivery(body.delivery);
  if (body.connection !== undefined && body.connection !== null) decodeConnection(body.connection);
  if (body.session_credential_retirement !== null) {
    decodeLaunchSessionCredentialRetirement(body.session_credential_retirement);
  }
  if (body.reach !== undefined && body.reach !== null) decodeLaunchReach(body.reach);
  if (body.diff !== undefined && body.diff !== null) decodeDiffArtifact(body.diff);
  if (body.contribution !== undefined && body.contribution !== null) {
    decodeContributionRecord(body.contribution);
  }
  if (body.pull_request !== undefined && body.pull_request !== null) {
    decodePullRequestRecord(body.pull_request);
  }
  return body as unknown as Launch;
}

function decodeTriggerRevisionContent(raw: unknown, label: string): TriggerRevisionContent {
  const content = safeObject(raw, label);
  requireInteger(content, 'schema_version', label);
  requireText(content, 'trigger_id', label);
  requireInteger(content, 'revision_number', label);
  requireText(content, 'publisher_subject_id', label);
  requireText(content, 'publisher_display_identity', label);

  const upstream = safeObject(content.upstream, `${label} upstream`);
  decodeRepositoryIdentity(upstream.repository, `${label} upstream repository`);
  requireEnum(upstream, 'source_mode', SOURCE_MODES, `${label} upstream`);
  if (upstream.observed_source_sha !== undefined) {
    requireText(upstream, 'observed_source_sha', `${label} upstream`);
  }
  requireText(upstream, 'contribution_base_ref', `${label} upstream`);
  decodeTriggerTask(content.task, `${label} task`);
  decodeTriggerSessionPolicy(content.session, `${label} session`);
  decodeRepositoryAccessPolicy(content.repository_access, `${label} repository access`);
  decodeSetupPolicy(content.setup, `${label} setup`);
  decodeContributionPolicy(content.contribution, `${label} contribution`);
  decodeCommercialPolicy(content.commercial_policy, `${label} commercial policy`);
  decodeOverridePolicy(content.override_policy, `${label} override policy`);
  const projection = safeObject(content.public_projection, `${label} public projection`);
  requireInteger(projection, 'version', `${label} public projection`);
  return content as unknown as TriggerRevisionContent;
}

function decodeRepositoryIdentity(raw: unknown, label: string): RepositoryIdentity {
  const repository = safeObject(raw, label);
  requireInteger(repository, 'repository_id', label);
  requireText(repository, 'repository_node_id', label);
  requireInteger(repository, 'owner_id', label);
  requireText(repository, 'owner_node_id', label);
  requireText(repository, 'canonical_owner', label);
  requireText(repository, 'canonical_name', label);
  requireEnum(repository, 'visibility', REPOSITORY_VISIBILITIES, label);
  requireBoolean(repository, 'allow_forking', label);
  requireText(repository, 'default_branch', label);
  requireText(repository, 'selected_ref', label);
  for (const optional of ['fork_parent_id', 'fork_source_id']) {
    if (repository[optional] !== undefined) requireInteger(repository, optional, label);
  }
  return repository as unknown as RepositoryIdentity;
}

function decodeSessionSize(raw: unknown, label: string): SessionSize {
  const size = safeObject(raw, label);
  requireInteger(size, 'cpu_millis', label);
  requireInteger(size, 'memory_mib', label);
  return size as unknown as SessionSize;
}

function decodeTriggerTask(raw: unknown, label: string): TriggerTask {
  const task = safeObject(raw, label);
  requireText(task, 'public_summary', label);
  requireText(task, 'prompt_template', label);
  requireInteger(task, 'renderer_version', label);
  const fields = requireArray(task, 'input_fields', label);
  for (const rawField of fields) {
    const field = safeObject(rawField, `${label} input field`);
    requireText(field, 'key', `${label} input field`);
    decodeTriggerInputKind(field.kind, `${label} input kind`);
    requireBoolean(field, 'required', `${label} input field`);
    requireBoolean(field, 'editable', `${label} input field`);
  }
  return task as unknown as TriggerTask;
}

function decodeTriggerInputKind(raw: unknown, label: string): TriggerInputFieldKind {
  if (raw === 'boolean') return raw;
  const kind = safeObject(raw, label);
  const variants = ['text', 'integer', 'choice'].filter((variant) => variant in kind);
  if (variants.length !== 1 || Object.keys(kind).length !== 1) {
    throw new ProtocolError(`${label} was invalid`);
  }
  switch (variants[0]) {
    case 'text': {
      const bounds = safeObject(kind.text, `${label} text bounds`);
      requireInteger(bounds, 'min_bytes', `${label} text bounds`);
      requireInteger(bounds, 'max_bytes', `${label} text bounds`);
      break;
    }
    case 'integer': {
      const bounds = safeObject(kind.integer, `${label} integer bounds`);
      requireSafeInteger(bounds, 'minimum', `${label} integer bounds`);
      requireSafeInteger(bounds, 'maximum', `${label} integer bounds`);
      break;
    }
    case 'choice': {
      const choice = safeObject(kind.choice, `${label} choice values`);
      requireTextArray(choice, 'values', `${label} choice values`);
      break;
    }
  }
  return kind as unknown as TriggerInputFieldKind;
}

function decodeTriggerSessionPolicy(raw: unknown, label: string): TriggerSessionPolicy {
  const session = safeObject(raw, label);
  requireEnumArray(session, 'required_capabilities', REQUIRED_CAPABILITIES, label);
  requireEnum(session, 'permission_ceiling', PERMISSION_CEILINGS, label);
  requireText(session, 'sandbox_profile', label);
  const resources = safeObject(session.resources, `${label} resources`);
  decodeSessionSize(resources.default, `${label} default resources`);
  decodeSessionSize(resources.ceiling, `${label} resource ceiling`);
  requireInteger(session, 'timeout_default_seconds', label);
  requireInteger(session, 'timeout_ceiling_seconds', label);
  return session as unknown as TriggerSessionPolicy;
}

function decodeRepositoryAccessPolicy(raw: unknown, label: string): TriggerRepositoryAccessPolicy {
  const policy = safeObject(raw, label);
  requireEnum(policy, 'credential_route', CREDENTIAL_ROUTES, label);
  requireEnumArray(policy, 'required_scope_classes', SCOPE_CLASSES, label);
  requireEnumArray(policy, 'allowed_operation_classes', REPOSITORY_OPERATION_CLASSES, label);
  requireEnum(policy, 'workflow_change_policy', WORKFLOW_CHANGE_POLICIES, label);
  return policy as unknown as TriggerRepositoryAccessPolicy;
}

function decodeSetupPolicy(raw: unknown, label: string): TriggerSetupPolicy {
  const setup = safeObject(raw, label);
  requireEnum(setup, 'executor_policy', ['disabled'] as const, label);
  requireTextArray(setup, 'steps', label);
  return setup as unknown as TriggerSetupPolicy;
}

function decodeContributionPolicy(raw: unknown, label: string): TriggerContributionPolicy {
  const contribution = safeObject(raw, label);
  requireText(contribution, 'branch_prefix', label);
  requireEnum(contribution, 'commit_attribution', ['contributor_no_reply'] as const, label);
  const pullRequest = safeObject(contribution.pull_request, `${label} pull request`);
  requireText(pullRequest, 'title_template', `${label} pull request`);
  requireText(pullRequest, 'body_template', `${label} pull request`);
  return contribution as unknown as TriggerContributionPolicy;
}

function decodeCommercialPolicy(raw: unknown, label: string): TriggerCommercialPolicy {
  const policy = safeObject(raw, label);
  requireEnum(policy, 'billing_mode', BILLING_MODES, label);
  requireText(policy, 'policy_reference', label);
  return policy as unknown as TriggerCommercialPolicy;
}

function decodeOverridePolicy(raw: unknown, label: string): TriggerOverridePolicy {
  const policy = safeObject(raw, label);
  requireEnumArray(policy, 'editable_fields', EDITABLE_FIELDS, label);
  const optionalChecks = requireArray(policy, 'optional_checks', label);
  if (optionalChecks.length !== 0)
    throw new ProtocolError(`${label} optional_checks was not empty`);
  return policy as unknown as TriggerOverridePolicy;
}

function decodeLaunchStep(raw: unknown): LaunchStep {
  const step = safeObject(raw, 'Launch step');
  requireEnum(step, 'key', STEP_KEYS, 'Launch step');
  requireEnum(step, 'status', STEP_STATUSES, 'Launch step');
  requireInteger(step, 'sequence', 'Launch step');
  requireInteger(step, 'attempt', 'Launch step');
  return step as unknown as LaunchStep;
}

function decodeLaunchFailure(raw: unknown): LaunchFailure {
  const failure = safeObject(raw, 'Launch failure');
  requireEnum(failure, 'class', FAILURE_CLASSES, 'Launch failure');
  requireText(failure, 'code', 'Launch failure');
  requireText(failure, 'correlation_id', 'Launch failure');
  return failure as unknown as LaunchFailure;
}

function decodeLaunchQuota(raw: unknown, label: string): LaunchQuotaSnapshot {
  const quota = safeObject(raw, label);
  requireInteger(quota, 'live_sessions', label);
  requireInteger(quota, 'reserved_cpu_millis', label);
  requireInteger(quota, 'reserved_memory_mib', label);
  for (const optional of ['live_session_limit', 'free_cpu_millis', 'free_memory_mib']) {
    if (quota[optional] !== undefined) requireInteger(quota, optional, label);
  }
  requireText(quota, 'checked_at', label);
  return quota as unknown as LaunchQuotaSnapshot;
}

function decodePromptDelivery(raw: unknown): PromptDelivery {
  const body = safeObject(raw, 'Prompt delivery');
  requireText(body, 'launch_id', 'Prompt delivery');
  requireText(body, 'session_id', 'Prompt delivery');
  requireText(body, 'revision_digest', 'Prompt delivery');
  requireText(body, 'origin_key', 'Prompt delivery');
  requireText(body, 'payload_hash', 'Prompt delivery');
  requireEnum(body, 'state', PROMPT_DELIVERY_STATES, 'Prompt delivery');
  requireInteger(body, 'attempt', 'Prompt delivery');
  requireInteger(body, 'recovery_generation', 'Prompt delivery');
  if (body.dispatch_fence_sequence !== null)
    requireInteger(body, 'dispatch_fence_sequence', 'Prompt delivery');
  requireText(body, 'queued_at', 'Prompt delivery');
  requireText(body, 'updated_at', 'Prompt delivery');
  return body as unknown as PromptDelivery;
}

function decodeLaunchReach(raw: unknown): LaunchReachStatus {
  const reach = safeObject(raw, 'Launch reach status');
  requireEnum(reach, 'state', CAPABILITY_REVOCATION_STATES, 'Launch reach status');
  if (reach.last_refusal !== undefined) {
    requireEnumValue(reach.last_refusal, CAPABILITY_REFUSALS, 'Launch reach last_refusal');
  }
  return reach as unknown as LaunchReachStatus;
}

function decodeLaunchSessionCredentialRetirement(raw: unknown): LaunchSessionCredentialRetirement {
  const retirement = safeObject(raw, 'Launch session credential retirement');
  requireText(retirement, 'session_id', 'Launch session credential retirement');
  requireTimestamp(retirement, 'settled_at', 'Launch session credential retirement');
  return retirement as unknown as LaunchSessionCredentialRetirement;
}

function decodeDiffArtifact(raw: unknown): DiffArtifact {
  const artifact = safeObject(raw, 'Launch diff artifact');
  requireEnum(artifact, 'validation', DIFF_VALIDATION_OUTCOMES, 'Launch diff artifact');
  if (!Array.isArray(artifact.files)) {
    throw new ProtocolError('Launch diff artifact files was not an array');
  }
  for (const rawFile of artifact.files) {
    const file = safeObject(rawFile, 'Launch diff file');
    requireEnum(file, 'change', DIFF_CHANGE_KINDS, 'Launch diff file');
    const binary = requireBoolean(file, 'binary', 'Launch diff file');
    const additions = requireNullableInteger(file, 'additions', 'Launch diff file');
    const deletions = requireNullableInteger(file, 'deletions', 'Launch diff file');
    if (
      binary ? additions !== null || deletions !== null : additions === null || deletions === null
    ) {
      throw new ProtocolError(
        'Launch diff file carried line statistics inconsistent with its binary state',
      );
    }
  }
  return artifact as unknown as DiffArtifact;
}

function decodeContributionRecord(raw: unknown): ContributionRecord {
  const contribution = safeObject(raw, 'Launch contribution');
  requireEnum(contribution, 'state', PUBLICATION_STATES, 'Launch contribution');
  return contribution as unknown as ContributionRecord;
}

function decodePullRequestRecord(raw: unknown): PullRequestRecord {
  const pullRequest = safeObject(raw, 'Launch pull request', [['provider_request_id']]);
  requireEnum(pullRequest, 'operation', PULL_REQUEST_OPERATIONS, 'Launch pull request');
  requireEnum(pullRequest, 'state', PULL_REQUEST_STATES, 'Launch pull request');
  requireEnum(pullRequest, 'mergeability', MERGEABILITIES, 'Launch pull request');
  requireEnum(pullRequest, 'checks', CHECK_SUMMARY_STATES, 'Launch pull request');
  for (const nullable of ['status_failure', 'provider_request_id', 'head_matches_launch']) {
    if (!(nullable in pullRequest)) {
      throw new ProtocolError(`Launch pull request omitted ${nullable}`);
    }
  }
  if (pullRequest.status_failure !== null) decodeLaunchFailure(pullRequest.status_failure);
  if (pullRequest.provider_request_id !== null) {
    requireText(pullRequest, 'provider_request_id', 'Launch pull request');
  }
  if (
    pullRequest.head_matches_launch !== null &&
    typeof pullRequest.head_matches_launch !== 'boolean'
  ) {
    throw new ProtocolError('Launch pull request carried an invalid head_matches_launch');
  }
  return pullRequest as unknown as PullRequestRecord;
}

function decodeDiffReview(raw: unknown): DiffReview {
  const body = safeObject(raw, 'Launch diff review', [['pull_request', 'provider_request_id']]);
  requireText(body, 'launch_id', 'Launch diff review');
  requireInteger(body, 'revision_number', 'Launch diff review');
  requireEnum(body, 'state', LAUNCH_STATES, 'Launch diff review');
  if (body.artifact !== undefined && body.artifact !== null) decodeDiffArtifact(body.artifact);
  if (body.contribution !== undefined && body.contribution !== null) {
    decodeContributionRecord(body.contribution);
  }
  if (body.pull_request !== undefined && body.pull_request !== null) {
    decodePullRequestRecord(body.pull_request);
  }
  return body as unknown as DiffReview;
}

function decodePullRequestDraft(raw: unknown): PullRequestDraft {
  const body = safeObject(raw, 'pull-request draft');
  requireText(body, 'launch_id', 'pull-request draft');
  requireEnum(body, 'operation', PULL_REQUEST_OPERATIONS, 'pull-request draft');
  requireText(body, 'intent_digest', 'pull-request draft');
  requireText(body, 'expected_head_revision', 'pull-request draft');
  return body as unknown as PullRequestDraft;
}

function decodeLaunchProgress(
  raw: unknown,
  requestedLaunchId: string,
  after: number,
): LaunchProgress {
  const body = safeObject(raw, 'Launch progress');
  const pageLaunchId = requireText(body, 'launch_id', 'Launch progress');
  if (pageLaunchId !== requestedLaunchId) {
    throw new ProtocolError(
      `Launch progress response belonged to ${pageLaunchId}, not ${requestedLaunchId}`,
    );
  }
  const nextCursor = requireInteger(body, 'next_cursor', 'Launch progress');
  if (!Array.isArray(body.events))
    throw new ProtocolError('Launch progress events was not an array');
  const accepted: LaunchEvent[] = [];
  let expectedSequence: number | null = after === Number.MAX_SAFE_INTEGER ? null : after + 1;
  for (const event of body.events) {
    const value = safeObject(event, 'Launch progress event');
    const sequence = requireInteger(value, 'sequence', 'Launch progress event');
    const eventLaunchId = requireText(value, 'launch_id', 'Launch progress event');
    if (eventLaunchId !== pageLaunchId) {
      throw new ProtocolError(
        `Launch progress event ${String(sequence)} belonged to ${eventLaunchId}, not ${pageLaunchId}`,
      );
    }
    requireText(value, 'occurred_at', 'Launch progress event');
    requireText(value, 'correlation_id', 'Launch progress event');
    decodeLaunchProgressState(value.progress);
    decodeLaunchEventKind(value.kind);

    const decoded = value as unknown as LaunchEvent;
    const previous = accepted.at(-1);
    if (previous !== undefined && sequence === previous.sequence) {
      if (jsonEqual(previous, decoded)) continue;
      throw new ProtocolError(
        `Launch progress sequence ${String(sequence)} had conflicting duplicate envelopes`,
      );
    }
    if (expectedSequence === null || sequence !== expectedSequence) {
      const problem =
        expectedSequence !== null && sequence > expectedSequence
          ? 'contained a sequence gap'
          : 'contained an out-of-order or unprovable duplicate event';
      throw new ProtocolError(
        `Launch progress ${problem}: expected ${String(expectedSequence)}, received ${String(sequence)}`,
      );
    }
    accepted.push(decoded);
    expectedSequence = sequence === Number.MAX_SAFE_INTEGER ? null : sequence + 1;
  }
  const expectedCursor = accepted.at(-1)?.sequence ?? after;
  if (nextCursor !== expectedCursor) {
    throw new ProtocolError(
      `Launch progress next_cursor ${String(nextCursor)} did not match accepted sequence ${String(expectedCursor)}`,
    );
  }
  return { ...body, events: accepted } as unknown as LaunchProgress;
}

function decodeLaunchProgressState(raw: unknown): LaunchProgressState {
  const progress = safeObject(raw, 'Launch progress state');
  requireEnum(progress, 'state', LAUNCH_STATES, 'Launch progress state');
  for (const nullable of ['blocked_reason', 'failure', 'resume_state', 'reach', 'delivery']) {
    if (!(nullable in progress)) {
      throw new ProtocolError(`Launch progress state omitted ${nullable}`);
    }
  }
  if (progress.blocked_reason !== null) {
    requireEnumValue(progress.blocked_reason, BLOCKED_REASONS, 'Launch progress blocked_reason');
  }
  if (progress.failure !== null) decodeLaunchFailure(progress.failure);
  if (progress.resume_state !== null) {
    requireEnumValue(progress.resume_state, LAUNCH_STATES, 'Launch progress resume_state');
  }
  if (progress.reach !== null) decodeLaunchReach(progress.reach);
  if (progress.delivery !== null) decodePromptDelivery(progress.delivery);
  return progress as unknown as LaunchProgressState;
}

function decodeLaunchEventKind(raw: unknown): LaunchEventKind {
  const kind = safeObject(raw, 'Launch progress event kind');
  const type = requireText(kind, 'type', 'Launch progress event kind');
  switch (type) {
    case 'created':
      if (!Array.isArray(kind.steps)) {
        throw new ProtocolError('Launch creation event omitted its initial steps');
      }
      for (const step of kind.steps) decodeLaunchStep(step);
      break;
    case 'state_changed':
      requireEnum(kind, 'from', LAUNCH_STATES, 'Launch state-changed event');
      requireEnum(kind, 'to', LAUNCH_STATES, 'Launch state-changed event');
      break;
    case 'step_changed':
      decodeLaunchStep(kind.step);
      break;
    case 'blocked':
    case 'resumed':
      requireEnum(kind, 'reason', BLOCKED_REASONS, 'Launch blocked event');
      break;
    case 'failure':
      decodeLaunchFailure(kind.failure);
      break;
    case 'effect_reconciled':
    case 'recovery_authorized':
      requireEnum(kind, 'effect', LAUNCH_EFFECT_KINDS, 'Launch effect event');
      break;
    case 'delivery_changed':
      requireEnum(kind, 'state', PROMPT_DELIVERY_STATES, 'Launch delivery event');
      break;
    case 'reach_changed':
      decodeLaunchReach(kind.reach);
      break;
    case 'pull_request_status_changed':
      requireEnum(kind, 'state', PULL_REQUEST_STATES, 'Launch pull-request status event');
      requireEnum(kind, 'mergeability', MERGEABILITIES, 'Launch pull-request status event');
      requireEnum(kind, 'checks', CHECK_SUMMARY_STATES, 'Launch pull-request status event');
      if (!('failure' in kind)) {
        throw new ProtocolError('Launch pull-request status event omitted failure');
      }
      if (kind.failure !== null) decodeLaunchFailure(kind.failure);
      break;
    case 'confirmation_recorded':
      requireEnum(kind, 'action', CONFIRMATION_ACTIONS, 'Launch confirmation event');
      break;
    case 'cleanup_planned':
    case 'cleanup_completed':
      requireInteger(kind, 'target_count', 'Launch cleanup event');
      break;
    case 'draft_updated':
    case 'connection_selected':
    case 'started':
    case 'pull_request_status_refresh_requested':
    case 'cancelled':
      break;
    default:
      throw new ProtocolError('Launch progress event kind was unknown');
  }
  return kind as unknown as LaunchEventKind;
}

function decodeCursorPage<T>(
  raw: unknown,
  decode: (value: unknown) => T,
  name: string,
  allowedForbiddenPaths: readonly (readonly string[])[] = [],
): CursorPage<T> {
  const body = safeObject(raw, name, allowedForbiddenPaths);
  if (!Array.isArray(body.items)) throw new ProtocolError(`${name} items was not an array`);
  if (body.next_cursor !== null && typeof body.next_cursor !== 'string')
    throw new ProtocolError(`${name} next_cursor was invalid`);
  return { items: body.items.map(decode), next_cursor: body.next_cursor };
}

function safeObject(
  raw: unknown,
  name: string,
  allowedForbiddenPaths: readonly (readonly string[])[] = [],
): Record<string, unknown> {
  const object = objectOf(raw, name);
  assertJsonValue(object, name, new Set());
  for (const forbidden of [
    'idempotency_key_hash',
    'intent_key',
    'provider_request_id',
    'object_reference',
    'baseline_object_reference',
    'backing_key_id',
    'worker_fencing_token',
    'fencing_counter',
    'subject_token_generation',
    'session_credential_generation',
    'session_credential_authority',
    'custody_generation',
    'provider_key_id',
    'provider_state_version',
    'grant_fingerprint',
    'checkpoint_object',
    'diff_artifact_object',
    'prompt_origin_key',
    'prompt_payload_hash',
  ]) {
    if (containsKey(object, forbidden, [], allowedForbiddenPaths)) {
      throw new ProtocolError(`${name} leaked ${forbidden}`);
    }
  }
  return object;
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw))
    throw new ProtocolError(`${name} was not an object`);
  return raw as Record<string, unknown>;
}

function assertJsonValue(value: unknown, name: string, seen: Set<object>): void {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new ProtocolError(`${name} carried a non-finite number`);
    return;
  }
  if (typeof value !== 'object') throw new ProtocolError(`${name} was not JSON`);
  if (seen.has(value)) throw new ProtocolError(`${name} was cyclic`);
  seen.add(value);
  for (const entry of Array.isArray(value) ? value : Object.values(value))
    assertJsonValue(entry, name, seen);
  seen.delete(value);
}

function containsKey(
  value: unknown,
  key: string,
  path: readonly string[],
  allowedPaths: readonly (readonly string[])[],
): boolean {
  if (typeof value !== 'object' || value === null) return false;
  if (Array.isArray(value)) {
    return value.some((entry) => containsKey(entry, key, [...path, '*'], allowedPaths));
  }
  if (Object.prototype.hasOwnProperty.call(value, key)) {
    const found = [...path, key];
    if (!allowedPaths.some((allowed) => pathsEqual(found, allowed))) return true;
  }
  return Object.entries(value).some(([name, entry]) =>
    containsKey(entry, key, [...path, name], allowedPaths),
  );
}

function pathsEqual(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((part, index) => part === right[index]);
}

function requireText(body: Record<string, unknown>, field: string, name: string): string {
  const value = body[field];
  if (typeof value !== 'string' || value.trim() === '')
    throw new ProtocolError(`${name} carried no ${field}`);
  return value;
}

function requireTimestamp(body: Record<string, unknown>, field: string, name: string): string {
  const value = requireText(body, field, name);
  const parsed = Date.parse(value);
  const canonical = Number.isNaN(parsed) ? '' : new Date(parsed).toISOString();
  if (
    !UTC_TIMESTAMP.test(value) ||
    (value !== canonical && value !== canonical.replace('.000Z', 'Z'))
  ) {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value;
}

function requireEnum<const T extends string>(
  body: Record<string, unknown>,
  field: string,
  allowed: readonly T[],
  name: string,
): T {
  const value = requireText(body, field, name);
  if (!allowed.includes(value as T)) {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value as T;
}

function requireEnumValue<const T extends string>(
  value: unknown,
  allowed: readonly T[],
  name: string,
): T {
  if (typeof value !== 'string' || !allowed.includes(value as T)) {
    throw new ProtocolError(`${name} was invalid`);
  }
  return value as T;
}

function requireEnumArray<const T extends string>(
  body: Record<string, unknown>,
  field: string,
  allowed: readonly T[],
  name: string,
): readonly T[] {
  const value = body[field];
  if (!Array.isArray(value)) {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value.map((entry) => requireEnumValue(entry, allowed, `${name} ${field}`));
}

function requireArray(
  body: Record<string, unknown>,
  field: string,
  name: string,
): readonly unknown[] {
  const value = body[field];
  if (!Array.isArray(value)) throw new ProtocolError(`${name} carried an invalid ${field}`);
  return value;
}

function requireTextArray(
  body: Record<string, unknown>,
  field: string,
  name: string,
): readonly string[] {
  return requireArray(body, field, name).map((value) => {
    if (typeof value !== 'string' || value.trim() === '') {
      throw new ProtocolError(`${name} carried an invalid ${field}`);
    }
    return value;
  });
}

function requireBoolean(body: Record<string, unknown>, field: string, name: string): boolean {
  const value = body[field];
  if (typeof value !== 'boolean') {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value;
}

function requireNullableText(
  body: Record<string, unknown>,
  field: string,
  name: string,
): string | null {
  if (!Object.prototype.hasOwnProperty.call(body, field)) {
    throw new ProtocolError(`${name} omitted ${field}`);
  }
  if (body[field] === null) return null;
  return requireText(body, field, name);
}

function requireNullableInteger(
  body: Record<string, unknown>,
  field: string,
  name: string,
): number | null {
  if (!Object.prototype.hasOwnProperty.call(body, field)) {
    throw new ProtocolError(`${name} omitted ${field}`);
  }
  if (body[field] === null) return null;
  return requireInteger(body, field, name);
}

function requireSafeInteger(body: Record<string, unknown>, field: string, name: string): number {
  const value = body[field];
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) {
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  }
  return value;
}

function requireInteger(body: Record<string, unknown>, field: string, name: string): number {
  const value = requireSafeInteger(body, field, name);
  if (value < 0) throw new ProtocolError(`${name} carried an invalid ${field}`);
  return value;
}

function jsonEqual(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((entry, index) => jsonEqual(entry, right[index]))
    );
  }
  if (typeof left !== 'object' || left === null || typeof right !== 'object' || right === null) {
    return false;
  }
  const leftObject = left as Record<string, unknown>;
  const rightObject = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftObject);
  const rightKeys = Object.keys(rightObject);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every(
      (key) =>
        Object.prototype.hasOwnProperty.call(rightObject, key) &&
        jsonEqual(leftObject[key], rightObject[key]),
    )
  );
}

function identifier(value: string, name: string): string {
  if (value.trim() === '' || value.includes('/'))
    throw new ValidationError(`${name} must be a non-empty path identifier`);
  return encodeURIComponent(value);
}

function repositoryLocator(value: string, name: string): string {
  if (
    value.trim() !== value ||
    value.length < 1 ||
    value.length > 100 ||
    !/^[A-Za-z0-9._-]+$/.test(value) ||
    value === '.' ||
    value === '..'
  ) {
    throw new ValidationError(`${name} must be one canonical owner or repository name`);
  }
  return value;
}

function launchPath(launchId: string): string {
  return `/api/v1/launches/${identifier(launchId, 'Launch id')}`;
}

function cursorTarget(path: string, limit: number, cursor?: string): string {
  const params = new URLSearchParams({ limit: String(pageLimit(limit, 'Trigger')) });
  if (cursor !== undefined) params.set('cursor', cursor);
  return `${path}?${params.toString()}`;
}

function pageLimit(limit: number, name: string): number {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200) {
    throw new ValidationError(`${name} page limit must be an integer from 1 through 200`);
  }
  return limit;
}

function validateIdempotencyKey(key: string): void {
  if (key.length < 1 || key.length > 255 || [...key].some((value) => value < '!' || value > '~')) {
    throw new ValidationError('idempotency key must contain 1 to 255 visible ASCII characters');
  }
}
