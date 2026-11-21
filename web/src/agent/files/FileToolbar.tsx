import { useRef } from 'react';
import { Button } from '../../ui/Button';
import { IconRefreshOutline16, IconPlusOutline16, IconPaperclipOutline16 } from '../../ui/icons';

export function FileToolbar({
  hidden,
  onHidden,
  busy,
  blocked,
  denied,
  onRefresh,
  onCreate,
  onUpload,
}: {
  readonly hidden: boolean;
  readonly onHidden: (hidden: boolean) => void;
  readonly busy: boolean;
  readonly blocked: boolean;
  readonly denied: boolean;
  readonly onRefresh: () => void;
  readonly onCreate: (kind: 'file' | 'directory') => void;
  readonly onUpload: (files: readonly File[]) => void;
}) {
  const upload = useRef<HTMLInputElement>(null);
  return (
    <div className="agent-files__toolbar">
      <label className="agent-files__hidden">
        <input
          type="checkbox"
          checked={hidden}
          onChange={(event) => onHidden(event.target.checked)}
        />{' '}
        Show hidden
      </label>
      <Button
        size="sm"
        icon={<IconRefreshOutline16 />}
        aria-label="Refresh files"
        title="Refresh files"
        disabled={busy || denied}
        onClick={onRefresh}
      />
      {!blocked ? (
        <>
          <Button
            size="sm"
            icon={<IconPlusOutline16 />}
            disabled={busy}
            onClick={() => onCreate('file')}
          >
            New file
          </Button>
          <Button size="sm" disabled={busy} onClick={() => onCreate('directory')}>
            New folder
          </Button>
          <Button
            size="sm"
            icon={<IconPaperclipOutline16 />}
            disabled={busy}
            onClick={() => upload.current?.click()}
          >
            Upload
          </Button>
          <input
            ref={upload}
            type="file"
            hidden
            multiple
            aria-label="Upload files"
            onChange={(event) => {
              onUpload(Array.from(event.target.files ?? []));
              event.target.value = '';
            }}
          />
        </>
      ) : (
        <span className="agent-files__read-only">View only</span>
      )}
    </div>
  );
}
