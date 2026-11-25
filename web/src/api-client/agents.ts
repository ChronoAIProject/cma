import { readAgentProjection } from './agents/read-projection.js';
import { readReplyPreview } from './agents/reply-preview.js';
import { readOriginalRequest } from './agents/original-request.js';
import { readRetainedOutput, type RetainedOutputOptions } from './agents/retained-output.js';
import { CapacityRecovery } from './agents/capacity-recovery.js';
import {
  readTranscript,
  readTranscriptPage,
  type TranscriptPageOptions,
} from './agents/transcript.js';
import {
  readAgentPreferences,
  changeAgentPreferences,
  type UpdateAgentPreferencesRequest,
} from './agents/preferences.js';
import {
  listMessageFeedback,
  setMessageFeedback,
  type MessageVote,
} from './agents/message-feedback.js';
import {
  feedbackDetails,
  setFeedbackReasons,
  type FeedbackReasonRequest,
} from './agents/feedback-reasons.js';
import {
  readAgentSettings,
  changeAgentSettings,
  type UpdateAgentSettingsRequest,
} from './agents/settings.js';
import { listAgentModels } from './agents/models.js';
import {
  readAgentSkills,
  installAgentSkill,
  type InstallAgentSkillRequest,
} from './agents/skills.js';
import {
  listAgentAccessRequests,
  approveAgentAccess,
  denyAgentAccess,
  updateAgentServiceAccess,
  type ApproveAgentAccessRequest,
  type UpdateAgentServiceAccessRequest,
} from './agents/access-requests.js';
import { readSuggestions } from './agents/suggestions.js';
import { requestVoid, type OperationOptions } from './http.js';
import { ProtocolError } from './errors.js';
import { boolean, object, text, type Parser } from './schema.js';
import {
  command,
  identity,
  input,
  page,
  query,
  read,
  segment,
  type CommandOptions,
  type EventOptions,
  type PageOptions,
} from './resources.js';
import { agentStatus, agentTaskStatus, type AgentStatus } from './instances/types.js';
import {
  agent,
  agentResources,
  agentSummary,
  agentTextRequest,
  agentUsage,
  approvalDecisionRequest,
  createAgentRequest,
  stopAgentRequest,
  type AgentTextRequest,
  type ApprovalDecisionRequest,
  type CreateAgentRequest,
  type StopAgentRequest,
} from './agents/types.js';
import {
  agentCreationCategory,
  agentProvenance,
  agentProvenanceInventoryItem,
  type AgentCreationCategory,
} from './agents/provenance.js';
import { readAgentEvents } from './agents/events.js';
import {
  cancelAgentLifecycle,
  createAgentLifecycle,
  readAgentLifecycle,
  recoverAgentWorktree,
  type AgentLifecycleCreateRequest,
} from './agents/lifecycle.js';
import { accessibleAgent, accessibleAgentSource, accessibleAgentSummary } from './agents/shared.js';
import { requireRefusal } from './unsupported.js';
import { listAgentProviderAssociations } from './agents/provider-associations.js';
import { readAgentChannelUsage, listAgentChannelUsageHistory } from './agents/channel-usage.js';
import { readDesktopCapability, type DesktopScope } from './agents/desktop.js';
import {
  listAgentArtifacts,
  readAgentArtifact,
  publishAgentArtifact,
  downloadAgentArtifact,
  type PublishAgentArtifactRequest,
} from './agents/artifacts.js';
import { AgentDeliverables } from './agents/deliverables.js';
export interface AgentListOptions extends PageOptions {
  readonly sandbox?: string;
  readonly workspace?: string;
  readonly status?: AgentStatus;
}
export interface AccessibleAgentListOptions extends AgentListOptions {
  /** Canonical organization ID; omission selects personal inventory. */
  readonly organization?: string;
}
export interface AgentProvenanceInventoryOptions extends AccessibleAgentListOptions {
  readonly taskStatus?: import('./instances/types.js').AgentTaskStatus;
  /** Explicit incoming shares across current organizations; mutually exclusive with organization. */
  readonly shared?: boolean;
  /** Immutable category. Continue with `next` even after an empty filtered page. */
  readonly creationOrigin?: AgentCreationCategory;
  /** Filter explicit current bindings without changing creation categories. */
  readonly channelActive?: boolean;
  /** Filter any retained channel attachment/use history, including detached bindings. */
  readonly channelHistory?: boolean;
}
/** Public Agent REST commands and durable read projections. */
export class Agents extends AgentDeliverables {
  originalRequest(id: string, options: OperationOptions = {}) {
    return readOriginalRequest(this.config, id, options);
  }

