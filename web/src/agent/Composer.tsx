import { Presence } from '../ui/Presence';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { AgentStatus } from '../api/types';
import { Button } from '../ui/Button';
import { IconSendOutline16, IconStopFill16 } from '../ui/icons';
import type { SandboxInputAvailability } from './sandboxInput';
import type { TriggerQuickMessage } from '@chronoai/cma-sdk';
import { QuickMessages } from './QuickMessages';
import type { AgentDraft } from './draft';
import { attachmentMessage, transferredFiles, type UploadAttachment } from './attachments';
import { useComposerAttachments } from './useComposerAttachments';
import { ComposerAttachmentPicker, ComposerAttachments } from './ComposerAttachments';

interface ComposerProps {
  readonly draft?: AgentDraft;
  readonly status: AgentStatus;
  readonly pausedNotice?: string;
  readonly running: boolean;
  readonly queueWhileRunning?: boolean;
  readonly queueWhileStarting?: boolean;
  readonly starting?: boolean;
  readonly controlsUnavailable?: boolean;
  readonly requestPending?: boolean;
  readonly acceptedMessage?: string;
  readonly inputAvailability: SandboxInputAvailability;
  readonly busy: boolean;
  readonly quickMessages?: readonly TriggerQuickMessage[];
  readonly uploadAttachment?: UploadAttachment;
  readonly onSend: (content: string, onAccepted: () => void) => Promise<void> | void;
  readonly onStop: () => Promise<void> | void;
}

function canMessage(status: AgentStatus): boolean {
  return status === 'ready' || status === 'working' || status === 'paused';
}

