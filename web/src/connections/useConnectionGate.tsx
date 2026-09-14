import { useEffect, useRef, useState } from 'react';
import { Modal } from '../ui/Modal';
import { ConnectionWizard } from './ConnectionWizard';
import type { ConnectionTarget } from './client';

interface PendingAction {
  readonly id: number;
  readonly target: ConnectionTarget;
  readonly action: () => void;
  readonly forceSubscription: boolean;
}

/** Keep the original operation in memory; only the verified server result releases it. */
export function useConnectionGate() {
  const [pending, setPending] = useState<PendingAction>();
  const active = useRef<PendingAction | undefined>(undefined);
  const serial = useRef(0);
  useEffect(
    () => () => {
      active.current = undefined;
    },
    [],
  );
  const run = (target: ConnectionTarget, action: () => void, forceSubscription = false) => {
    if (active.current !== undefined) return;
    const value = { id: ++serial.current, target, action, forceSubscription };
    active.current = value;
    setPending(value);
  };
  const cancel = () => {
    active.current = undefined;
    setPending(undefined);
  };
  const complete = (value: PendingAction) => {
    if (active.current !== value) return;
    active.current = undefined;
    setPending(undefined);
    value.action();
  };
  const dialog =
    pending === undefined ? null : (
      <Modal open title="Connect your accounts" onClose={cancel}>
        <ConnectionWizard
          key={pending.id}
          target={pending.target}
          forceSubscription={pending.forceSubscription}
          onReady={() => complete(pending)}
          onCancel={cancel}
        />
      </Modal>
    );
  return { run, dialog, pending: pending !== undefined };
}
