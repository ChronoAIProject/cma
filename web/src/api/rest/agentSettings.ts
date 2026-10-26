import { agentPreferencesClient } from './agentPreferences';
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
  | 'readAgentSettings'
  | 'updateAgentSettings'
  | 'readAgentModels'
  | 'readAgentPreferences'
  | 'updateAgentPreferences'
> {
  return {
    ...agentPreferencesClient(),
    readAgentModels: async (id, options) => {
      const response = await fetch(`/api/v1/agents/${encodeURIComponent(id)}/models`, {
        credentials: 'same-origin',
        cache: 'no-store',
        signal: options?.signal,
      });
      if (!response.ok)
        throw new AgentSettingsError('Available models could not be loaded.', response.status);
      const value = (await response.json()) as { models?: unknown };
      if (
        !Array.isArray(value?.models) ||
        value.models.length > 2000 ||
        value.models.some(
          (model: unknown) =>
            !model ||
            typeof model !== 'object' ||
            !('id' in model) ||
            typeof model.id !== 'string' ||
            !model.id.trim() ||
            model.id.length > 256 ||
            !('label' in model) ||
            typeof model.label !== 'string' ||
            !model.label.trim() ||
            ('available' in model && typeof model.available !== 'boolean'),
        ) ||
        new Set(value.models.map((model: { id: string }) => model.id)).size !== value.models.length
      )
        throw new ProtocolError('Agent Manager returned an invalid model catalog.');
      return (value.models as { id: string; label: string; available?: boolean }[]).map(
        (model) => ({ ...model, available: model.available === true }),
      );
    },
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
