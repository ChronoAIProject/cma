import { useEffect, useRef, useState } from 'react';
import { ConnectionWizard } from './ConnectionWizard';
import { connectionClient, type ConnectionTarget } from './client';

interface PendingAction {
  readonly id: number;
  readonly target: ConnectionTarget;
  readonly action: () => void;
  readonly forceSubscription: boolean;
  readonly owner?: string;
  readonly request?: AbortController;
}

/** Keep the original operation in memory; only the verified server result releases it. */
export function useConnectionGate() {
  const [pending, setPending] = useState<PendingAction>();
  const [interactive, setInteractive] = useState(false);
  const [error, setError] = useState<string>();
  const active = useRef<PendingAction | undefined>(undefined);
  const serial = useRef(0);
  useEffect(
    () => () => {
      active.current?.request?.abort();
      active.current = undefined;
    },
    [],
  );
  const run = (target: ConnectionTarget, action: () => void, forceSubscription = false) => {
    if (active.current !== undefined) return;
    const value = { id: ++serial.current, target, action, forceSubscription };
    active.current = value;
    setError(undefined);
    setInteractive(true);
    setPending(value);
  };
  const cancel = () => {
    active.current?.request?.abort();
    active.current = undefined;
    setPending(undefined);
  };
  const complete = (value: PendingAction) => {
    if (active.current !== value) return;
    active.current = undefined;
    setPending(undefined);
    value.action();
  };
  const checkAndRun = async (target: ConnectionTarget, owner: string, action: () => void) => {
    if (active.current !== undefined) return;
    const value: PendingAction = {
      id: ++serial.current,
      target,
      action,
      owner,
      forceSubscription: false,
      request: new AbortController(),
    };
    active.current = value;
    setError(undefined);
    setInteractive(false);
    setPending(value);
    try {
      const report = await connectionClient.check(target, undefined, value.request!.signal);
      if (active.current !== value) return;
      if (report.owner !== owner)
        throw new Error('Your CMA account changed. Open the Agent again to continue.');
      if (report.ready) complete(value);
      else if (report.services.some((row) => row.state === 'missing' || row.state === 'inactive'))
        setInteractive(true);
      else throw new Error('The connections could not be verified. Please try sending again.');
    } catch (cause) {
      if (active.current !== value) return;
      cancel();
      setError(
        cause instanceof Error
          ? cause.message
          : 'The connections could not be verified. Please try sending again.',
      );
    }
  };
  const dialog =
    pending === undefined || !interactive ? null : (
      <ConnectionWizard
        key={pending.id}
        target={pending.target}
        presentation="dialog"
        dialogTitle="Connect your accounts"
        forceSubscription={pending.forceSubscription}
        expectedOwner={pending.owner}
        onReady={() => complete(pending)}
        onCancel={cancel}
      />
    );
  return {
    run,
    checkAndRun,
    dialog,
    pending: pending !== undefined,
    error,
    clearError: () => setError(undefined),
  };
}
