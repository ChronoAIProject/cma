import { useState } from 'react';
import { useCmaClient } from '../../api/context';
import { Button } from '../../ui/Button';
import { IconFolderOpen16 } from '../../ui/icons';
import { FileExplorer } from './FileExplorer';
export { FileExplorer } from './FileExplorer';

export function AgentFiles({
  agentId,
  active,
  readOnly = false,
  onAccessChanged,
}: {
  readonly agentId: string;
  readonly active: boolean;
  readonly readOnly?: boolean;
  readonly onAccessChanged?: () => void;
}) {
  const files = useCmaClient().files;
  const [open, setOpen] = useState(false);
  if (!files) return null;
  return (
    <section className="agent-details__band" aria-label="Agent files">
      <h2>Files</h2>
      <p>
        <code>/home/</code>
      </p>
      <Button icon={<IconFolderOpen16 />} onClick={() => setOpen(true)}>
        Browse files
      </Button>
      {open && active ? (
        <FileExplorer
          key={agentId}
          client={files}
          agentId={agentId}
          readOnly={readOnly}
          onClose={() => setOpen(false)}
          onAccessChanged={onAccessChanged}
        />
      ) : null}
    </section>
  );
}
