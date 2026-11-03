import { useImperativeHandle, useState, type RefObject } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';

export interface ComposerSuggestionEditorHandle {
  edit(text: string, onEdited?: (edited: boolean) => void): void;
  discardUndo(): void;
}

export function ComposerSuggestionEditor({
  editorRef,
  inputRef,
  content,
  onChange,
}: {
  readonly editorRef?: RefObject<ComposerSuggestionEditorHandle | null>;
  readonly inputRef: RefObject<HTMLTextAreaElement | null>;
  readonly content: string;
  readonly onChange: (content: string) => void;
}) {
  const [pending, setPending] = useState<{ text: string; onEdited?: (edited: boolean) => void }>();
  const [replacement, setReplacement] = useState<{
    before: string;
    after: string;
    onEdited?: (edited: boolean) => void;
  }>();
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
        description="Choose how to add the suggestion. Your attachments stay in the composer."
        footer={
          <>
            <Button onClick={cancel}>Cancel</Button>
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
            <Button
              onClick={() => {
                if (pending === undefined) return;
                onChange(`${content}\n${pending.text}`);
                setReplacement(undefined);
                pending.onEdited?.(true);
                setPending(undefined);
              }}
            >
              Append to draft
            </Button>
          </>
        }
      />
    </>
  );
}
