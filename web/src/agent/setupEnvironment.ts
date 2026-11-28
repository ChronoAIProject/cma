import type { AgentStatus, SandboxDetail } from '../api/types';

export const SETUP_ENVIRONMENT_NOTICE =
  'Review the environment before confirming deletion. This removes the environment and all Agents inside it.';

export function setupEnvironmentRoute(
  status: AgentStatus,
  sandboxId: string,
  viewerId: string | undefined,
  mayDelete: boolean,
  observation?: { readonly sandbox: SandboxDetail; readonly stale: boolean },
) {
  const sandbox = observation?.sandbox;
  if (
    status !== 'starting' ||
    !mayDelete ||
    viewerId === undefined ||
    sandbox === undefined ||
    observation?.stale !== false ||
    sandbox.id !== sandboxId ||
    sandbox.owner !== viewerId ||
    !['provisioning', 'installing'].includes(sandbox.status)
  )
    return undefined;
  return { kind: 'sandbox', sandboxId: sandbox.id } as const;
}
