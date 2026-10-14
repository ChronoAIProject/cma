import { ProtocolError } from '@chronoai/cma-sdk';
import type { AgentSkillsView, CmaWebClient } from '../client';

export class AgentSkillsError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}
async function request(id: string, init: RequestInit): Promise<AgentSkillsView> {
  const response = await fetch(`/api/v1/agents/${encodeURIComponent(id)}/skills`, {
    credentials: 'same-origin',
    cache: 'no-store',
    ...init,
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => ({}))) as { detail?: unknown };
    throw new AgentSkillsError(
      typeof problem.detail === 'string' ? problem.detail : 'Agent skills are unavailable.',
      response.status,
    );
  }
  const value = (await response.json()) as AgentSkillsView;
  if (
    !value ||
    typeof value.pending !== 'boolean' ||
    typeof value.can_install !== 'boolean' ||
    (value.unavailable_reason !== undefined && typeof value.unavailable_reason !== 'string') ||
    !Array.isArray(value.items) ||
    value.items.length > 256 ||
    value.items.some(
      (item) =>
        !item ||
        typeof item.id !== 'string' ||
        !item.id.trim() ||
        typeof item.version !== 'string' ||
        !item.version.trim() ||
        item.source !== 'ornn' ||
        !['installed', 'pending', 'failed'].includes(item.status) ||
        (item.name !== undefined && typeof item.name !== 'string') ||
        (item.error !== undefined && typeof item.error !== 'string'),
    ) ||
    new Set(value.items.map((item) => item.id)).size !== value.items.length ||
    (!value.pending && value.items.some((item) => item.status === 'pending'))
  )
    throw new ProtocolError('Agent Manager returned invalid skill status.');
  return value;
}
export function agentSkillsClient(): Pick<CmaWebClient, 'readAgentSkills' | 'installAgentSkill'> {
  return {
    readAgentSkills: (id, options) => request(id, { signal: options?.signal }),
    installAgentSkill: (id, skill, options) =>
      request(id, {
        method: 'POST',
        signal: options.signal,
        headers: { 'Content-Type': 'application/json', 'Idempotency-Key': options.idempotencyKey },
        body: JSON.stringify({ id: skill.id, version: skill.version }),
      }),
  };
}
