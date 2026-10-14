import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
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
  const [accepted, setAccepted] = useState(false);
  const [sending, setSending] = useState(true);
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
  }, [agentId, client, requestKey, revision]);
  useEffect(() => {
    if (complete) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const detail = await client.readAgent(agentId, { signal: request.signal });
        if (request.signal.aborted) return;
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
      } catch {
        if (request.signal.aborted) return;
        setReadError('Stop status could not be refreshed. Completion is not yet confirmed.');
      }
      timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [agentId, client, complete, revision]);
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
