import { useState } from 'react';
import { useCmaClient } from '../../api/context';
import { Presence } from '../../ui/Presence';
import { FileExplorer } from './FileExplorer';
import { homeFilePath } from '../fileChanges';
import { FileRoots } from './FilePaths';
export { FileExplorer } from './FileExplorer';

export function AgentFiles({
  agentId,
  active,
  readOnly = false,
  onAccessChanged,
  workingDirectory,
}: {
  readonly agentId: string;
  readonly active: boolean;
  readonly readOnly?: boolean;
  readonly onAccessChanged?: (downloadMessage?: string) => void;
  readonly workingDirectory?: string;
}) {
  const files = useCmaClient().files;
  const [open, setOpen] = useState(false);
  const directory = homeFilePath(workingDirectory ?? '');
  if (!files) return null;
  return (
    <section className="agent-details__band" aria-label="Agent files">
      <h2>Files</h2>
      <FileRoots directory={directory} />
      <button type="button" className="agent-details__link" onClick={() => setOpen(true)}>
        Browse files
      </button>
      {active ? (
        <Presence>
          {open ? (
            <FileExplorer
              key={agentId}
              client={files}
              agentId={agentId}
              initialDirectory={directory}
              readOnly={readOnly}
              onClose={() => setOpen(false)}
              onAccessChanged={onAccessChanged}
            />
          ) : null}
        </Presence>
      ) : null}
    </section>
  );
}
