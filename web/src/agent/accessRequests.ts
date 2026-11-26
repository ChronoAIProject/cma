import { ApiError, ProtocolError, retryAfterMilliseconds } from '@cma/client';
import type { ConnectionTarget } from '../connections/client';

export function validAccessServices(value: unknown): value is readonly string[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.length <= 32 &&
    value.every((service) => typeof service === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(service))
  );
}

export interface AgentAccessRequest {
  readonly id: string;
  readonly agent_id: string;
  readonly status: 'pending' | 'connecting' | 'applying' | 'succeeded' | 'denied' | 'failed';
  readonly services: readonly string[];
  readonly reason: string;
  readonly connection?: {
    readonly target: Extract<ConnectionTarget, { type: 'agent_access_request' }>;
  };
  readonly error?: string;
  readonly github_approval?: GithubApprovalContext;
}

export interface GithubConnectionApproval {
  readonly sandbox_id: string;
  readonly user_service_id: string;
  readonly scope_revision: number;
  readonly credential_generation: number;
}

interface GithubApprovalContext extends Omit<GithubConnectionApproval, 'user_service_id'> {
  readonly sandbox_name: string;
  readonly other_agents: readonly string[];
  readonly current_user_service_id: string | null;
}

export function validGithubConnectionApproval(
  value: GithubConnectionApproval | undefined,
): value is GithubConnectionApproval {
  return (
    value !== undefined &&
    typeof value === 'object' &&
    value !== null &&
    typeof value.sandbox_id === 'string' &&
    value.sandbox_id.length > 0 &&
    typeof value.user_service_id === 'string' &&
    /^[a-zA-Z0-9_-]{1,128}$/.test(value.user_service_id) &&
    Number.isSafeInteger(value.scope_revision) &&
    value.scope_revision >= 0 &&
    Number.isSafeInteger(value.credential_generation) &&
    value.credential_generation >= 0
  );
}
function accessRequest(value: unknown, agentId: string): AgentAccessRequest {
  const request = value as AgentAccessRequest | undefined;
  if (
    !request ||
    typeof request.id !== 'string' ||
    !request.id ||
    request.agent_id !== agentId ||
    !['pending', 'connecting', 'applying', 'succeeded', 'denied', 'failed'].includes(
      request.status,
    ) ||
    !validAccessServices(request.services) ||
    typeof request.reason !== 'string' ||
    (request.error !== undefined && typeof request.error !== 'string') ||
    (request.github_approval !== undefined &&
      (!request.github_approval ||
        !validGithubConnectionApproval({ ...request.github_approval, user_service_id: 'probe' }) ||
        typeof request.github_approval.sandbox_name !== 'string' ||
        !Array.isArray(request.github_approval.other_agents) ||
        !request.github_approval.other_agents.every((id) => typeof id === 'string') ||
        (request.github_approval.current_user_service_id !== null &&
          typeof request.github_approval.current_user_service_id !== 'string'))) ||
    (request.connection !== undefined &&
      (request.connection?.target?.type !== 'agent_access_request' ||
        request.connection?.target?.id !== request.id)) ||
    (request.status === 'connecting' && request.connection === undefined)
  )
    throw new ProtocolError('Agent Manager returned an invalid access request.');
  return request;
}
export class AgentAccessError extends ApiError {
  constructor(status: number, message: string, retryAfterMs?: number, code?: string) {
    super(status, message, code, undefined, retryAfterMs);
  }
}
async function readResponse(response: Response): Promise<unknown> {
  if (!response.ok) {
    const problem = (await response.json().catch(() => ({}))) as {
      detail?: unknown;
      code?: unknown;
    };
    throw new AgentAccessError(
      response.status,
      typeof problem.detail === 'string'
        ? problem.detail
        : `Agent access could not be updated (${String(response.status)}).`,
      retryAfterMilliseconds(response),
      typeof problem.code === 'string' ? problem.code : undefined,
    );
  }
  return response.json();
}
const path = (agentId: string) =>
  `/api/agent-manager/v1/agents/${encodeURIComponent(agentId)}/access-requests`;
export const accessRequests = {
  async replace(
    agentId: string,
    services: readonly string[],
    expectedServices: readonly string[],
    key: string,
    signal: AbortSignal,
  ): Promise<AgentAccessRequest> {
    if (!validAccessServices(services) || !validAccessServices(expectedServices))
      throw new Error('Choose 1 to 32 services with valid service identifiers.');
    return accessRequest(
      await readResponse(
        await fetch(`/api/agent-manager/v1/agents/${encodeURIComponent(agentId)}/service-access`, {
          method: 'PUT',
          credentials: 'same-origin',
          cache: 'no-store',
          signal,
          headers: { 'Content-Type': 'application/json', 'Idempotency-Key': key },
          body: JSON.stringify({ services, expected_services: expectedServices }),
        }),
      ),
      agentId,
    );
  },
  async list(agentId: string, signal: AbortSignal): Promise<readonly AgentAccessRequest[]> {
    const value = (await readResponse(
      await fetch(path(agentId), { credentials: 'same-origin', cache: 'no-store', signal }),
    )) as { items?: unknown };
    if (!value || !Array.isArray(value.items))
      throw new ProtocolError('Agent Manager returned invalid access requests.');
    const items = value.items.map((item) => accessRequest(item, agentId));
    if (new Set(items.map((item) => item.id)).size !== items.length)
      throw new ProtocolError('Agent Manager returned repeated access requests.');
    return items;
  },
  async decide(
    agentId: string,
    id: string,
    action: 'approve' | 'deny',
    key: string,
    services: readonly string[],
    signal: AbortSignal,
    githubConnection?: GithubConnectionApproval,
  ): Promise<AgentAccessRequest> {
    if (action === 'approve' && !validAccessServices(services))
      throw new Error('Choose 1 to 32 services with valid service identifiers.');
    const result = accessRequest(
      await readResponse(
        await fetch(`${path(agentId)}/${encodeURIComponent(id)}/${action}`, {
          method: 'POST',
          credentials: 'same-origin',
          cache: 'no-store',
          signal,
          headers: { 'Content-Type': 'application/json', 'Idempotency-Key': key },
          body: JSON.stringify(
            action === 'approve' ? { services, github_connection: githubConnection } : {},
          ),
        }),
      ),
      agentId,
    );
    if (result.id !== id)
      throw new ProtocolError('Agent Manager returned a different access request.');
    return result;
  },
};
