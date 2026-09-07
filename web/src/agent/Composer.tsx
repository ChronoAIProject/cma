import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { AgentStatus } from '../api/types';
import { Button } from '../ui/Button';
import { IconSendOutline16, IconStopFill16 } from '../ui/icons';
import type { SandboxInputAvailability } from './sandboxInput';

interface ComposerProps {
  readonly status: AgentStatus;
  readonly pausedNotice?: string;
  readonly running: boolean;
  readonly starting?: boolean;
  readonly inputAvailability: SandboxInputAvailability;
  readonly busy: boolean;
  readonly onSend: (content: string, onAccepted: () => void) => Promise<void> | void;
  readonly onStop: () => Promise<void> | void;
}

function canMessage(status: AgentStatus): boolean {
  return status === 'ready' || status === 'working' || status === 'paused';
}

function placeholder(status: AgentStatus): string {
  if (status === 'paused') return 'Send a message to continue.';
  if (status === 'starting') return 'Agent is starting.';
  if (status === 'waiting_approval') return 'Resolve the approval to continue.';
  if (status === 'stopped') return 'This Agent has stopped.';
  if (status === 'deleted') return 'This Agent was deleted.';
  if (status === 'failed') return 'This Agent is unavailable.';
  return 'Message Agent';
}

function fit(field: HTMLTextAreaElement): void {
  field.style.height = 'auto';
  field.style.height = `${String(field.scrollHeight)}px`;
}

export function Composer({
  status,
  pausedNotice,
  running,
  starting = false,
  inputAvailability,
  busy,
  onSend,
  onStop,
}: ComposerProps) {
  const [content, setContent] = useState('');
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const sending = useRef(false);
  const noticeId = useId();
  const inputBlocked = !inputAvailability.allowed;
  const notice = inputAvailability.notice ?? pausedNotice;
  const hasNotice =
    starting || notice !== undefined || inputAvailability.savingStartedAt !== undefined;
  const usable = canMessage(status);

  useEffect(() => {
    if (usable) inputRef.current?.focus();
  }, [usable]);

  useLayoutEffect(() => {
    const field = inputRef.current;
    if (field !== null) fit(field);
  }, [content]);

  useLayoutEffect(() => {
    const field = inputRef.current;
    if (field === null) return;
    let width = field.clientWidth;
    const observer = new ResizeObserver(() => {
      if (field.clientWidth === width) return;
      width = field.clientWidth;
      fit(field);
    });
    observer.observe(field);
    return () => observer.disconnect();
  }, []);

  const submit = () => {
    const message = content.trim();
    if (message === '' || !usable || busy || running || starting || inputBlocked || sending.current)
      return;
    const draft = content;
    sending.current = true;
    void Promise.resolve(
      onSend(message, () => setContent((current) => (current === draft ? '' : current))),
    ).finally(() => {
      sending.current = false;
    });
  };

  return (
    <div className="agent-composer">
      {!hasNotice ? null : (
        <p id={noticeId} className="agent-composer__notice" role="status">
          {starting ? 'Starting. ' : null}
          {notice}
          {inputAvailability.savingStartedAt === undefined ? null : (
            <>
              {' '}
              Saving started{' '}
              <time dateTime={inputAvailability.savingStartedAt}>
                {new Date(inputAvailability.savingStartedAt).toLocaleString()}
              </time>
              .
            </>
          )}
        </p>
      )}
      <form
        className="agent-composer__form"
        aria-label="Message Agent"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <label className="sr-only" htmlFor="agent-message">
          Message
        </label>
        <div className="agent-composer__capsule">
          <textarea
            ref={inputRef}
            id="agent-message"
            rows={1}
            value={content}
            disabled={!usable}
            aria-describedby={hasNotice ? noticeId : undefined}
            placeholder={placeholder(status)}
            onChange={(event) => {
              setContent(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                submit();
              }
            }}
          />
          <div className="agent-composer__row" />
        </div>
        {running ? (
          <Button
            placement="agent-composer__submit agent-composer__submit--stop"
            variant="primary"
            icon={<IconStopFill16 />}
            aria-label="Stop response"
            title="Stop response"
            disabled={busy}
            onClick={() => void onStop()}
          />
        ) : (
          <Button
            placement="agent-composer__submit"
            variant="primary"
            type="submit"
            icon={<IconSendOutline16 />}
            aria-label="Send message"
            title="Send message"
            disabled={!usable || busy || starting || inputBlocked || content.trim() === ''}
            aria-describedby={hasNotice ? noticeId : undefined}
          />
        )}
      </form>
    </div>
  );
}
