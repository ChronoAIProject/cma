/** Public CMA Trigger and Launch REST contract. */

import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, requestBytes, type OperationOptions } from './http.js';

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

export interface TriggerPreview {
  readonly public_id: string;
  readonly availability: 'published' | 'retired';
  readonly publisher_display_identity: string;
  readonly repository_id: number;
  readonly repository_owner: string;
  readonly repository_name: string;
  readonly visibility: 'public' | 'private' | 'internal';
  readonly allow_forking: boolean;
  readonly public_summary: string;
  readonly permission_ceiling: string;
  readonly required_capabilities: readonly string[];
  readonly resource_default: SessionSize;
  readonly resource_ceiling: SessionSize;
  readonly timeout_default_seconds: number;
  readonly timeout_ceiling_seconds: number;
  readonly billing_mode: 'contributor' | 'zero_cost' | 'sponsored';
  readonly pricing_policy_reference: string;
  readonly revision_digest: string;
}

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

export type TriggerEditableField =
  'task_input' | 'branch_suffix' | 'resource_size' | 'timeout' | 'optional_checks';

export interface TriggerOverridePolicy {
  readonly editable_fields: readonly TriggerEditableField[];
  readonly optional_checks: readonly string[];
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
    readonly role: string;
    readonly verified_at: string;
  };
  readonly published_at: string;
  readonly retired_at: string | null;
}

