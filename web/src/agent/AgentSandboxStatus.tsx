import type { AppRoute } from '../routing/routes';
import { LifecycleProgress } from '../sandbox/LifecycleProgress';
import { SandboxRecoveryNotice } from '../sandbox/SandboxRecoveryNotice';
import type { AgentController } from './controller';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { SETUP_ENVIRONMENT_NOTICE } from './setupEnvironment';

export function AgentSandboxStatus({
  observation,
  onNavigate,
  onReviewEnvironment,
}: {
  readonly observation: AgentController['progressObservation'];
  readonly onNavigate: (route: AppRoute) => void;
  readonly onReviewEnvironment?: () => void;
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
      {onReviewEnvironment ? (
        <section className="setup-cancel" aria-label="Cancel Agent setup">
          <Alert
            tone="info"
            actions={
              <Button variant="outline" onClick={onReviewEnvironment}>
                Cancel setup…
              </Button>
            }
          >
            {SETUP_ENVIRONMENT_NOTICE}
          </Alert>
        </section>
      ) : null}
    </>
  );
}
