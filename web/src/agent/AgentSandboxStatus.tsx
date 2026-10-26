import type { AppRoute } from '../routing/routes';
import { LifecycleProgress } from '../sandbox/LifecycleProgress';
import { SandboxRecoveryNotice } from '../sandbox/SandboxRecoveryNotice';
import type { AgentController } from './controller';

export function AgentSandboxStatus({
  observation,
  onNavigate,
}: {
  readonly observation: AgentController['progressObservation'];
  readonly onNavigate: (route: AppRoute) => void;
}) {
  if (observation === undefined) return null;
  return (
    <>
      <SandboxRecoveryNotice
        {...observation}
        onDetails={() => onNavigate({ kind: 'sandbox', sandboxId: observation.sandbox.id })}
      />
      {['provisioning', 'installing'].includes(observation.sandbox.status) ? (
        <LifecycleProgress {...observation} />
      ) : null}
    </>
  );
}
