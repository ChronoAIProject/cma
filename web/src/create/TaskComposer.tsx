import type { ReactNode, RefObject } from 'react';
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
  readonly children: ReactNode;
}) {
  const modifier = /Mac|iPhone|iPad/.test(navigator.userAgent) ? '⌘' : 'Ctrl';
  return (
    <div className="task-home__composer">
      <div className="task-home__toolbar">{children}</div>
      <div className="task-home__panel">
        <textarea
          className="task-home__input"
          aria-label={template ? 'Task preview' : 'Your task'}
          readOnly={template}
          aria-keyshortcuts={modifier === '⌘' ? 'Meta+Enter' : 'Control+Enter'}
          placeholder="What would you like your Agent to do?"
          ref={input}
          value={text}
          aria-invalid={validation !== undefined}
          aria-describedby={validation ? errorId : undefined}
          onChange={(event) => onChange(event.target.value)}
        />
        <div className="task-home__footer">
          <span className="task-home__shortcut">{modifier} ↵ to run</span>
          <Button
            type="submit"
            variant="primary"
            placement="task-home__send"
            aria-label="Start task"
            icon={<IconArrowUpOutline16 />}
            disabled={disabled || invalid || !text.trim()}
          />
        </div>
      </div>
    </div>
  );
}
