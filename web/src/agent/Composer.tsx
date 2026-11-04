import { Presence } from '../ui/Presence';
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { AgentStatus, AgentUsage } from '../api/types';
import { Button } from '../ui/Button';
import { IconSendOutline16, IconStopFill16 } from '../ui/icons';
import type { SandboxInputAvailability } from './sandboxInput';
import type { TriggerQuickMessage } from '@chronoai/cma-sdk';
import { QuickMessages } from './QuickMessages';
import type { AgentDraft } from './draft';
import {
  attachmentMessage,
  transferredFiles,
  type DraftAttachment,
  type UploadAttachment,
} from './attachments';
import { useComposerAttachments } from './useComposerAttachments';
import { ComposerAttachmentPicker, ComposerAttachments } from './ComposerAttachments';
import { useComposerCommands } from './ComposerCommands';
import { commandRefusal, type SessionCommand } from './slashCommands';
import { SteeringModePicker, type SteeringMode } from './SteeringMode';
import { ContextRing } from './ContextRing';
import './Composer.css';
import {
  ComposerSuggestionEditor,
  type ComposerSuggestionEditorHandle,
} from './ComposerSuggestionEditor';

interface ComposerProps {
  readonly editorRef?: React.RefObject<ComposerSuggestionEditorHandle | null>;
  readonly suggestionScope?: string;
  readonly draft?: AgentDraft;
  readonly availableCommands?: readonly SessionCommand[];
  readonly usage?: AgentUsage;
  readonly showUsage?: boolean;
  readonly settings?: ReactNode;
  readonly steeringMode?: SteeringMode;
  readonly onSteeringModeChange?: (mode: SteeringMode) => void;
  readonly status: AgentStatus;
  readonly pausedNotice?: string;
  readonly running: boolean;
  readonly queueWhileRunning?: boolean;
  readonly queueWhileStarting?: boolean;
  readonly starting?: boolean;
  readonly controlsUnavailable?: boolean;
  readonly requestPending?: boolean;
  readonly acceptedMessage?: string;
  readonly acceptedSteeringMessage?: string;
  readonly acceptedInterruptionMessage?: string;
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
  editorRef,
  suggestionScope,
  draft,
  availableCommands,
  usage,
  showUsage = false,
  settings,
  steeringMode = 'queue',
  onSteeringModeChange,
  status,
  pausedNotice,
  running,
  queueWhileRunning = false,
  queueWhileStarting = false,
  starting = false,
  controlsUnavailable = false,
  requestPending = false,
  acceptedMessage,
  acceptedSteeringMessage,
  acceptedInterruptionMessage,
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
  const previousSteering = useRef(acceptedSteeringMessage);
  const previousInterruption = useRef(acceptedInterruptionMessage);
  const [submitting, setSubmitting] = useState(false);
  const [commandError, setCommandError] = useState<string>();
  const [handoff, setHandoff] = useState<{ attachments: readonly DraftAttachment[] }>();
  const editVersion = useRef(0);
  const submitted = useRef<{ message: string; version: number; clearsDraft: boolean } | undefined>(
    undefined,
  );
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
  // Retain the original draft while its outgoing row owns the presentation.
  // A failed/uncertain request reveals it again unless the owner has edited.
  const visibleContent = handoff === undefined ? content : '';
  const visibleAttachments =
    handoff === undefined
      ? attachments.items
      : attachments.items.filter(
          (item) => !handoff.attachments.some(({ key }) => key === item.key),
        );
  const messageContent = attachmentMessage(visibleContent, visibleAttachments);
  // Outside a turn both steering modes send ordinary input, so the choice is
  // only offered while there is a turn to queue behind or interrupt.
  const steering = running && queueWhileRunning && onSteeringModeChange !== undefined;
  const commands = useComposerCommands(
    visibleContent,
    availableCommands,
    usable && !inputBlocked && !busy && !submitting && !requestPending,
    inputRef,
    (value) => {
      editorRef?.current?.discardUndo();
      editVersion.current += 1;
      setCommandError(undefined);
      setContent(value);
    },
  );

  useLayoutEffect(() => {
    const changed = [
      acceptedMessage === previousAccepted.current ? undefined : acceptedMessage,
      acceptedSteeringMessage === previousSteering.current ? undefined : acceptedSteeringMessage,
      acceptedInterruptionMessage === previousInterruption.current
        ? undefined
        : acceptedInterruptionMessage,
    ].filter((value) => value !== undefined);
    previousAccepted.current = acceptedMessage;
    previousSteering.current = acceptedSteeringMessage;
    previousInterruption.current = acceptedInterruptionMessage;
    // A receipt from an earlier visit cannot own text newly typed on this visit.
    if (submitted.current === undefined && editVersion.current !== 0) return;
    for (const accepted of changed) {
      if (
        submitted.current !== undefined &&
        submitted.current.message === accepted &&
        (!submitted.current.clearsDraft || submitted.current.version !== editVersion.current)
      )
        continue;
      if (attachmentMessage(content, attachments.items) === accepted) {
        setContent('');
        attachments.accept(attachments.items);
      }
    }
  }, [
    acceptedMessage,
    acceptedSteeringMessage,
    acceptedInterruptionMessage,
    content,
    setContent,
    attachments,
  ]);

