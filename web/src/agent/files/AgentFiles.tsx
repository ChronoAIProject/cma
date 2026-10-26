import { useState } from 'react';
import { useCmaClient } from '../../api/context';
import { Presence } from '../../ui/Presence';
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
