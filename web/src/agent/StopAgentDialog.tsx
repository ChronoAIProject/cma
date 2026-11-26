import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { STOP_RESPONSE_GUIDANCE } from './stopAgentCopy';
import './StopAgentDialog.css';

export function StopAgentDialog({
  client,
  agentId,
  requestKey,
  onChanged,
  onStopped,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly agentId: string;
  readonly requestKey: string;
  readonly onChanged: () => void;
  readonly onStopped: () => void;
  readonly onClose: () => void;
}) {
  const [confirmed, setConfirmed] = useState(false);
  const [accepted, setAccepted] = useState(false);
  const [sending, setSending] = useState(false);
  const [complete, setComplete] = useState(false);
  const [commandError, setCommandError] = useState<string>();
  const [readError, setReadError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const command = useRef<Promise<void> | undefined>(undefined);
  const callbacks = useRef({ onChanged, onStopped });
  useEffect(() => {
    callbacks.current = { onChanged, onStopped };
  }, [onChanged, onStopped]);
  useEffect(() => {
    if (!confirmed) return;
    let active = true;
    command.current ??= client.stopAgent(agentId, { idempotencyKey: requestKey });
    void command.current
      .then(() => {
        if (!active) return;
        setAccepted(true);
        setCommandError(undefined);
        callbacks.current.onChanged();
      })
      .catch((cause: unknown) => {
        if (active)
          setCommandError(
            cause instanceof Error ? cause.message : 'The stop request could not be confirmed.',
          );
      })
      .finally(() => {
        if (active) setSending(false);
      });
    return () => {
      active = false;
    };
  }, [agentId, client, confirmed, requestKey, revision]);
  useEffect(() => {
    if (!confirmed || complete) return;
    let failures = 0;
    const observer = observeRead(async (signal) => {
      try {
        const detail = await client.readAgent(agentId, { signal: signal });
        if (signal.aborted) return;
        if (detail.id !== agentId)
          throw new Error('Agent identity changed while checking stop progress.');
        if (detail.status === 'stopped') {
          callbacks.current.onStopped();
          setComplete(true);
          setReadError(undefined);
          callbacks.current.onChanged();
          return;
        }
        setReadError(
          detail.status === 'failed' || detail.status === 'deleted'
            ? 'The Agent is unavailable. Stopping has not been confirmed.'
            : undefined,
        );
        failures = 0;
      } catch (error) {
        if (signal.aborted) return;
        setReadError('Stop status could not be refreshed. Completion is not yet confirmed.');
        return retrySchedule(error, ++failures);
      }
      return { next: 1000, cooldown: 1000 };
    });
    return () => observer.dispose();
  }, [agentId, client, complete, confirmed, revision]);
  if (!confirmed)
    return (
      <Modal
        open
        title="Stop Agent"
        onClose={onClose}
        footer={
          <>
            <Button onClick={onClose}>Cancel</Button>
            <Button
              variant="primary"
              onClick={() => {
                setSending(true);
                setConfirmed(true);
              }}
            >
              Stop Agent
            </Button>
          </>
        }
      >
        <p>
          Stopping this Agent ends its active work. Its conversation history and Worktree are
          retained. Delete remains a separate action.
        </p>
        <p>{STOP_RESPONSE_GUIDANCE}</p>
      </Modal>
    );
  return (
    <Modal
      open
      title={complete ? 'Agent stopped' : 'Stopping Agent'}
      description="Progress follows the Agent’s confirmed state."
      dismissible={!sending}
      onClose={onClose}
      footer={
        <>
          <Button disabled={sending} onClick={onClose}>
            {complete ? 'Done' : 'Close'}
          </Button>
          {commandError && !complete && !accepted ? (
            <Button
              disabled={sending}
              onClick={() => {
                command.current = undefined;
                setSending(true);
                setCommandError(undefined);
                setRevision((value) => value + 1);
              }}
            >
              Retry stop request
            </Button>
          ) : null}
        </>
      }
    >
      <ol className="agent-stop-progress" aria-label="Stop Agent progress" aria-live="polite">
        <li>
          <span>Stop request</span>
          <strong>
            {accepted || complete ? 'Accepted' : commandError ? 'Unconfirmed' : 'In progress'}
          </strong>
        </li>
        <li>
          <span>Agent stopped</span>
          <strong>{complete ? 'Confirmed' : 'Waiting for confirmation'}</strong>
        </li>
      </ol>
      {complete ? (
        <p role="status">The Agent has stopped.</p>
      ) : (
        <p role="status">Waiting for the Agent to finish stopping.</p>
      )}
      {!complete && commandError ? <p role="alert">{commandError}</p> : null}
      {!complete && readError ? <p role="alert">{readError}</p> : null}
    </Modal>
  );
}
