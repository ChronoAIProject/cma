import { useEffect, useState } from 'react';
import type { SandboxDetail } from '../api/types';

interface RecoveryObservation {
  readonly sandboxId: string;
  readonly recovering: boolean;
  readonly completed: boolean;
}

/** Completion feedback requires a recovery-to-active observation in this view. */
export function useRecoveryCompletion(sandbox: SandboxDetail, stale: boolean) {
  const purpose = sandbox.lifecycle?.operation?.purpose;
  const recovering = sandbox.status === 'reviving' || purpose === 'revive';
  const active = sandbox.status === 'active' && purpose === undefined;
  const snapshot = `${sandbox.id}:${recovering}:${active}:${stale}`;
  const [previous, setPrevious] = useState(snapshot);
  const [observation, setObservation] = useState<RecoveryObservation>({
    sandboxId: sandbox.id,
    recovering: recovering && !stale,
    completed: false,
  });
  if (snapshot !== previous) {
    setPrevious(snapshot);
    if (!stale || sandbox.id !== observation.sandboxId)
      setObservation({
        sandboxId: sandbox.id,
        recovering: recovering && !stale,
        completed:
          !stale &&
          active &&
          sandbox.id === observation.sandboxId &&
          (observation.recovering || observation.completed),
      });
  }
  const completed = observation.sandboxId === sandbox.id && observation.completed;
  useEffect(() => {
    if (!completed) return;
    const timer = window.setTimeout(
      () => setObservation((value) => ({ ...value, completed: false })),
      8000,
    );
    return () => window.clearTimeout(timer);
  }, [sandbox.id, completed]);
  return {
    completed: completed && active && !stale,
    dismiss: () => setObservation((value) => ({ ...value, completed: false })),
  };
}
