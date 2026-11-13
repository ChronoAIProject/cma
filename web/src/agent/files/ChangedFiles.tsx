import { useEffect, useRef, useState } from 'react';
import type { AgentFileEntry, AgentFilesClient } from '../../api/agentFiles';
import type { FileChange } from '../fileChanges';
import { FileChangeStats } from './FileChangeStats';

export function ChangedFiles({
  changes,
  directory,
  client,
  agentId,
  active,
  busy,
  onSelect,
  onError,
}: {
  readonly changes?: ReadonlyMap<string, FileChange>;
  readonly directory: string;
  readonly client: AgentFilesClient;
  readonly agentId: string;
  readonly active: boolean;
  readonly busy: boolean;
  readonly onSelect: (file: AgentFileEntry) => void;
  readonly onError: (cause: unknown) => void;
}) {
  const request = useRef<AbortController | undefined>(undefined);
  const [opening, setOpening] = useState<string>();
  useEffect(() => () => request.current?.abort(), [client, agentId, active]);
  const open = async (path: string) => {
    if (!active || busy || request.current) return;
    const controller = new AbortController();
    request.current = controller;
    setOpening(path);
    try {
      const parent = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
      const entries = await client.list(agentId, parent, { signal: controller.signal });
      if (controller.signal.aborted) return;
      const file = entries.find((entry) => entry.path === path && entry.kind === 'file');
      if (!file) throw new Error('The changed file is no longer available.');
      onSelect(file);
    } catch (error) {
      if (!controller.signal.aborted) onError(error);
    } finally {
      if (request.current === controller) {
        request.current = undefined;
        setOpening(undefined);
      }
    }
  };
  if (!changes?.size) return null;
  return (
    <section className="changed-files" aria-label="Changed files">
      <h3>Changed files</h3>
      <ul>
        {[...changes.values()].map((change) => (
          <li key={change.path}>
            <button
              type="button"
              className="changed-files__file"
              disabled={!active || busy || opening !== undefined}
              onClick={() => void open(change.path)}
              aria-label={`Open changed file ${change.path}`}
            >
              <span className="agent-files__file-label">
                <strong>{change.path.split('/').at(-1)}</strong>
                <span className="agent-files__file-path">
                  {directory && change.path.startsWith(directory + '/')
                    ? change.path.slice(directory.length + 1)
                    : change.path}
                </span>
              </span>
              <span className="agent-files__changes">
                <FileChangeStats change={change} />
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
