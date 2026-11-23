import { ApiError, ProtocolError, retryAfterMilliseconds } from '@cma/client';
import {
  agentUpdateHeaders,
  agentUpdateProblem,
  type AgentUpdateReason,
} from './agentUpdateProblem';
import type { AgentPreferencesView, CmaWebClient } from '../client';

export class AgentPreferencesError extends ApiError {
  constructor(
    message: string,
    status: number,
    retryAfterMs?: number,
    code?: string,
    reason?: AgentUpdateReason,
  ) {
    super(status, message, code, undefined, retryAfterMs, { settings_update_reason: reason });
  }
}

async function request(id: string, init: RequestInit): Promise<AgentPreferencesView> {
  const response = await fetch(`/api/v1/agents/${encodeURIComponent(id)}/preferences`, {
    credentials: 'same-origin',
    cache: 'no-store',
    ...init,
  });
  if (!response.ok) {
    const problem = await agentUpdateProblem(response);
    throw new AgentPreferencesError(
      'Agent preferences could not be loaded or updated.',
      response.status,
      retryAfterMilliseconds(response),
      problem.code,
      problem.reason,
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
        headers: agentUpdateHeaders(options.idempotencyKey),
        body: JSON.stringify(preferences),
      }),
  };
}
