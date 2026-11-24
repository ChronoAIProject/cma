const reasons = [
  'dispatch_in_progress',
  'dispatch_recovery_required',
  'stale_revision',
  'admission_conflict',
  'agent_not_idle',
  'outcome_unconfirmed',
] as const;

export type AgentUpdateReason = (typeof reasons)[number];

export function agentUpdateHeaders(key: string): HeadersInit {
  return {
    'Content-Type': 'application/json',
    'Idempotency-Key': key,
    Prefer: 'cma-settings-reasons',
  };
}

export function isAgentUpdateReason(value: unknown): value is AgentUpdateReason {
  return reasons.some((reason) => reason === value);
}

/** Preserve only the public decisions the editor understands. */
export async function agentUpdateProblem(response: Response): Promise<{
  code?: string;
  reason?: AgentUpdateReason;
}> {
  const problem: unknown = await response.json().catch(() => undefined);
  if (!problem || typeof problem !== 'object') return {};
  const { code, settings_update_reason: reason } = problem as Record<string, unknown>;
  return {
    code: typeof code === 'string' && /^[a-z][a-z0-9_]{0,127}$/.test(code) ? code : undefined,
    reason: isAgentUpdateReason(reason) ? reason : undefined,
  };
}
