import {
  validAccessServices,
  validGithubConnectionApproval,
  type GithubConnectionApproval,
} from './accessRequests';

export interface AccessDecision {
  readonly key: string;
  readonly action: 'approve' | 'deny';
  readonly services: readonly string[];
  readonly githubConnection?: GithubConnectionApproval;
}
export function accessDecisionStorageKey(owner: string, agentId: string, requestId: string) {
  return `cma:agent-access:${JSON.stringify([location.origin, owner, agentId, requestId])}`;
}
export function loadAccessDecision(
  owner: string | undefined,
  agentId: string,
  requestId: string,
): { intent?: AccessDecision; error?: string } {
  try {
    if (!owner) throw new Error();
    const raw = sessionStorage.getItem(accessDecisionStorageKey(owner, agentId, requestId));
    if (raw === null) return {};
    const saved = JSON.parse(raw) as { version?: number; intent?: AccessDecision };
    const intent = saved.intent;
    if (
      saved.version !== 1 ||
      !intent ||
      !/^[a-zA-Z0-9-]{1,200}$/.test(intent.key) ||
      !['approve', 'deny'].includes(intent.action) ||
      !Array.isArray(intent.services) ||
      (intent.action === 'approve' && !validAccessServices(intent.services)) ||
      (intent.action === 'deny' && intent.services.length !== 0) ||
      (intent.githubConnection !== undefined &&
        (intent.action !== 'approve' || !validGithubConnectionApproval(intent.githubConnection)))
    )
      throw new Error();
    return { intent };
  } catch {
    return {
      error:
        'The saved access decision could not be read. Restore your browser session before continuing.',
    };
  }
}
export function saveAccessDecision(
  owner: string | undefined,
  agentId: string,
  requestId: string,
  intent?: AccessDecision,
) {
  if (!owner) throw new Error('Your account could not be verified. Open the Agent again.');
  const key = accessDecisionStorageKey(owner, agentId, requestId);
  if (intent) sessionStorage.setItem(key, JSON.stringify({ version: 1, intent }));
  else sessionStorage.removeItem(key);
}
