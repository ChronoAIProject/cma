import type { ResolvedConfig } from './config.js';
import { requestVoid, type OperationOptions } from './http.js';
import { ProtocolError } from './errors.js';
import { array, boolean, integer, object, optional, type Parser } from './schema.js';
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
import { agentStatus, type AgentStatus } from './instances/types.js';
import {
  agent,
  agentResources,
  agentSummary,
  agentTextRequest,
  agentTranscript,
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
import { agentEvent } from './agents/events.js';
import { accessibleAgent, accessibleAgentSource, accessibleAgentSummary } from './agents/shared.js';
import { requireRefusal } from './unsupported.js';

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
  /** Immutable category. Continue with `next` even after an empty filtered page. */
  readonly creationOrigin?: AgentCreationCategory;
}

/** Public Agent REST commands and durable read projections. */
export class Agents {
  constructor(private readonly config: ResolvedConfig) {}

  /** Safe creation inventory, including retained archived Agents. Omit organization for personal scope. */
  provenanceInventory(options: AgentProvenanceInventoryOptions = {}) {
    if (options.status !== undefined) input(agentStatus, options.status);
    if (options.creationOrigin !== undefined) input(agentCreationCategory, options.creationOrigin);
    return read(
      this.config,
      query('/api/v1/agents/provenance', {
        organization: options.organization,
        sandbox: options.sandbox,
        workspace: options.workspace,
        status: options.status,
        creation_origin: options.creationOrigin,
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

  delete(id: string, options: CommandOptions) {
    return this.operate(id, '', 'DELETE', options);
  }

  events(id: string, options: EventOptions = {}) {
    return read(
      this.config,
      query(`/api/v1/agents/${segment(id)}/events`, {
        after: options.after,
        limit: options.limit,
      }),
      object({ items: array(agentEvent), next: optional(integer) }),
      options,
    );
  }

  transcript(id: string, options: OperationOptions = {}) {
    return this.readProjection(id, 'transcript', agentTranscript, options);
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
    return read(
      this.config,
      `/api/v1/agents/${segment(id)}/${name}`,
      (value) => {
        const projection = parse(value);
        if (projection.agent_id !== id)
          throw new ProtocolError('projection Agent identity did not match the request');
        return projection;
      },
      options,
    );
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
