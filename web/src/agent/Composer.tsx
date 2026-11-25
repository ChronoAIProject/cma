import { Presence } from '../ui/Presence';
import { useConversationFocus } from './environment/useConversationFocus';
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { AgentStatus } from '../api/types';
import type { ConnectionReport } from '../connections/client';
import { Button } from '../ui/Button';
import { IconSendOutline16 } from '../ui/icons';
import type { SandboxInputAvailability } from './sandboxInput';
import type { TriggerQuickMessage } from '@cma/client';
import { QuickMessages } from './QuickMessages';
import type { AgentDraft } from './draft';
import { ComposerDraftNotice } from './ComposerDraftNotice';
import {
  canMessage,
  placeholder,
  FirstTextSendNotice,
  ComposerStatusNotice,
} from './composerPresentation';
import type { FirstTextSend } from './useFirstTextSend';
import {
  attachmentMessage,
  transferredFiles,
  type DraftAttachment,
  type UploadAttachment,
} from './attachments';
import { useComposerAttachments } from './useComposerAttachments';
import { useComposerResize } from './useComposerResize';
import { ComposerAttachmentPicker, ComposerAttachments } from './ComposerAttachments';
import { useComposerCommands } from './ComposerCommands';
import { commandRefusal, type SessionCommand } from './slashCommands';
import { SteeringModePicker, type SteeringMode } from './SteeringMode';
import './Composer.css';
import type { ResponseControlAvailability } from './controlReadiness';
import { ResponseStop, useResponseStop } from './ResponseStop';
import {
  ComposerSuggestionEditor,
  type ComposerSuggestionEditorHandle,
} from './ComposerSuggestionEditor';

interface ComposerProps {
  readonly editorRef?: React.RefObject<ComposerSuggestionEditorHandle | null>;
  readonly suggestionScope?: string;
  readonly draft?: AgentDraft;
  readonly availableCommands?: readonly SessionCommand[];
  readonly settings?: ReactNode;
  readonly steeringMode?: SteeringMode;
  readonly onSteeringModeChange?: (mode: SteeringMode) => void;
  readonly status: AgentStatus;
  readonly onRestart?: () => Promise<void>;
  readonly pausedNotice?: string;
  readonly running: boolean;
  readonly queueWhileRunning?: boolean;
  readonly queueWhileStarting?: boolean;
  readonly starting?: boolean;
  readonly controlsUnavailable?: boolean;
  readonly responseControls?: ResponseControlAvailability;
  readonly requestPending?: boolean;
  readonly acceptedMessage?: string;
  readonly acceptedSteeringMessage?: string;
  readonly acceptedInterruptionMessage?: string;
  readonly inputAvailability: SandboxInputAvailability;
  readonly busy: boolean;
  readonly quickMessages?: readonly TriggerQuickMessage[];
  readonly uploadAttachment?: UploadAttachment;
  readonly firstTextSend?: FirstTextSend;
  readonly onSend: (
    content: string,
    onAccepted: () => void,
    verified?: ConnectionReport,
  ) => Promise<void> | void;
  readonly onStop: () => Promise<void> | void;
  /** The interrupt request is pending; the active turn remains server-authoritative. */
  readonly stopping?: boolean;
}

