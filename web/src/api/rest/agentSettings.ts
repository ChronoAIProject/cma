import { ProtocolError } from '@chronoai/cma-sdk';
import type { AgentSettingsView, CmaWebClient } from '../client';

export class AgentSettingsError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

async function settingsRequest(id: string, init: RequestInit): Promise<AgentSettingsView> {
  const response = await fetch(`/api/agent-manager/v1/agents/${encodeURIComponent(id)}/settings`, {
    credentials: 'same-origin',
    cache: 'no-store',
    ...init,
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => ({}))) as { detail?: unknown };
    throw new AgentSettingsError(
      typeof problem.detail === 'string'
        ? problem.detail
        : response.status === 409
          ? 'Wait for this Agent to finish its current turn before changing settings.'
          : `Agent settings are unavailable (${String(response.status)}).`,
      response.status,
    );
  }
  const value = (await response.json()) as AgentSettingsView;
  if (
    !value ||
    typeof value.model !== 'string' ||
    value.model.trim() === '' ||
    !['minimal', 'low', 'medium', 'high', 'x_high'].includes(value.reasoning_effort) ||
    !['guarded', 'autonomous'].includes(value.permission_mode) ||
    (value.pending !== undefined && typeof value.pending !== 'boolean')
  )
    throw new ProtocolError('Agent Manager returned invalid Agent settings.');
  return value;
}

export function agentSettingsClient(): Pick<
  CmaWebClient,
  'readAgentSettings' | 'updateAgentSettings'
> {
  return {
    readAgentSettings: (id, options) => settingsRequest(id, { signal: options?.signal }),
    updateAgentSettings: (id, settings, options) =>
      settingsRequest(id, {
        method: 'PATCH',
        signal: options.signal,
        headers: { 'Content-Type': 'application/json', 'Idempotency-Key': options.idempotencyKey },
        body: JSON.stringify({
          model: settings.model,
          reasoning_effort: settings.reasoning_effort,
          permission_mode: settings.permission_mode,
        }),
      }),
  };
}
