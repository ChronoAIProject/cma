import { useState } from 'react';
import { Button } from '../../ui/Button';
import { absoluteFilePath } from './filePath';

export function FilePath({ label, path }: { readonly label: string; readonly path?: string }) {
  return (
    <div className="agent-files__path" aria-label={label}>
      <span>{label}</span>
      {path === undefined ? (
        <span>Unknown</span>
      ) : (
        <code>{absoluteFilePath(path) ?? 'Unavailable'}</code>
      )}
    </div>
  );
}

export function FileRoots({ directory }: { readonly directory?: string }) {
  return (
    <div className="agent-files__roots">
      <FilePath label="Sandbox root" path="" />
      <FilePath label="Working directory" path={directory} />
    </div>
  );
}

export function SelectedFilePath({ path }: { readonly path: string }) {
  const absolute = absoluteFilePath(path);
  const [status, setStatus] = useState<string>();
  return (
    <div className="agent-files__selected-path">
      <FilePath label="Selected file" path={path} />
      {absolute === undefined ? null : (
        <Button
          size="sm"
          onClick={() => {
            void Promise.resolve()
              .then(() => navigator.clipboard.writeText(absolute))
              .then(
                () => setStatus('Path copied.'),
                () => setStatus('Could not copy. Select the full path above and copy it manually.'),
              );
          }}
        >
          Copy path
        </Button>
      )}
      {status ? <p role="status">{status}</p> : null}
    </div>
  );
}
