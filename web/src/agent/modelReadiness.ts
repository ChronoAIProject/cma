import type { AgentDetail, SandboxDetail, SandboxEvent } from '../api/types';

export function agentInitialized(status: string, readyAt?: string) {
  return (
    readyAt !== undefined ||
    ['ready', 'working', 'waiting_approval', 'paused', 'stopped'].includes(status)
  );
}

export function modelCatalogReady(
  detail: AgentDetail,
  observation?: { readonly sandbox: SandboxDetail; readonly stale: boolean },
  events: readonly SandboxEvent[] = [],
) {
  const sandbox =
    observation?.sandbox.id === detail.sandbox.id && !observation.stale
      ? observation.sandbox
      : detail.sandbox;
  return (
    agentInitialized(detail.status, detail.readyAt) ||
    sandbox.status === 'active' ||
    events.some((event) => {
      if (event.sandboxId !== detail.sandbox.id || event.kind.type !== 'work_step_completed')
        return false;
      const step = event.kind.step;
      return (
        typeof step === 'object' &&
        step !== null &&
        'kind' in step &&
        step.kind === 'deliver_initial_credential'
      );
    })
  );
}