export interface TriggerConnection {
  readonly service_id: string;
  readonly state_version: number;
  readonly state: string;
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

export type LaunchStepStatus =
  'pending' | 'running' | 'completed' | 'blocked' | 'retryable' | 'failed' | 'cancelled';

export interface LaunchStep {
  readonly key: string;
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

export interface LaunchFailure {
  readonly class: string;
  readonly code: string;
  readonly detail: string;
  readonly remediation: string;
  readonly correlation_id: string;
  readonly retry_at?: string;
  readonly observed_at: string;
}

export interface LaunchOverrides {
  readonly task_inputs: Readonly<Record<string, JsonValue>>;
  readonly branch_suffix?: string;
  readonly resource_size?: SessionSize;
  readonly timeout_seconds?: number;
  readonly optional_checks: readonly string[];
}

export interface EffectiveConfiguration {
  readonly rendered_prompt: string;
  readonly branch_suffix: string;
  readonly permission: string;
  readonly resource_size: SessionSize;
  readonly timeout_seconds: number;
  readonly optional_checks: readonly string[];
  readonly digest: string;
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
  readonly validation: string;
  readonly checks: readonly {
    readonly key: string;
    readonly passed: boolean;
    readonly summary: string;
  }[];
  readonly created_at: string;
  readonly delete_after: string;
  readonly download_url: string;
}

export interface ContributionRecord {
  readonly id: string;
  readonly diff_artifact_id: string;
  readonly revision_digest: string;
  readonly baseline_digest: string;
  readonly checkpoint_digest: string;
  readonly diff_digest: string;
  readonly branch_name: string;
  readonly state: string;
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
  readonly checks: string;
  readonly confirmation_snapshot_digest: string;
  readonly confirmed_at: string;
  readonly created_at: string;
  readonly updated_at: string;
  readonly provider_created_at: string | null;
  readonly provider_updated_at: string | null;
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
  readonly blocked_reason: string | null;
  readonly failure: LaunchFailure | null;
  readonly overrides: LaunchOverrides;
  readonly effective: EffectiveConfiguration;
  readonly pricing: JsonObject | null;
  readonly connection: TriggerConnection | null;
  readonly workflow_paths: readonly string[];
  readonly workflow_risk_acknowledged: boolean;
  readonly steps: readonly LaunchStep[];
  readonly fork: ForkRecord | null;
  readonly branch: BranchRecord | null;
  readonly workspace: JsonObject | null;
  readonly session: JsonObject | null;
  readonly reach: JsonObject | null;
  readonly delivery: JsonObject | null;
  readonly contribution_checkpoint_version: number | null;
  readonly diff: DiffArtifact | null;
  readonly contribution: ContributionRecord | null;
  readonly pull_request: PullRequestRecord | null;
  readonly created_at: string;
  readonly updated_at: string;
  readonly started_at: string | null;
  readonly terminal_at: string | null;
}

export interface LaunchProgress {
  readonly launch_id: string;
  readonly events: readonly (JsonObject & {
    readonly sequence: number;
    readonly launch_id: string;
    readonly occurred_at: string;
    readonly correlation_id: string;
    readonly kind: JsonObject;
  })[];
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
  const body = objectOf(
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
  params.set('limit', String(page.limit ?? 50));
  if (page.cursor !== undefined) params.set('cursor', page.cursor);
  return decodeCursorPage(
    await request(config, `/api/v1/launches?${params.toString()}`, {
      ...options,
      expectedStatus: 200,
    }),
    decodeLaunch,
    'Launches',
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

export async function readLaunchProgress(
  config: ResolvedConfig,
  launchId: string,
  after = 0,
  limit = 100,
  options: OperationOptions = {},
): Promise<LaunchProgress> {
  if (!Number.isSafeInteger(after) || after < 0 || !Number.isSafeInteger(limit) || limit < 1)
    throw new ValidationError(
      'Launch progress cursor and limit must be non-negative bounded integers',
    );
  return decodeLaunchProgress(
    await request(
      config,
      `${launchPath(launchId)}/events?after=${String(after)}&limit=${String(limit)}`,
      { ...options, expectedStatus: 200 },
    ),
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
  requireText(body, 'revision_digest', 'Trigger preview');
  requireText(body, 'public_summary', 'Trigger preview');
  requireInteger(body, 'repository_id', 'Trigger preview');
  return body as unknown as TriggerPreview;
}

function decodeTriggerDraft(raw: unknown): TriggerDraft {
  const body = safeObject(raw, 'Trigger draft');
  requireText(body, 'id', 'Trigger draft');
  requireText(body, 'selected_service_id', 'Trigger draft');
  requireInteger(body, 'version', 'Trigger draft');
  safeObject(body.content, 'Trigger draft content');
  return body as unknown as TriggerDraft;
}

function decodeTriggerRevision(raw: unknown): TriggerRevision {
  const body = safeObject(raw, 'Trigger revision');
  requireText(body, 'id', 'Trigger revision');
  requireText(body, 'public_id', 'Trigger revision');
  requireText(body, 'digest', 'Trigger revision');
  safeObject(body.content, 'Trigger revision content');
  return body as unknown as TriggerRevision;
}

function decodeConnection(raw: unknown): TriggerConnection {
  const body = safeObject(raw, 'Trigger connection');
  requireText(body, 'service_id', 'Trigger connection');
  requireInteger(body, 'state_version', 'Trigger connection');
  requireText(body, 'state', 'Trigger connection');
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
  requireText(repository, 'default_branch', 'Trigger repository identity');
  requireText(repository, 'selected_ref', 'Trigger repository identity');
  return body as unknown as TriggerRepositorySelection;
}

export function decodeLaunch(raw: unknown): Launch {
  const body = safeObject(raw, 'Launch');
  requireText(body, 'id', 'Launch');
  requireText(body, 'trigger_public_id', 'Launch');
  requireText(body, 'revision_digest', 'Launch');
  requireText(body, 'state', 'Launch');
  requireInteger(body, 'revision_number', 'Launch');
  if (!Array.isArray(body.steps)) throw new ProtocolError('Launch steps was not an array');
  safeObject(body.revision_content, 'Launch revision content');
  safeObject(body.effective, 'Launch effective configuration');
  return body as unknown as Launch;
}

function decodeDiffReview(raw: unknown): DiffReview {
  const body = safeObject(raw, 'Launch diff review');
  requireText(body, 'launch_id', 'Launch diff review');
  requireInteger(body, 'revision_number', 'Launch diff review');
  requireText(body, 'state', 'Launch diff review');
  return body as unknown as DiffReview;
}

function decodePullRequestDraft(raw: unknown): PullRequestDraft {
  const body = safeObject(raw, 'pull-request draft');
  requireText(body, 'launch_id', 'pull-request draft');
  requireText(body, 'operation', 'pull-request draft');
  requireText(body, 'intent_digest', 'pull-request draft');
  requireText(body, 'expected_head_revision', 'pull-request draft');
  return body as unknown as PullRequestDraft;
}

function decodeLaunchProgress(raw: unknown): LaunchProgress {
  const body = safeObject(raw, 'Launch progress');
  requireText(body, 'launch_id', 'Launch progress');
  requireInteger(body, 'next_cursor', 'Launch progress');
  if (!Array.isArray(body.events))
    throw new ProtocolError('Launch progress events was not an array');
  let previous = 0;
  for (const event of body.events) {
    const value = safeObject(event, 'Launch progress event');
    const sequence = requireInteger(value, 'sequence', 'Launch progress event');
    if (sequence <= previous)
      throw new ProtocolError('Launch progress events were not strictly ordered');
    previous = sequence;
  }
  return body as unknown as LaunchProgress;
}

function decodeCursorPage<T>(
  raw: unknown,
  decode: (value: unknown) => T,
  name: string,
): CursorPage<T> {
  const body = safeObject(raw, name);
  if (!Array.isArray(body.items)) throw new ProtocolError(`${name} items was not an array`);
  if (body.next_cursor !== null && typeof body.next_cursor !== 'string')
    throw new ProtocolError(`${name} next_cursor was invalid`);
  return { items: body.items.map(decode), next_cursor: body.next_cursor };
}

function safeObject(raw: unknown, name: string): Record<string, unknown> {
  const object = objectOf(raw, name);
  assertJsonValue(object, name, new Set());
  for (const forbidden of [
    'idempotency_key_hash',
    'intent_key',
    'provider_request_id',
    'object_reference',
    'backing_key_id',
    'worker_fencing_token',
  ]) {
    if (containsKey(object, forbidden)) throw new ProtocolError(`${name} leaked ${forbidden}`);
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

function containsKey(value: unknown, key: string): boolean {
  if (typeof value !== 'object' || value === null) return false;
  if (!Array.isArray(value) && Object.prototype.hasOwnProperty.call(value, key)) return true;
  return (Array.isArray(value) ? value : Object.values(value)).some((entry) =>
    containsKey(entry, key),
  );
}

function requireText(body: Record<string, unknown>, field: string, name: string): string {
  const value = body[field];
  if (typeof value !== 'string' || value.trim() === '')
    throw new ProtocolError(`${name} carried no ${field}`);
  return value;
}

function requireInteger(body: Record<string, unknown>, field: string, name: string): number {
  const value = body[field];
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new ProtocolError(`${name} carried an invalid ${field}`);
  return value;
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
  if (!Number.isSafeInteger(limit) || limit < 1)
    throw new ValidationError('Trigger page limit must be a positive integer');
  const params = new URLSearchParams({ limit: String(limit) });
  if (cursor !== undefined) params.set('cursor', cursor);
  return `${path}?${params.toString()}`;
}

function validateIdempotencyKey(key: string): void {
  if (key.length < 1 || key.length > 255 || [...key].some((value) => value < '!' || value > '~')) {
    throw new ValidationError('idempotency key must contain 1 to 255 visible ASCII characters');
  }
}
