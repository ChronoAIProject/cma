import { useCallback, useEffect, useId, useRef, useState, type SetStateAction } from 'react';
import { Button } from '../ui/Button';
import { IconCloseOutline16, IconPaperclipOutline16 } from '../ui/icons';
import { usePresenceExiting } from '../ui/Presence';
import {
  attachmentLimit,
  attachmentMarker,
  splitAttachmentMessage,
  transferredFiles,
  type DraftAttachment,
  type UploadAttachment,
} from './attachments';
import { ComposerAttachmentPicker, ComposerAttachments } from './ComposerAttachments';
import { useComposerAttachments } from './useComposerAttachments';

/** Existing attachments remain server-issued references; only new files are uploaded. */
export function QueuedMessageEditor({
  input,
  disabled,
  unavailable = false,
  uploadAttachment,
  onSave,
  onCancel,
}: {
  readonly input: string;
  readonly disabled: boolean;
  readonly unavailable?: boolean;
  readonly uploadAttachment?: UploadAttachment;
  readonly onSave: (input: string) => Promise<void>;
  readonly onCancel: () => void;
}) {
  const [original] = useState(() => splitAttachmentMessage(input));
  const [text, setText] = useState(original?.text ?? input);
  const [existing, setExisting] = useState(original?.files ?? []);
  const [files, setFiles] = useState<readonly DraftAttachment[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);
  const submitting = useRef(false);
  const field = useRef<HTMLTextAreaElement>(null);
  const composing = useRef(false);
  const label = useId();
  const exiting = usePresenceExiting();
  const localLocked = saving || exiting;
  const locked = disabled || localLocked;
  const remaining = attachmentLimit - existing.length;
  const setAttachments = useCallback(
    (update: SetStateAction<readonly DraftAttachment[]>) => {
      setFiles((current) => {
        const next = typeof update === 'function' ? update(current) : update;
        return next.length <= remaining ? next : current;
      });
    },
    [remaining],
  );
  const attachments = useComposerAttachments(
    { content: text, setContent: setText, attachments: files, setAttachments },
    uploadAttachment,
    !locked,
  );
  useEffect(() => {
    field.current?.focus();
  }, []);

  const add = (incoming: readonly File[]) => {
    if (locked || unavailable) return;
    if (existing.length + attachments.items.length + incoming.length > attachmentLimit) {
      setError(`Attach up to ${String(attachmentLimit)} files per message.`);
      return;
    }
    setError(undefined);
    attachments.add(incoming);
  };
  function draftMessage() {
    const added = attachments.items.flatMap((item) =>
      item.uploaded
        ? [
            {
              name: item.uploaded.name,
              path: item.uploaded.path,
              media_type: item.uploaded.mediaType,
              size_bytes: item.uploaded.size,
            },
          ]
        : [],
    );
    const manifest = [...existing, ...added];
    return manifest.length
      ? `${text.trim() || 'Please review the attached files.'}${attachmentMarker}${JSON.stringify(manifest, null, 2)}`
      : text.trim();
  }
  async function copy() {
    if (localLocked) return;
    setError(undefined);
    try {
      await navigator.clipboard.writeText(draftMessage());
      setCopied(true);
    } catch {
      setError('The draft could not be copied. It is still kept here; try again.');
    }
  }
  async function save() {
    if (unavailable || locked || submitting.current || attachments.blocked) return;
    const content = draftMessage();
    if (!content) return;
    submitting.current = true;
    setSaving(true);
    setError(undefined);
    try {
      await onSave(content);
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : 'The update could not be confirmed. Your edit is kept here.',
      );
    } finally {
      submitting.current = false;
      setSaving(false);
    }
  }
  return (
    <form
      className="queued-message-editor"
      aria-label={unavailable ? 'Retained message draft' : 'Edit queued message'}
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
      onPaste={(event) => {
        const incoming = transferredFiles(event.clipboardData);
        if (incoming.length) {
          event.preventDefault();
          add(incoming);
        }
      }}
      onDragOver={(event) => {
        if (Array.from(event.dataTransfer.types).includes('Files')) {
          event.preventDefault();
          event.dataTransfer.dropEffect =
            unavailable || locked || !uploadAttachment ? 'none' : 'copy';
        }
      }}
      onDrop={(event) => {
        const incoming = transferredFiles(event.dataTransfer);
        if (incoming.length) {
          event.preventDefault();
          add(incoming);
        }
      }}
    >
      <label className="sr-only" htmlFor={label}>
        {unavailable ? 'Retained draft' : 'Queued message'}
      </label>
      <textarea
        ref={field}
        id={label}
        value={text}
        rows={3}
        disabled={unavailable ? localLocked : locked}
        readOnly={unavailable}
        onChange={(event) => setText(event.target.value)}
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
          if (
            composing.current ||
            event.nativeEvent.isComposing ||
            event.nativeEvent.keyCode === 229
          )
            return;
          if (event.key === 'Escape' && !(unavailable ? localLocked : locked)) {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          }
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void save();
          }
        }}
      />
      {existing.length ? (
        <ul className="queued-message-editor__files" aria-label="Saved attachments">
          {existing.map((file, index) => (
            <li key={`${String(index)}:${file.path}`}>
              <IconPaperclipOutline16 />
              <span title={file.path}>{file.name}</span>
              <small>Uploaded</small>
              <Button
                size="sm"
                icon={<IconCloseOutline16 />}
                aria-label={`Remove ${file.name}`}
                disabled={locked || unavailable}
                onClick={() =>
                  setExisting((current) => current.filter((_, position) => position !== index))
                }
              />
            </li>
          ))}
        </ul>
      ) : null}
      <ComposerAttachments
        {...attachments}
        error={attachments.error}
        enabled={!locked && !unavailable && uploadAttachment !== undefined}
        locked={locked || unavailable}
      />
      {unavailable ? (
        <p className="queued-messages__error" role="status">
          This message is no longer queued and cannot be edited. Your draft and attachments are kept
          here. Copy the draft before dismissing it.
          {attachments.blocked ? ' Unfinished uploads are not included when copying.' : ''}
          {copied ? ' Draft copied.' : ''}
        </p>
      ) : null}
      {error ? (
        <p className="queued-messages__error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="queued-message-editor__actions">
        {uploadAttachment && !unavailable ? (
          <ComposerAttachmentPicker enabled={!locked} locked={locked} add={add} />
        ) : null}
        {unavailable ? (
          <>
            {attachments.items
              .filter((item) => item.status === 'error')
              .map((item) => (
                <Button
                  key={item.key}
                  size="sm"
                  disabled={locked}
                  onClick={() => attachments.retry(item.key)}
                >
                  Retry {item.file.name}
                </Button>
              ))}
            <Button size="sm" disabled={localLocked} onClick={() => void copy()}>
              Copy draft
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            variant="primary"
            type="submit"
            disabled={
              locked ||
              attachments.blocked ||
              (!text.trim() && existing.length + attachments.items.length === 0)
            }
          >
            {saving ? 'Saving…' : 'Save'}
          </Button>
        )}
        <Button size="sm" disabled={unavailable ? localLocked : locked} onClick={onCancel}>
          {unavailable ? 'Dismiss' : 'Cancel'}
        </Button>
      </div>
    </form>
  );
}
