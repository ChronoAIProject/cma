import type { ComponentProps } from 'react';
import { IconFolderOpen16 } from '../../ui/icons';
import { FilePath } from './FilePaths';
import { FileTree } from './FileTree';

export function FileRail({
  directory,
  onRoot,
  ...tree
}: ComponentProps<typeof FileTree> & {
  readonly directory: string;
  readonly onRoot: () => void;
}) {
  const root = tree.rootPath ?? '';
  return (
    <nav className="agent-files__rail" aria-label="File explorer">
      <button
        type="button"
        className="agent-files__row"
        disabled={tree.disabled}
        onClick={onRoot}
        aria-current={directory === '' ? 'true' : undefined}
      >
        <IconFolderOpen16 />
        <span>Sandbox root /home/</span>
      </button>
      <FilePath label="Tree root" path={root} />
      {tree.loading.has(root) ? <p role="status">Loading files…</p> : null}
      <FileTree {...tree} />
      {!tree.loading.has(root) && tree.directories.get(root)?.length === 0 ? (
        <p>No files yet.</p>
      ) : null}
    </nav>
  );
}
