import { ProtocolError } from '@chronoai/cma-sdk';
import type { AgentPreferencesView, CmaWebClient } from '../client';

export class AgentPreferencesError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

async function request(id: string, init: RequestInit): Promise<AgentPreferencesView> {
  const response = await fetch(`/api/v1/agents/${encodeURIComponent(id)}/preferences`, {
    credentials: 'same-origin',
    cache: 'no-store',
    ...init,
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => ({}))) as { detail?: unknown };
    throw new AgentPreferencesError(
      typeof problem.detail === 'string'
        ? problem.detail
        : response.status === 409
          ? 'The Agent changed. Reload its preferences and try again when its turn is finished.'
          : `Agent preferences are unavailable (${String(response.status)}).`,
      response.status,
    );
  }
  const value = (await response.json()) as AgentPreferencesView;
  if (
    !value ||
    (value.title !== undefined && typeof value.title !== 'string') ||
    typeof value.system_prompt !== 'string' ||
    new TextEncoder().encode(value.system_prompt).length > 32768 ||
    typeof value.prompt_pending !== 'boolean' ||
    typeof value.prompt_supported !== 'boolean' ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 0
  )
    throw new ProtocolError('Agent Manager returned invalid Agent preferences.');
  return value;
}

export function agentPreferencesClient(): Pick<
  CmaWebClient,
  'readAgentPreferences' | 'updateAgentPreferences'
> {
  return {
    readAgentPreferences: (id, options) => request(id, { signal: options?.signal }),
    updateAgentPreferences: (id, preferences, options) =>
      request(id, {
        method: 'PATCH',
        signal: options.signal,
        headers: { 'Content-Type': 'application/json', 'Idempotency-Key': options.idempotencyKey },
        body: JSON.stringify(preferences),
      }),
  };
}
