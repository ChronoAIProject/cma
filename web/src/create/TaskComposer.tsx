import { useId, type ReactNode, type RefObject } from 'react';
import { Button } from '../ui/Button';
import { IconArrowUpOutline16 } from '../ui/icons';

export function TaskComposer({
  text,
  input,
  validation,
  errorId,
  disabled,
  template = false,
  invalid = false,
  onChange,
  onClearTemplate,
  capacity,
  children,
}: {
  readonly text: string;
  readonly input: RefObject<HTMLTextAreaElement | null>;
  readonly validation?: string;
  readonly errorId: string;
  readonly disabled: boolean;
  readonly template?: boolean;
  readonly invalid?: boolean;
  readonly onChange: (text: string) => void;
  readonly onClearTemplate?: () => void;
  readonly capacity?: ReactNode;
  readonly children: ReactNode;
}) {
  const modifier = /Mac|iPhone|iPad/.test(navigator.userAgent) ? '⌘' : 'Ctrl';
  const shortcut = modifier === '⌘' ? 'Meta+Enter' : 'Control+Enter';
  const previewId = useId();
  const canSubmit = !disabled && !invalid && Boolean(text.trim());
  return (
    <div className="task-home__composer">
      <div className="task-home__panel">
        {template ? (
          <div className="task-home__preview">
            <h2 id={previewId}>Task preview · read-only</h2>
            <div
              className="task-home__preview-text"
              role="region"
              data-task-preview
              aria-labelledby={previewId}
              aria-keyshortcuts={canSubmit ? shortcut : undefined}
              tabIndex={text ? 0 : undefined}
            >
              {text || 'Fill in the required fields above to generate the task preview.'}
            </div>
          </div>
        ) : (
          <textarea
            className="task-home__input"
            aria-label="Your task"
            aria-keyshortcuts={shortcut}
            placeholder="Describe the job. It gets its own cloud computer to do it."
            ref={input}
            value={text}
            aria-invalid={validation !== undefined}
            aria-describedby={validation ? errorId : undefined}
            onChange={(event) => onChange(event.target.value)}
          />
        )}
        <div className="task-home__footer">
          <div className="task-home__toolbar">
            {children}
            <span className="task-home__model">Default model</span>
          </div>
          {template && onClearTemplate ? (
            <Button variant="ghost" placement="task-home__write" onClick={onClearTemplate}>
              Write a task without a template
            </Button>
          ) : null}
          {!template || canSubmit ? (
            <span className="task-home__shortcut">{modifier} ↵ to run</span>
          ) : null}
          {capacity}
          <Button
            type="submit"
            variant="primary"
            placement="task-home__send"
            aria-label="Start task"
            aria-keyshortcuts={template && canSubmit ? shortcut : undefined}
            icon={<IconArrowUpOutline16 />}
            disabled={!canSubmit}
          >
            {template ? 'Start task' : undefined}
          </Button>
        </div>
      </div>
    </div>
  );
}