function placeholder(status: AgentStatus): string {
  if (status === 'paused') return 'Send a message to continue.';
  // Startup is represented by the lifecycle progress surface and the
  // disabled/queued controls. Keep the composer quiet so it never adds a
  // second visible "starting" notification to the conversation area.
  if (status === 'starting') return 'Message Agent';
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
  draft,
  status,
  pausedNotice,
  running,
  queueWhileRunning = false,
  queueWhileStarting = false,
  starting = false,
  controlsUnavailable = false,
  requestPending = false,
  acceptedMessage,
  inputAvailability,
  busy,
  quickMessages = [],
  uploadAttachment,
  onSend,
  onStop,
}: ComposerProps) {
  const [localContent, setLocalContent] = useState('');
  const content = draft?.content ?? localContent;
  const setContent = draft?.setContent ?? setLocalContent;
  const previousAccepted = useRef(acceptedMessage);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const sending = useRef(false);
  const composing = useRef(false);
  const noticeId = useId();
  const inputBlocked = !inputAvailability.allowed;
  const preparing = status === 'starting' && queueWhileStarting;
  const notice = inputAvailability.notice ?? pausedNotice;
  const hasNotice = notice !== undefined || inputAvailability.savingStartedAt !== undefined;
  const usable = canMessage(status) || preparing;
  const attachmentEnabled =
    (status === 'ready' || status === 'working') && !inputBlocked && !starting && !busy;
  const attachments = useComposerAttachments(draft, uploadAttachment, attachmentEnabled);
  const messageContent = attachmentMessage(content, attachments.items);

  useLayoutEffect(() => {
    if (acceptedMessage === previousAccepted.current) return;
    previousAccepted.current = acceptedMessage;
    if (acceptedMessage !== undefined && messageContent === acceptedMessage) {
      setContent('');
      attachments.accept(attachments.items);
    }
  }, [acceptedMessage, messageContent, setContent, attachments]);

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

  const submit = (quickMessage?: string) => {
    const message = quickMessage ?? messageContent;
    if (
      message === '' ||
      composing.current ||
      !usable ||
      busy ||
      (running && !queueWhileRunning) ||
      starting ||
      inputBlocked ||
      requestPending ||
      (quickMessage === undefined && attachments.blocked) ||
      sending.current
    )
      return;
    const draft = content;
    const submittedAttachments = attachments.items;
    sending.current = true;
    setSubmitting(true);
    void Promise.resolve(
      onSend(message, () => {
        if (quickMessage === undefined) {
          setContent((current) => (current === draft ? '' : current));
          attachments.accept(submittedAttachments);
        }
      }),
    ).finally(() => {
      sending.current = false;
      setSubmitting(false);
    });
  };

  return (
    <div
      className="agent-composer"
      onPaste={(event) => {
        const files = transferredFiles(event.clipboardData);
        if (files.length === 0) return;
        event.preventDefault();
        if (!submitting && !requestPending) attachments.add(files);
      }}
      onDragOver={(event) => {
        if (!Array.from(event.dataTransfer.types).includes('Files')) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = attachmentEnabled ? 'copy' : 'none';
      }}
      onDrop={(event) => {
        const files = transferredFiles(event.dataTransfer);
        if (files.length === 0) return;
        event.preventDefault();
        if (!submitting && !requestPending) attachments.add(files);
      }}
    >
      <QuickMessages
        messages={quickMessages}
        disabled={
          !usable ||
          busy ||
          (running && !queueWhileRunning) ||
          starting ||
          inputBlocked ||
          submitting ||
          requestPending
        }
        onSend={submit}
      />
      <Presence>
        {!hasNotice ? null : (
          <p id={noticeId} className="agent-composer__notice" role="status">
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
      </Presence>
      <form
        className={`agent-composer__form${running && queueWhileRunning ? ' agent-composer__form--queue' : ''}`}
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
          <div className="agent-composer__editor">
            <ComposerAttachments
              {...attachments}
              enabled={attachmentEnabled && uploadAttachment !== undefined}
              locked={submitting || requestPending}
            />
            <div className="agent-composer__input-row">
              <textarea
                ref={inputRef}
                id="agent-message"
                rows={1}
                value={content}
                disabled={!usable}
                aria-describedby={hasNotice ? noticeId : undefined}
                placeholder={preparing ? 'Message Agent' : placeholder(status)}
                onChange={(event) => {
                  setContent(event.target.value);
                }}
                onCompositionStart={() => {
                  composing.current = true;
                }}
                onCompositionEnd={() => {
                  composing.current = false;
                }}
                onBlur={() => {
                  composing.current = false;
                }}
                onKeyDown={(event) => {
                  // Safari can end composition before the confirming Enter and reports keyCode 229.
                  if (
                    composing.current ||
                    event.nativeEvent.isComposing ||
                    event.nativeEvent.keyCode === 229
                  )
                    return;
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault();
                    submit();
                  }
                }}
              />
              {uploadAttachment === undefined ? null : (
                <ComposerAttachmentPicker
                  add={attachments.add}
                  enabled={attachmentEnabled && uploadAttachment !== undefined}
                  locked={submitting || requestPending}
                />
              )}
            </div>
          </div>
        </div>
        {!running || queueWhileRunning ? (
          <Button
            placement="agent-composer__submit"
            variant="primary"
            type="submit"
            icon={<IconSendOutline16 />}
            aria-label={running ? 'Queue message' : 'Send message'}
            title={running ? 'Queue message' : 'Send message'}
            disabled={
              !usable ||
              busy ||
              submitting ||
              starting ||
              inputBlocked ||
              requestPending ||
              messageContent === '' ||
              attachments.blocked
            }
            aria-describedby={hasNotice ? noticeId : undefined}
          />
        ) : null}
        {running ? (
          <Button
            placement="agent-composer__submit agent-composer__submit--stop"
            variant="primary"
            icon={<IconStopFill16 />}
            aria-label="Stop response"
            title={
              controlsUnavailable
                ? 'This turn does not support response controls.'
                : 'Stop response'
            }
            disabled={busy || controlsUnavailable}
            onClick={() => void onStop()}
          />
        ) : null}
      </form>
    </div>
  );
}
