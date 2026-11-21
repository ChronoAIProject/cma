import type { AgentDetail } from '../api/types';
import type { ResourceViewsClient } from './client';

export interface LifecycleHandoff {
  readonly agentId: string;
  readonly workspaceId: string;
  readonly sandboxId: string;
  readonly expiresAt: number;
}

// This is only a navigation optimization. It never carries an access token or
// command authority, and the access hook still revalidates the Agent before
// the page settles.
const handoffs = new WeakMap<ResourceViewsClient, Map<string, LifecycleHandoff>>();
const HANDOFF_TTL_MS = 30_000;

export function retainLifecycleHandoff(
  client: ResourceViewsClient | undefined,
  viewerId: string | undefined,
  detail: AgentDetail | undefined,
) {
  if (client === undefined || viewerId === undefined || detail === undefined) return;
  let values = handoffs.get(client);
  if (values === undefined) {
    values = new Map();
    handoffs.set(client, values);
  }
  values.set(`${viewerId}:${detail.id}`, {
    agentId: detail.id,
    workspaceId: detail.workspace.id,
    sandboxId: detail.sandbox.id,
    expiresAt: Date.now() + HANDOFF_TTL_MS,
  });
  if (values.size > 8) values.delete(values.keys().next().value!);
}

export function readLifecycleHandoff(
  client: ResourceViewsClient | undefined,
  viewerId: string | undefined,
  agentId: string | undefined,
): LifecycleHandoff | undefined {
  if (client === undefined || viewerId === undefined || agentId === undefined) return undefined;
  const values = handoffs.get(client);
  const key = `${viewerId}:${agentId}`;
  const value = values?.get(key);
  if (value === undefined) return undefined;
  if (value.expiresAt <= Date.now()) {
    values?.delete(key);
    return undefined;
  }
  return value;
}

export function forgetLifecycleHandoff(
  client: ResourceViewsClient | undefined,
  viewerId: string | undefined,
  agentId: string | undefined,
) {
  if (client === undefined || viewerId === undefined || agentId === undefined) return;
  handoffs.get(client)?.delete(`${viewerId}:${agentId}`);
}
