import { ProtocolError } from '@chronoai/cma-sdk';
import type { ConnectionTarget } from '../connections/client';

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
    !Array.isArray(request.services) ||
    request.services.length > 64 ||
    !request.services.every(
      (service) => typeof service === 'string' && service.trim() === service && service.length > 0,
    ) ||
    typeof request.reason !== 'string' ||
    (request.error !== undefined && typeof request.error !== 'string') ||
    (request.connection !== undefined &&
      (request.connection?.target?.type !== 'agent_access_request' ||
        request.connection?.target?.id !== request.id)) ||
    (request.status === 'connecting' && request.connection === undefined)
  )
    throw new ProtocolError('Agent Manager returned an invalid access request.');
  return request;
}
async function readResponse(response: Response): Promise<unknown> {
  if (!response.ok) {
    const problem = (await response.json().catch(() => ({}))) as { detail?: unknown };
    throw new Error(
      typeof problem.detail === 'string'
        ? problem.detail
        : `Agent access could not be updated (${String(response.status)}).`,
    );
  }
  return response.json();
}
const path = (agentId: string) =>
  `/api/agent-manager/v1/agents/${encodeURIComponent(agentId)}/access-requests`;
export const accessRequests = {
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
  ): Promise<AgentAccessRequest> {
    const result = accessRequest(
      await readResponse(
        await fetch(`${path(agentId)}/${encodeURIComponent(id)}/${action}`, {
          method: 'POST',
          credentials: 'same-origin',
          cache: 'no-store',
          signal,
          headers: { 'Content-Type': 'application/json', 'Idempotency-Key': key },
          body: JSON.stringify(action === 'approve' ? { services } : {}),
        }),
      ),
      agentId,
    );
    if (result.id !== id)
      throw new ProtocolError('Agent Manager returned a different access request.');
    return result;
  },
};
