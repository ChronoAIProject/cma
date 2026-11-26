import { useId } from 'react';
import { Input } from '../../ui/Input';
import { Button } from '../../ui/Button';

export function fileNameIssue(name: string): string | undefined {
  if (
    !name.trim() ||
    /[/\\]/.test(name) ||
    [...name].some((character) => character.charCodeAt(0) < 32) ||
    name === '.' ||
    name === '..'
  )
    return 'Enter a file or folder name without path separators.';
  if (new TextEncoder().encode(name).byteLength > 255)
    return 'File and folder names must be at most 255 UTF-8 bytes.';
}

export function FileCreateForm({
  kind,
  name,
  busy,
  onName,
  onCancel,
  onCreate,
}: {
  readonly kind: 'file' | 'directory';
  readonly name: string;
  readonly busy: boolean;
  readonly onName: (name: string) => void;
  readonly onCancel: () => void;
  readonly onCreate: () => void;
}) {
  const label = kind === 'directory' ? 'Folder name' : 'File name';
  const errorId = useId();
  const issue = fileNameIssue(name);
  return (
    <form
      className="agent-files__create"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy) return;
        if (issue) {
          event.currentTarget.querySelector('input')?.focus();
          return;
        }
        onCreate();
      }}
    >
      <label>
        {label}
        <Input
          aria-label={label}
          aria-invalid={issue ? true : undefined}
          aria-describedby={issue ? errorId : undefined}
          value={name}
          onChange={(event) => onName(event.target.value)}
        />
      </label>
      {issue ? <p id={errorId}>{issue}</p> : null}
      <Button size="sm" disabled={busy} onClick={onCancel}>
        Cancel
      </Button>
      <Button size="sm" variant="primary" type="submit" disabled={busy || !!issue}>
        Create
      </Button>
    </form>
  );
}
