import { useId, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { IconStopFill16 } from '../ui/icons';
import type { ResponseControlAvailability } from './controlReadiness';
import { RESPONSE_STOP_COMMAND_NOTICE } from './stopAgentCopy';

const notices: Record<Exclude<ResponseControlAvailability, 'available'>, string> = {
  checking: 'Checking response controls…',
  'checking-access': 'Checking Agent access…',
  unbound: 'Stop is unavailable for this response.',
  denied: 'Only the owner can stop this response.',
};

export function useResponseStop({
  running,
  availability,
  busy,
  stopping,
  onStop,
}: {
  running: boolean;
  availability: ResponseControlAvailability;
  busy: boolean;
  stopping: boolean;
  onStop: () => void | Promise<void>;
}) {
  const pending = useRef(false);
  const [submitting, setSubmitting] = useState(false);
  const inProgress = stopping || submitting;
  const enabled = running && availability === 'available' && !busy && !inProgress;
  const notice =
    availability !== 'available'
      ? notices[availability]
      : inProgress
        ? 'Stopping…'
        : busy
          ? 'Another Agent action is pending.'
          : undefined;
  const stop = () => {
    if (!enabled || pending.current) return;
    pending.current = true;
    setSubmitting(true);
    const finish = () => {
      pending.current = false;
      setSubmitting(false);
    };
    try {
      void Promise.resolve(onStop()).then(finish, finish);
    } catch {
      // The controller owns the error and retained command recovery.
      finish();
    }
  };
  return { enabled, inProgress, notice, stop };
}

export function ResponseStop({
  control,
  shortcut,
}: {
  control: ReturnType<typeof useResponseStop>;
  shortcut: boolean;
}) {
  const id = useId();
  const scopeId = `${id}-scope`;
  const hint = control.notice ?? (shortcut ? 'Esc to stop' : undefined);
  const description = [hint && id, control.enabled && scopeId].filter(Boolean).join(' ');
  const action = shortcut ? 'Stop response (Esc)' : 'Stop response';
  return (
    <>
      {control.enabled ? (
        <span className="agent-composer__stop-scope" id={scopeId}>
          {RESPONSE_STOP_COMMAND_NOTICE}
        </span>
      ) : null}
      <div className="agent-composer__actions">
        {hint ? (
          <span id={id} className="agent-composer__stop-hint">
            {hint}
          </span>
        ) : null}
        <Button
          placement="agent-composer__submit agent-composer__submit--stop"
          variant="primary"
          icon={<IconStopFill16 />}
          aria-label={control.inProgress ? 'Stopping response' : 'Stop response'}
          aria-describedby={description || undefined}
          title={control.notice ?? `${action}. ${RESPONSE_STOP_COMMAND_NOTICE}`}
          disabled={!control.enabled}
          onClick={control.stop}
        />
      </div>
    </>
  );
}
