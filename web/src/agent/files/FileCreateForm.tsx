import { Input } from '../../ui/Input';
import { Button } from '../../ui/Button';
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
  return (
    <form
      className="agent-files__create"
      onSubmit={(event) => {
        event.preventDefault();
        onCreate();
      }}
    >
      <label>
        {label}
        <Input
          aria-label={label}
          value={name}
          onChange={(event) => onName(event.target.value)}
          maxLength={255}
        />
      </label>
      <Button size="sm" disabled={busy} onClick={onCancel}>
        Cancel
      </Button>
      <Button size="sm" variant="primary" type="submit" disabled={busy || !name.trim()}>
        Create
      </Button>
    </form>
  );
}
