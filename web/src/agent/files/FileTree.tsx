import { useId } from 'react';
import type { FileChange } from '../fileChanges';
import type { AgentFileEntry } from '../../api/agentFiles';
import {
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconCodeOutline16,
  IconFolderClose16,
  IconFolderOpen16,
  IconLinkOutline16,
} from '../../ui/icons';

export function FileTree({
  directories,
  expanded,
  hidden,
  selected,
  loading,
  disabled = false,
  onDirectory,
  changes,
  rootPath = '',
  onFile,
}: {
  readonly changes?: ReadonlyMap<string, FileChange>;
  readonly rootPath?: string;
  readonly directories: ReadonlyMap<string, readonly AgentFileEntry[]>;
  readonly expanded: ReadonlySet<string>;
  readonly hidden: boolean;
  readonly selected?: string;
  readonly loading: ReadonlySet<string>;
  readonly disabled?: boolean;
  readonly onDirectory: (path: string) => void;
  readonly onFile: (file: AgentFileEntry) => void;
}) {
  const metadataId = useId();
  const rows = (path: string, level: number) => (
    <ul className="agent-files__tree" aria-label={level === 0 ? 'Home files' : undefined}>
      {(directories.get(path) ?? [])
        .filter((file) => hidden || !file.hidden)
        .map((file) => {
          const change = changes?.get(file.path);
          const descriptionId = `${metadataId}-${encodeURIComponent(file.path)}`;
          const folder = file.kind === 'directory';
          const open = expanded.has(file.path);
          return (
            <li key={file.path}>
              <button
                type="button"
                className="agent-files__row"
                aria-expanded={folder ? open : undefined}
                aria-current={selected === file.path ? 'true' : undefined}
                title={`/home/${file.path}`}
                aria-label={file.name}
                aria-describedby={
                  change ? `${descriptionId} ${descriptionId}-changes` : descriptionId
                }
                onClick={() => (folder ? onDirectory(file.path) : onFile(file))}
                disabled={disabled || file.kind === 'symlink'}
              >
                {folder ? (
                  open ? (
                    <IconChevronDownOutline14 />
                  ) : (
                    <IconChevronRightOutline14 />
                  )
                ) : (
                  <span className="agent-files__indent" />
                )}
                {folder ? (
                  open ? (
                    <IconFolderOpen16 />
                  ) : (
                    <IconFolderClose16 />
                  )
                ) : file.kind === 'symlink' ? (
                  <IconLinkOutline16 />
                ) : (
                  <IconCodeOutline16 />
                )}
                <span className="agent-files__file-label">
                  <span>{file.name}</span>
                  <span id={descriptionId} className="agent-files__file-path">
                    /home/{file.path}
                  </span>
                </span>
                {change ? (
                  <span
                    className="agent-files__changes"
                    id={`${descriptionId}-changes`}
                    title="Lines changed in completed recorded edits"
                  >
                    <span className="agent-files__added">+{change.added}</span>
                    <span className="agent-files__removed">−{change.removed}</span>
                    <span aria-label="Modified by the Agent">M</span>
                  </span>
                ) : null}
              </button>
              {folder && open ? (
                <div className="agent-files__children">
                  {loading.has(file.path) ? (
                    <p role="status">Loading…</p>
                  ) : (
                    rows(file.path, level + 1)
                  )}
                  {!loading.has(file.path) && directories.get(file.path)?.length === 0 ? (
                    <p>Empty folder</p>
                  ) : null}
                </div>
              ) : null}
            </li>
          );
        })}
    </ul>
  );
  return rows(rootPath, 0);
}