  replyPreview(id: string, options: OperationOptions = {}) {
    return readReplyPreview(this.config, id, options);
  }

  /** Discover and page retained partial text without changing the original failed outcome. */
  retainedOutput(id: string, options: RetainedOutputOptions = {}) {
    return readRetainedOutput(
      this.config,
      `/api/v1/agents/${segment(id)}/retained-output`,
      options,
      id,
    );
  }
  capacityRecovery(id: string) {
    return new CapacityRecovery(this.config, id);
  }
  suggestions(
    id: string,
    scope: import('./answer-context.js').AnswerContext,
    options: OperationOptions = {},
  ) {
    return readSuggestions(this.config, id, scope, options);
  }
  feedbackDetails(id: string, messageId: string, options: OperationOptions = {}) {
    return feedbackDetails(this.config, id, messageId, options);
  }
  setFeedbackReasons(
    id: string,
    messageId: string,
    body: FeedbackReasonRequest,
    options: OperationOptions = {},
  ) {
    return setFeedbackReasons(this.config, id, messageId, body, options);
  }
  /** Read saved owner votes; use next as after until pagination completes. */
  messageFeedback(id: string, options: EventOptions = {}) {
    return listMessageFeedback(this.config, id, options);
  }
  /** Replace or clear feedback without creating an Agent turn. */
  setMessageFeedback(
    id: string,
    messageId: string,
    vote: MessageVote | null,
    options: OperationOptions = {},
  ) {
    return setMessageFeedback(this.config, id, messageId, vote, options);
  }
  /** Discover the owner-authenticated desktop stream. The host opens its WebSocket. */
  desktop(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/agents/${segment(id)}/desktop`,
      object({ websocket_url: text }),
      options,
    );
  }
  /** Discover admitted models and their current runtime availability. */
  models(id: string, options: OperationOptions = {}) {
    return listAgentModels(this.config, id, options);
  }
  /** Read skill installation status and current runtime installation support. */
  skills(id: string, options: OperationOptions = {}) {
    return readAgentSkills(this.config, id, options);
  }
  /** Add an exact skill version; reuse the unchanged body and key after uncertain replies. */
  installSkill(id: string, body: InstallAgentSkillRequest, options: CommandOptions) {
    return installAgentSkill(this.config, id, body, options);
  }
  /** Read editable owner preferences and current native instruction support. */
  preferences(id: string, options: OperationOptions = {}) {
    return readAgentPreferences(this.config, id, options);
  }
  /** Rename or update instructions using the last observed revision. */
  updatePreferences(id: string, body: UpdateAgentPreferencesRequest, options: CommandOptions) {
    return changeAgentPreferences(this.config, id, body, options);
  }
  /** Read desired runtime settings and their confirmation state. */
  settings(id: string, options: OperationOptions = {}) {
    return readAgentSettings(this.config, id, options);
  }
  /** Update an idle Agent; preserve body and key when retrying uncertain replies. */
  updateSettings(id: string, body: UpdateAgentSettingsRequest, options: CommandOptions) {
    return changeAgentSettings(this.config, id, body, options);
  }
  /** Read durable owner-visible service requests without granting access. */
  accessRequests(id: string, options: OperationOptions = {}) {
    return listAgentAccessRequests(this.config, id, options);
  }
  /** Replace exact shared service access without restarting the running Sandbox. */
  updateServiceAccess(id: string, body: UpdateAgentServiceAccessRequest, options: CommandOptions) {
    return updateAgentServiceAccess(this.config, id, body, options);
  }
  /** Approve an exact selection; retry uncertain replies with the original body and key. */
  approveAccessRequest(
    id: string,
    requestId: string,
    body: ApproveAgentAccessRequest,
    options: CommandOptions,
  ) {
    return approveAgentAccess(this.config, id, requestId, body, options);
  }
  /** Explicitly deny this request using a stable command key. */
  denyAccessRequest(id: string, requestId: string, options: CommandOptions) {
    return denyAgentAccess(this.config, id, requestId, options);
  }
  /** Admit or resume the exact same inline request with its original idempotency key. */
  createLifecycle(body: AgentLifecycleCreateRequest, options: CommandOptions) {
    return createAgentLifecycle(this.config, body, options);
  }
  desktopCapability(scope: DesktopScope = {}, options: OperationOptions = {}) {
    return readDesktopCapability(this.config, scope, options);
  }

  /** Retry a never-ready Agent's failed initial worktree preparation, preserving its PV and identity.
   * The failed worktree is retained and a fresh one is allocated. Reuse the key after uncertain replies.
   */
  recoverWorktree(id: string, options: CommandOptions) {
    return recoverAgentWorktree(this.config, id, options);
  }

  /** Read owner-visible provisioning progress; succeeded means the Agent is ready. */
  readLifecycle(id: string, options: OperationOptions = {}) {
    return readAgentLifecycle(this.config, id, options);
  }

  /** Cancel an owner-scoped lifecycle operation before Sandbox allocation. */
  cancelLifecycle(id: string, options: OperationOptions = {}) {
    return cancelAgentLifecycle(this.config, id, options);
  }

  /** Legacy Bridge usage projection; provider admissions require providerAssociations(). */
  channelUsage(id: string, options: OperationOptions = {}) {
    return readAgentChannelUsage(this.config, id, options);
  }

  /** Read retained provider memberships under current Agent management authority. */
  providerAssociations(id: string, options: PageOptions = {}) {
    return listAgentProviderAssociations(this.config, id, options);
  }

  /** Current bridge managers can page retained channel binding history. */
  channelUsageHistory(id: string, options: PageOptions = {}) {
    return listAgentChannelUsageHistory(this.config, id, options);
  }

  /** List explicitly approved artifacts under current Agent visibility. */
  listArtifacts(id: string, options: PageOptions = {}) {
    return listAgentArtifacts(this.config, id, options);
  }
  readArtifact(id: string, artifactId: string, options: OperationOptions = {}) {
    return readAgentArtifact(this.config, id, artifactId, options);
  }
  /** Publish immutable selected content; requires current sharing-management authority. */
  publishArtifact(id: string, body: PublishAgentArtifactRequest, options: CommandOptions) {
    return publishAgentArtifact(this.config, id, body, options);
  }
  /** Download selected artifact bytes with current authorization; does not execute or render them. */
  downloadArtifact(id: string, artifactId: string, options: OperationOptions = {}) {
    return downloadAgentArtifact(this.config, id, artifactId, options);
  }

  /** Safe creation inventory. Omit organization and shared for personal scope. */
  provenanceInventory(options: AgentProvenanceInventoryOptions = {}) {
    if (options.taskStatus !== undefined) input(agentTaskStatus, options.taskStatus);
    if (options.status !== undefined) input(agentStatus, options.status);
    if (options.creationOrigin !== undefined) input(agentCreationCategory, options.creationOrigin);
    if (options.channelActive !== undefined) input(boolean, options.channelActive);
    if (options.channelHistory !== undefined) input(boolean, options.channelHistory);
    return read(
      this.config,
      query('/api/v1/agents/provenance', {
        shared: options.shared === undefined ? undefined : String(options.shared),
        organization: options.organization,
        sandbox: options.sandbox,
        workspace: options.workspace,
        status: options.status,
        creation_origin: options.creationOrigin,
        task_status: options.taskStatus,
        channel_active:
          options.channelActive === undefined ? undefined : String(options.channelActive),
        channel_history:
          options.channelHistory === undefined ? undefined : String(options.channelHistory),
        cursor: options.cursor,
        limit: options.limit,
      }),
      page(agentProvenanceInventoryItem),
      options,
    );
  }

  list(options: AgentListOptions = {}) {
    if (options.status !== undefined) input(agentStatus, options.status);
    return read(
      this.config,
      query('/api/v1/agents', {
        sandbox: options.sandbox,
        workspace: options.workspace,
        status: options.status,
        cursor: options.cursor,
        limit: options.limit,
      }),
      page(agentSummary),
      options,
    );
  }

  /** List personal Agents or the Agents visible in one current organization. */
  listAccessible(options: AccessibleAgentListOptions = {}) {
    if (options.status !== undefined) input(agentStatus, options.status);
    return read(
      this.config,
      query('/api/v1/agents', {
        organization: options.organization,
        sandbox: options.sandbox,
        workspace: options.workspace,
        status: options.status,
        cursor: options.cursor,
        limit: options.limit,
      }),
      page(accessibleAgentSummary),
      options,
    );
  }

  read(id: string, options: OperationOptions = {}) {
    return read(this.config, `/api/v1/agents/${segment(id)}`, identity(agent, id), options);
  }

  /** Read an owner detail or the safe Agent-only shared detail. */
  readAccessible(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/agents/${segment(id)}`,
      identity(accessibleAgent, id),
      options,
    );
  }

  create(workspaceId: string, body: CreateAgentRequest, options: CommandOptions) {
    return read(this.config, `/api/v1/workspaces/${segment(workspaceId)}/agents`, agent, {
      ...command(options),
      method: 'POST',
      body: input(createAgentRequest, body),
      expectedStatus: 201,
    });
  }

  input(id: string, body: AgentTextRequest, options: CommandOptions) {
    return read(this.config, `/api/v1/agents/${segment(id)}/input`, object({ queued: boolean }), {
      ...command(options),
      method: 'POST',
      body: input(agentTextRequest, body),
      expectedStatus: 202,
    });
  }

  steer(id: string, body: AgentTextRequest, options: CommandOptions) {
    return this.operate(id, '/steer', 'POST', options, input(agentTextRequest, body));
  }

  interrupt(id: string, options: CommandOptions) {
    return this.operate(id, '/interrupt', 'POST', options);
  }

  answerApproval(
    id: string,
    requestId: string,
    body: ApprovalDecisionRequest,
    options: CommandOptions,
  ) {
    return this.operate(
      id,
      `/approvals/${segment(requestId)}`,
      'POST',
      options,
      input(approvalDecisionRequest, body),
    );
  }

  stop(id: string, body: StopAgentRequest, options: CommandOptions) {
    return this.operate(id, '/stop', 'POST', options, input(stopAgentRequest, body));
  }

  restart(id: string, options: CommandOptions) {
    return read(
      this.config,
      `/api/v1/agents/${segment(id)}/restart`,
      object({ agent_id: text, worktree_id: text, operation_id: text, accepted_at: text }),
      { ...command(options), method: 'POST', expectedStatus: 202 },
    );
  }

  delete(id: string, options: CommandOptions) {
    return this.operate(id, '', 'DELETE', options);
  }

  events(
    id: string,
    options: EventOptions & { readonly toolOutput?: boolean; readonly cache?: 'no-store' } = {},
  ) {
    return readAgentEvents(this.config, id, options);
  }

  transcript(id: string, options: import('./agents/transcript.js').TranscriptReadOptions = {}) {
    return readTranscript(this.config, id, options);
  }
  transcriptPage(id: string, options: TranscriptPageOptions = {}) {
    return readTranscriptPage(this.config, id, options);
  }
  usage(id: string, options: OperationOptions = {}) {
    return this.readProjection(id, 'usage', agentUsage, options);
  }
  resources(id: string, options: OperationOptions = {}) {
    return this.readProjection(id, 'resources', agentResources, options);
  }

  /** Immutable creation facts; source details require current resource management. */
  provenance(id: string, options: OperationOptions = {}) {
    return this.readProjection(id, 'provenance', agentProvenance, options);
  }

  async source(id: string, options: OperationOptions = {}) {
    const value = await this.sourceAccessible(id, options);
    if (value.access === 'shared')
      throw new ProtocolError('shared Agent source requires sourceAccessible');
    return value;
  }

  /** Personal provenance or a shared creation category, under current access. */
  sourceAccessible(id: string, options: OperationOptions = {}) {
    return this.readProjection(id, 'source', accessibleAgentSource, options);
  }

  /** Preserve the named terminal capability refusal; no attachment is available. */
  terminal(id: string, options: OperationOptions = {}): Promise<never> {
    return requireRefusal(this.config, `/api/v1/agents/${segment(id)}/terminal`, 'GET', options);
  }

  private readProjection<Value extends { readonly agent_id: string }>(
    id: string,
    name: string,
    parse: Parser<Value>,
    options: OperationOptions,
  ) {
    return readAgentProjection(this.config, id, name, parse, options);
  }

  private operate(
    id: string,
    suffix: string,
    method: string,
    options: CommandOptions,
    body?: unknown,
  ) {
    return requestVoid(this.config, `/api/v1/agents/${segment(id)}${suffix}`, {
      ...command(options),
      method,
      body,
      expectedStatus: 202,
    });
  }
}
