import { useImperativeHandle, useState, type RefObject } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';

export interface ComposerSuggestionEditorHandle {
  edit(text: string, onEdited?: (edited: boolean) => void): void;
  restore(text: string, onRestored: () => void): void;
  discardUndo(): void;
}

export function ComposerSuggestionEditor({
  editorRef,
  inputRef,
  content,
  onChange,
  hasAttachments = false,
  restoreDisabled = false,
}: {
  readonly editorRef?: RefObject<ComposerSuggestionEditorHandle | null>;
  readonly inputRef: RefObject<HTMLTextAreaElement | null>;
  readonly content: string;
  readonly onChange: (content: string) => void;
  readonly hasAttachments?: boolean;
  readonly restoreDisabled?: boolean;
}) {
  const [pending, setPending] = useState<{
    text: string;
    restoring?: boolean;
    onEdited?: (edited: boolean) => void;
  }>();
  const [replacement, setReplacement] = useState<{
    before: string;
    after: string;
    onEdited?: (edited: boolean) => void;
  }>();
  if (restoreDisabled && pending?.restoring) setPending(undefined);
  useImperativeHandle(editorRef, () => ({
    discardUndo: () => setReplacement(undefined),
    edit(text, onEdited) {
      inputRef.current?.focus({ preventScroll: true });
      if (content === '') {
        setReplacement(undefined);
        onChange(text);
        onEdited?.(true);
      } else setPending({ text, onEdited });
    },
    restore(text, onRestored) {
      if (restoreDisabled) return;
      inputRef.current?.focus({ preventScroll: true });
      if (content === '' && !hasAttachments) {
        setReplacement(undefined);
        onChange(text);
        onRestored();
      } else setPending({ text, restoring: true, onEdited: onRestored });
    },
  }));
  const cancel = () => setPending(undefined);
  return (
    <>
      {replacement && content === replacement.after && (
        <p className="agent-composer__notice" role="status">
          Draft text replaced.{' '}
          <Button
            size="sm"
            onClick={() => {
              onChange(replacement.before);
              replacement.onEdited?.(false);
              setReplacement(undefined);
              inputRef.current?.focus({ preventScroll: true });
            }}
          >
            Undo replacement
          </Button>
        </p>
      )}
      <Modal
        open={pending !== undefined}
        title="Keep your draft?"
        onClose={cancel}
        description={
          pending?.restoring
            ? 'Add this message to your current draft? Your text and attachments will be kept. Nothing will be sent.'
            : 'Choose how to add the suggestion. Your attachments stay in the composer.'
        }
        footer={
          <>
            {!pending?.restoring ? <Button onClick={cancel}>Cancel</Button> : null}
            {!pending?.restoring ? (
              <Button
                onClick={() => {
                  if (pending === undefined) return;
                  setReplacement({
                    before: content,
                    after: pending.text,
                    onEdited: pending.onEdited,
                  });
                  onChange(pending.text);
                  pending.onEdited?.(true);
                  setPending(undefined);
                }}
              >
                Replace text
              </Button>
            ) : null}
            <Button
              onClick={() => {
                if (pending === undefined) return;
                onChange(
                  `${content}${pending.restoring && content === '' ? '' : '\n'}${pending.text}`,
                );
                setReplacement(undefined);
                pending.onEdited?.(true);
                setPending(undefined);
              }}
            >
              {pending?.restoring ? 'Add to current draft' : 'Append to draft'}
            </Button>
            {pending?.restoring ? (
              <Button onClick={cancel} variant="primary" data-initial-focus="true">
                Keep editing
              </Button>
            ) : null}
          </>
        }
      />
    </>
  );
}
