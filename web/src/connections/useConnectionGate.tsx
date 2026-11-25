import { useCallback, useLayoutEffect, useRef, useState } from 'react';
import { ConnectionWizard } from './ConnectionWizard';
import {
  connectionClient,
  isConnectionReport,
  type ConnectionReport,
  type ConnectionTarget,
} from './client';
import { readWithConnectionCooldown } from './readCooldown';

const ACTION_CHECK_TIMEOUT_MS = 15_000;

interface PendingAction {
  readonly id: number;
  readonly target: ConnectionTarget;
  readonly action: (report: ConnectionReport) => void;
  readonly forceSubscription: boolean;
  readonly owner?: string;
  readonly request?: AbortController;
  readonly onCancelled?: () => void;
}

/** Keep the original operation in memory; only the verified server result releases it. */
export function useConnectionGate(
  enabled = true,
  scope?: object,
  description?: string,
  repositoryRecovery = false,
) {
  const [pending, setPending] = useState<PendingAction>();
  const [interactive, setInteractive] = useState(false);
  const [error, setError] = useState<string>();
  const active = useRef<PendingAction | undefined>(undefined);
  const serial = useRef(0);
  useLayoutEffect(
    () => () => {
      active.current?.request?.abort();
      active.current?.onCancelled?.();
      active.current = undefined;
      setPending(undefined);
    },
    [enabled, scope],
  );
  const run = (
    target: ConnectionTarget,
    action: (report: ConnectionReport) => void,
    forceSubscription = false,
    owner?: string,
  ) => {
    if (!enabled || active.current !== undefined) return;
    const value = { id: ++serial.current, target, action, forceSubscription, owner };
    active.current = value;
    setError(undefined);
    setInteractive(true);
    setPending(value);
  };
  const cancel = useCallback(() => {
    active.current?.request?.abort();
    active.current?.onCancelled?.();
    active.current = undefined;
    setPending(undefined);
  }, []);
  const complete = (value: PendingAction, report: ConnectionReport) => {
    if (active.current !== value) return;
    active.current = undefined;
    setPending(undefined);
    value.action(report);
  };
  const checkAndRun = async (
    target: ConnectionTarget,
    owner: string,
    action: (report: ConnectionReport) => void,
    prepared?: () => ConnectionReport | undefined,
    onCancelled?: () => void,
    readContext?: object,
  ) => {
    if (!enabled || active.current !== undefined) {
      onCancelled?.();
      return;
    }
    const value: PendingAction = {
      id: ++serial.current,
      target,
      action,
      owner,
      forceSubscription: false,
      request: new AbortController(),
      onCancelled,
    };
    active.current = value;
    setError(undefined);
    setInteractive(false);
    setPending(value);
    try {
      const signal =
        readContext === undefined
          ? value.request!.signal
          : AbortSignal.any([value.request!.signal, AbortSignal.timeout(ACTION_CHECK_TIMEOUT_MS)]);
      const read = () => connectionClient.check(target, undefined, signal);
      const report =
        prepared?.() ??
        (await (readContext === undefined
          ? read()
          : readWithConnectionCooldown(readContext, owner, signal, read)));
      if (active.current !== value) return;
      if (readContext !== undefined && !isConnectionReport(report))
        throw new Error('The connections could not be verified. Please try sending again.');
      if (report.owner !== owner)
        throw new Error('Your CMA account changed. Open the Agent again to continue.');
      if (report.ready) complete(value, report);
      else if (report.services.some((row) => row.state === 'missing' || row.state === 'inactive'))
        setInteractive(true);
      else throw new Error('The connections could not be verified. Please try sending again.');
    } catch (cause) {
      if (active.current !== value) return;
      cancel();
      setError(
        cause instanceof DOMException && cause.name === 'TimeoutError'
          ? 'The connection check timed out. Please try sending again.'
          : cause instanceof Error
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
        description={description}
        repositoryRecovery={repositoryRecovery}
        onReady={(report) => complete(pending, report)}
        onCancel={cancel}
      />
    );
  return {
    run,
    checkAndRun,
    cancel,
    dialog,
    pending: enabled && pending !== undefined,
    error,
    clearError: () => setError(undefined),
  };
}
