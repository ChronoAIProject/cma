import { useLayoutEffect, useRef, useState } from 'react';
import { ConnectionWizard } from './ConnectionWizard';
import { connectionClient, type ConnectionReport, type ConnectionTarget } from './client';

interface PendingAction {
  readonly id: number;
  readonly target: ConnectionTarget;
  readonly action: () => void;
  readonly forceSubscription: boolean;
  readonly owner?: string;
  readonly request?: AbortController;
}

/** Keep the original operation in memory; only the verified server result releases it. */
export function useConnectionGate(enabled = true, scope?: object) {
  const [pending, setPending] = useState<PendingAction>();
  const [interactive, setInteractive] = useState(false);
  const [error, setError] = useState<string>();
  const active = useRef<PendingAction | undefined>(undefined);
  const serial = useRef(0);
  useLayoutEffect(
    () => () => {
      active.current?.request?.abort();
      active.current = undefined;
      setPending(undefined);
    },
    [enabled, scope],
  );
  const run = (target: ConnectionTarget, action: () => void, forceSubscription = false) => {
    if (!enabled || active.current !== undefined) return;
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
  const checkAndRun = async (
    target: ConnectionTarget,
    owner: string,
    action: () => void,
    prepared?: () => ConnectionReport | undefined,
  ) => {
    if (!enabled || active.current !== undefined) return;
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
      const report =
        prepared?.() ?? (await connectionClient.check(target, undefined, value.request!.signal));
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
    !enabled || pending === undefined || !interactive ? null : (
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
    pending: enabled && pending !== undefined,
    error,
    clearError: () => setError(undefined),
  };
}