  useEffect(() => {
    if (usable) inputRef.current?.focus({ preventScroll: true });
  }, [usable]);

  useLayoutEffect(() => {
    const field = inputRef.current;
    if (field !== null) fit(field);
  }, [visibleContent]);

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
      (running && steeringMode === 'interrupt' && controlsUnavailable) ||
      starting ||
      inputBlocked ||
      requestPending ||
      (quickMessage === undefined && attachments.blocked) ||
      sending.current
    )
      return;
    const refusal = commandRefusal(message, availableCommands, visibleAttachments.length > 0);
    setCommandError(refusal);
    if (refusal) return;
    editorRef?.current?.discardUndo();
    const originalContent = content;
    const submittedAttachments = attachments.items;
    const version = editVersion.current;
    const pending = { attachments: submittedAttachments };
    submitted.current = { message, version, clearsDraft: quickMessage === undefined };
    sending.current = true;
    setSubmitting(true);
    if (quickMessage === undefined) setHandoff(pending);
    const finish = () => {
      setHandoff((current) => (current === pending ? undefined : current));
      sending.current = false;
      setSubmitting(false);
    };
    try {
      const sendingRequest = onSend(message, () => {
        if (quickMessage === undefined) {
          if (editVersion.current === version) {
            setContent((current) => (current === originalContent ? '' : current));
            attachments.accept(submittedAttachments);
          }
          setHandoff((current) => (current === pending ? undefined : current));
        }
      });
      void Promise.resolve(sendingRequest).then(finish, finish);
    } catch {
      // The parent owns delivery errors; never lose a draft to a synchronous refusal.
      finish();
    }
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
      <ComposerSuggestionEditor
        key={suggestionScope}
        editorRef={editorRef}
        inputRef={inputRef}
        content={content}
        onChange={(text) => {
          editVersion.current += 1;
          setCommandError(undefined);
          setHandoff(undefined);
          setContent(text);
        }}
      />
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
      {commands.panel}
      <Presence>
        {commandError ? (
          <p className="agent-composer__notice" role="alert">
            {commandError}
          </p>
        ) : null}
      </Presence>
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
        <div className="agent-composer__box">
          <ComposerAttachments
            {...attachments}
            items={visibleAttachments}
            enabled={attachmentEnabled && uploadAttachment !== undefined}
            locked={submitting || requestPending}
          />
          <textarea
            ref={inputRef}
            id="agent-message"
            rows={1}
            value={visibleContent}
            disabled={!usable}
            aria-describedby={hasNotice ? noticeId : undefined}
            aria-autocomplete="list"
            aria-haspopup="listbox"
            aria-expanded={commands.open}
            aria-controls={commands.open ? commands.id : undefined}
            aria-activedescendant={commands.activeId}
            placeholder={preparing ? 'Message Agent' : placeholder(status)}
            onChange={(event) => {
              editorRef?.current?.discardUndo();
              editVersion.current += 1;
              setCommandError(undefined);
              if (handoff !== undefined) attachments.accept(handoff.attachments);
              setHandoff(undefined);
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
              commands.onBlur();
            }}
            onFocus={commands.onFocus}
            onKeyDown={(event) => {
              // Safari can end composition before the confirming Enter and reports keyCode 229.
              if (
                composing.current ||
                event.nativeEvent.isComposing ||
                event.nativeEvent.keyCode === 229
              )
                return;
              if (commands.onKeyDown(event)) return;
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault();
                submit();
              }
            }}
          />
          {/*
            Every control lives in one row inside the box, placed by grid area
            so a row that wraps on a narrow screen keeps Send at the far end.
          */}
          <div className="agent-composer__controls" data-steering={steering ? '' : undefined}>
            {uploadAttachment === undefined ? null : (
              <span className="agent-composer__attach">
                <ComposerAttachmentPicker
                  add={attachments.add}
                  enabled={attachmentEnabled && uploadAttachment !== undefined}
                  locked={submitting || requestPending}
                />
              </span>
            )}
            {settings ? <div className="agent-composer__settings">{settings}</div> : null}
            {steering && onSteeringModeChange !== undefined ? (
              <SteeringModePicker value={steeringMode} onChange={onSteeringModeChange} />
            ) : null}
            {showUsage ? (
              <span className="agent-composer__ring">
                <ContextRing usage={usage} />
              </span>
            ) : null}
            <div className="agent-composer__actions">
              {!running || queueWhileRunning ? (
                <Button
                  placement="agent-composer__submit"
                  variant="primary"
                  type="submit"
                  icon={<IconSendOutline16 />}
                  aria-label={
                    running
                      ? steeringMode === 'queue'
                        ? 'Queue message'
                        : 'Interrupt with message'
                      : 'Send message'
                  }
                  title={
                    running
                      ? steeringMode === 'queue'
                        ? 'Queue message'
                        : 'Interrupt with message'
                      : 'Send message'
                  }
                  disabled={
                    !usable ||
                    busy ||
                    submitting ||
                    starting ||
                    inputBlocked ||
                    requestPending ||
                    messageContent === '' ||
                    attachments.blocked ||
                    (running && steeringMode === 'interrupt' && controlsUnavailable)
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
            </div>
          </div>
        </div>
      </form>
    </div>
  );
}