export function Composer({
  editorRef,
  suggestionScope,
  draft,
  availableCommands,
  settings,
  steeringMode = 'queue',
  onSteeringModeChange,
  status,
  onRestart,
  pausedNotice,
  running,
  queueWhileRunning = false,
  queueWhileStarting = false,
  starting = false,
  controlsUnavailable = false,
  responseControls,
  requestPending = false,
  acceptedMessage,
  acceptedSteeringMessage,
  acceptedInterruptionMessage,
  inputAvailability,
  busy,
  quickMessages = [],
  uploadAttachment,
  firstTextSend,
  onSend,
  onStop,
  stopping = false,
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
  const [inputFocused, setInputFocused] = useState(false);
  const [inputComposing, setInputComposing] = useState(false);
  const stopControl = useResponseStop({
    running,
    availability: responseControls ?? (controlsUnavailable ? 'unbound' : 'available'),
    busy,
    stopping,
    onStop,
  });
  const noticeId = useId();
  const inputBlocked = !inputAvailability.allowed || draft?.recoveryBlocked === true;
  const preparing = status === 'starting' && queueWhileStarting;
  const notice = inputAvailability.notice ?? pausedNotice;
  const hasNotice = notice !== undefined || inputAvailability.savingStartedAt !== undefined;
  const usable = canMessage(status) || preparing;
  const attachmentEnabled =
    (status === 'ready' || status === 'working') && !inputBlocked && !starting && !busy;
  const attachments = useComposerAttachments(draft, uploadAttachment, attachmentEnabled);
  // Hide the draft during handoff and retain it if sending fails.
  const visibleContent = handoff === undefined ? content : '';
  const visibleAttachments =
    handoff === undefined
      ? attachments.items
      : attachments.items.filter(
          (item) => !handoff.attachments.some(({ key }) => key === item.key),
        );
  const messageContent = attachmentMessage(visibleContent, visibleAttachments);
  const canPrepareFirstSend =
    firstTextSend?.available === true &&
    attachments.items.length === 0 &&
    !messageContent.startsWith('/') &&
    draft?.recoveryBlocked !== true;
  // Outside a turn both steering modes send ordinary input, so the choice is
  // only offered while there is a turn to queue behind or interrupt.
  const steering = running && queueWhileRunning && onSteeringModeChange !== undefined;
  const sendLabel = running
    ? steeringMode === 'queue'
      ? 'Queue message'
      : 'Interrupt with message'
    : 'Send message';
  const canEdit = usable && !inputBlocked && !busy && !submitting && !requestPending;
  const controlsBlocked =
    running && (!queueWhileRunning || (steeringMode === 'interrupt' && controlsUnavailable));
  useConversationFocus(inputRef, canEdit && !controlsBlocked && !starting);
  const commands = useComposerCommands(
    visibleContent,
    availableCommands,
    canEdit,
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
    if (draft?.beginSubmission) return;
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
    draft,
  ]);

  useEffect(() => {
    // History can finish after the owner has already focused a computer tab.
    // Initial readiness must not take keyboard input away from that control.
    if (usable && (!document.activeElement || document.activeElement === document.body))
      inputRef.current?.focus({ preventScroll: true });
  }, [usable]);

  useComposerResize(inputRef, visibleContent);
  const submit = (quickMessage?: string) => {
    const message = quickMessage ?? messageContent;
    const prepare = quickMessage === undefined && canPrepareFirstSend;
    if (
      message === '' ||
      composing.current ||
      !usable ||
      busy ||
      (running && !queueWhileRunning) ||
      (running && steeringMode === 'interrupt' && controlsUnavailable) ||
      starting ||
      (inputBlocked && !prepare) ||
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
    const send = (verified?: ConnectionReport) => {
      const pending = { attachments: submittedAttachments };
      const submission = quickMessage === undefined ? draft?.beginSubmission?.(message) : undefined;
      submitted.current = {
        message,
        version,
        clearsDraft: quickMessage === undefined,
      };
      sending.current = true;
      setSubmitting(true);
      if (quickMessage === undefined) setHandoff(pending);
      const finish = () => {
        submission?.settled();
        setHandoff((current) => (current === pending ? undefined : current));
        sending.current = false;
        setSubmitting(false);
      };
      try {
        const accepted = () => {
          if (quickMessage === undefined) {
            if (editVersion.current === version) {
              if (submission) submission.accepted();
              else {
                setContent((current) => (current === originalContent ? '' : current));
                attachments.accept(submittedAttachments);
              }
            }
            setHandoff((current) => (current === pending ? undefined : current));
          }
        };
        const sendingRequest = verified
          ? onSend(message, accepted, verified)
          : onSend(message, accepted);
        void Promise.resolve(sendingRequest).then(finish, finish);
      } catch {
        // The parent owns delivery errors; never lose a draft to a synchronous refusal.
        finish();
      }
    };
    if (prepare)
      firstTextSend.start(
        () => editVersion.current === version && inputRef.current?.value === originalContent,
        send,
      );
    else send();
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
        hasAttachments={attachments.items.length > 0}
        restoreDisabled={!usable || inputBlocked || submitting || requestPending}
        onChange={(text) => {
          firstTextSend?.cancel();
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
      {status === 'stopped' && onRestart ? (
        <Button size="sm" disabled={busy || requestPending} onClick={() => void onRestart()}>
          Restart Agent
        </Button>
      ) : null}
      <ComposerDraftNotice draft={draft} />
      {firstTextSend?.pending && <FirstTextSendNotice onCancel={firstTextSend.cancel} />}
      <Presence>
        {commandError ? (
          <p className="agent-composer__notice" role="alert">
            {commandError}
          </p>
        ) : null}
      </Presence>
      <ComposerStatusNotice
        id={noticeId}
        notice={notice}
        savingStartedAt={inputAvailability.savingStartedAt}
      />
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
            placeholder={
              preparing
                ? 'Message Agent'
                : running && queueWhileRunning
                  ? "Add a follow-up — it'll be queued while the agent works"
                  : placeholder(status, onRestart !== undefined)
            }
            onChange={(event) => {
              firstTextSend?.cancel();
              editorRef?.current?.discardUndo();
              editVersion.current += 1;
              setCommandError(undefined);
              if (handoff !== undefined) attachments.accept(handoff.attachments);
              setHandoff(undefined);
              setContent(event.target.value);
            }}
            onCompositionStart={() => {
              composing.current = true;
              setInputComposing(true);
            }}
            onCompositionEnd={() => {
              composing.current = false;
              setInputComposing(false);
            }}
            onBlur={() => {
              composing.current = false;
              setInputComposing(false);
              setInputFocused(false);
              commands.onBlur();
            }}
            onFocus={() => {
              setInputFocused(true);
              commands.onFocus();
            }}
            onKeyDown={(event) => {
              // Safari can end composition before the confirming Enter and reports keyCode 229.
              if (
                composing.current ||
                event.nativeEvent.isComposing ||
                event.nativeEvent.keyCode === 229
              )
                return;
              if (commands.onKeyDown(event)) return;
              if (event.key === 'Escape' && stopControl.enabled && !event.defaultPrevented) {
                event.preventDefault();
                stopControl.stop();
                return;
              }
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault();
                submit();
              }
            }}
          />
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
            <div className="agent-composer__actions">
              {!running || (messageContent !== '' && queueWhileRunning) ? (
                <Button
                  placement="agent-composer__submit"
                  variant="primary"
                  type="submit"
                  icon={<IconSendOutline16 />}
                  aria-label={sendLabel}
                  title={sendLabel}
                  disabled={
                    !usable ||
                    (running && !queueWhileRunning) ||
                    busy ||
                    submitting ||
                    starting ||
                    (inputBlocked && !canPrepareFirstSend) ||
                    requestPending ||
                    messageContent === '' ||
                    attachments.blocked ||
                    (running && steeringMode === 'interrupt' && controlsUnavailable)
                  }
                  aria-describedby={hasNotice ? noticeId : undefined}
                />
              ) : null}
              {running && (messageContent === '' || !queueWhileRunning) ? (
                <ResponseStop
                  control={stopControl}
                  shortcut={
                    stopControl.enabled && inputFocused && !inputComposing && !commands.open
                  }
                />
              ) : null}
            </div>
          </div>
        </div>
      </form>
    </div>
  );
}
