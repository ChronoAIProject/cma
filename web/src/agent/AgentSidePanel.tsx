import { useCallback, useState } from 'react';
import type { AgentDetail } from '../api/types';
import { useCmaClient } from '../api/context';
import { SharedAgentDetails } from '../resources/SharedAgentDetails';
import { useAgentAccess } from '../resources/agentAccess';
import type { ArtifactPublicationScope } from '../resources/artifactRecovery';
import type { AgentCommand } from './controller';
import { AgentDetails } from './Details';
import { FileExplorer } from './files/FileExplorer';
import { Button } from '../ui/Button';
import './files/AgentFiles.css';

export type AgentPanel = 'details' | 'files' | undefined;

/** Sibling of the conversation: resizing or switching this panel never remounts a stream. */
export function AgentSidePanel({
  agentId,
  shared,
  detail,
  panel,
  onClose,
  onAccessChanged,
  onOpenSandbox,
  pending,
  historyLoading,
  publicationScope,
}: {
  readonly agentId: string;
  readonly shared: boolean;
  readonly detail?: AgentDetail;
  readonly panel: AgentPanel;
  readonly onClose: () => void;
  readonly onAccessChanged: () => void;
  readonly onOpenSandbox?: () => void;
  readonly pending?: AgentCommand;
  readonly historyLoading?: boolean;
  readonly publicationScope: ArtifactPublicationScope;
}) {
  const client = useCmaClient();
  const filesOpen = panel === 'files';
  const [visited, setVisited] = useState(false);
  if (filesOpen && !visited) setVisited(true);
  // Shared file access is independently renewed and never inferred from a visible toolbar.
  const access = useAgentAccess(client.resourceViews, shared && filesOpen ? agentId : undefined);
  const authorized = access.state.status === 'ready' ? access.state.value.access : undefined;
  const reloadAccess = access.reload;
  const accessChanged = useCallback(() => {
    reloadAccess();
    onAccessChanged();
  }, [reloadAccess, onAccessChanged]);
  const mayRead = !shared || authorized?.capabilities.includes('view_files') === true;
  const mayWrite = !shared || authorized?.capabilities.includes('operate_agent') === true;
  return (
    <>
      <div
        className="agent-side-panel"
        hidden={panel !== 'details'}
        inert={panel !== 'details' || undefined}
      >
        {shared ? (
          <SharedAgentDetails
            agent={agentId}
            open={panel === 'details'}
            onClose={onClose}
            onAccessChanged={onAccessChanged}
            publicationScope={publicationScope}
          />
        ) : (
          <AgentDetails
            detail={detail}
            open={panel === 'details'}
            onClose={onClose}
            onOpenSandbox={onOpenSandbox}
            pending={pending}
            historyLoading={historyLoading}
          />
        )}
      </div>
      <div
        id="agent-files-panel"
        className="agent-side-panel"
        hidden={!filesOpen}
        inert={!filesOpen || undefined}
      >
        {visited && mayRead && client.files ? (
          <FileExplorer
            key={agentId}
            panel
            client={client.files}
            agentId={agentId}
            readOnly={!mayWrite}
            onClose={onClose}
            onAccessChanged={accessChanged}
          />
        ) : filesOpen ? (
          <aside className="agent-files-panel" aria-label="Agent files">
            <header className="agent-files-panel__header">
              <h2>Files</h2>
              <Button size="sm" onClick={onClose}>
                Close files
              </Button>
            </header>
            <div className="agent-files-panel__body">
              <p role="status">
                {shared && access.state.status === 'loading'
                  ? 'Checking file access…'
                  : access.state.status === 'error'
                    ? access.state.message
                    : 'File access is unavailable for this Agent.'}
              </p>
              {shared ? (
                <Button size="sm" onClick={access.reload}>
                  Check access again
                </Button>
              ) : null}
            </div>
          </aside>
        ) : null}
      </div>
    </>
  );
}
