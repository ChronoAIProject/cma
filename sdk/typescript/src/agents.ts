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
import { agentEvent } from './agents/events.js';

export interface AgentListOptions extends PageOptions {
  readonly sandbox?: string;
  readonly workspace?: string;
  readonly status?: AgentStatus;
}

/** Public Agent REST commands and durable read projections. */
export class Agents {
  constructor(private readonly config: ResolvedConfig) {}

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

  read(id: string, options: OperationOptions = {}) {
    return read(this.config, `/api/v1/agents/${segment(id)}`, identity(agent, id), options);
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
