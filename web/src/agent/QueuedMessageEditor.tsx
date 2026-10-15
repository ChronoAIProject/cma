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
  uploadAttachment,
  onSave,
  onCancel,
}: {
  readonly input: string;
  readonly disabled: boolean;
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
  const submitting = useRef(false);
  const field = useRef<HTMLTextAreaElement>(null);
  const composing = useRef(false);
  const label = useId();
  const exiting = usePresenceExiting();
  const locked = disabled || saving || exiting;
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
    if (locked) return;
    if (existing.length + attachments.items.length + incoming.length > attachmentLimit) {
      setError(`Attach up to ${String(attachmentLimit)} files per message.`);
      return;
    }
    setError(undefined);
    attachments.add(incoming);
  };
  async function save() {
    if (locked || submitting.current || attachments.blocked) return;
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
    if (!text.trim() && manifest.length === 0) return;
    const content = manifest.length
      ? `${text.trim() || 'Please review the attached files.'}${attachmentMarker}${JSON.stringify(manifest, null, 2)}`
      : text.trim();
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
      aria-label="Edit queued message"
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
          event.dataTransfer.dropEffect = locked || !uploadAttachment ? 'none' : 'copy';
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
        Queued message
      </label>
      <textarea
        ref={field}
        id={label}
        value={text}
        rows={3}
        disabled={locked}
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
          if (event.key === 'Escape' && !locked) {
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
                disabled={locked}
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
        enabled={!locked && uploadAttachment !== undefined}
        locked={locked}
      />
      {error ? (
        <p className="queued-messages__error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="queued-message-editor__actions">
        {uploadAttachment ? (
          <ComposerAttachmentPicker enabled={!locked} locked={locked} add={add} />
        ) : null}
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
        <Button size="sm" disabled={locked} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
