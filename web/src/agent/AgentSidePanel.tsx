import { useCallback, useEffect, useRef, useState } from 'react';
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
import type { ConversationState } from './model';
import { OperationOutput } from './environment/OperationOutput';
import { OperationReplay } from './environment/OperationReplay';
import './environment/AgentComputer.css';
import { FeatureAvailability, useAgentEnvironment } from './environment/availability';
import {
  useComputerObservation,
  useReplaySelection,
  type AgentComputerObservation,
} from './environment/observation';
import { BrowserPanel, EnableBrowser } from './environment/BrowserPanel';

export type AgentPanel = 'details' | 'files' | 'activity' | 'browser' | undefined;

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
  conversation,
  onSelect,
  observation,
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
  readonly conversation?: ConversationState;
  readonly onSelect?: (panel: AgentPanel) => void;
  readonly observation?: AgentComputerObservation;
}) {
  const client = useCmaClient();
  const filesOpen = panel === 'files';
  const computerOpen = panel !== undefined && panel !== 'details';
  const environment = useAgentEnvironment(client.files, agentId, computerOpen);
  const features = environment.state.status === 'ready' ? environment.state.value : undefined;
  const replaySelection = useReplaySelection(observation);
  const selection = useRef<
    { client: typeof client; agentId: string; tab: AgentPanel; open: boolean } | undefined
  >(undefined);
  useEffect(() => {
    const chosen =
      selection.current?.client === client && selection.current.agentId === agentId
        ? selection.current
        : undefined;
    if (!computerOpen) {
      if (chosen) selection.current = { ...chosen, open: false };
      return;
    }
    if (replaySelection && panel === 'activity') {
      selection.current = { client, agentId, tab: 'activity', open: true };
      return;
    }
    if (!features || chosen?.open) return;
    const tab =
      chosen?.tab ??
      (panel === 'files' && features.browser.state === 'available' ? 'browser' : panel);
    selection.current = { client, agentId, tab, open: true };
    if (tab !== panel && panel === 'files') onSelect?.(tab);
  }, [computerOpen, features, client, agentId, panel, onSelect, replaySelection]);
  const select = (tab: AgentPanel) => {
    selection.current = { client, agentId, tab, open: computerOpen };
    onSelect?.(tab);
  };
  const observed = useComputerObservation(observation);
  const operations = shared ? observed : (conversation ?? observed);
  const [visited, setVisited] = useState(false);
  if (filesOpen && !visited) setVisited(true);
  // Shared file access is independently renewed and never inferred from a visible toolbar.
  const access = useAgentAccess(
    client.resourceViews,
    shared && (visited || computerOpen) ? agentId : undefined,
  );
  const authorized = access.state.status === 'ready' ? access.state.value.access : undefined;
  const reloadAccess = access.reload;
  const accessChanged = useCallback(() => {
    reloadAccess();
    onAccessChanged();
  }, [reloadAccess, onAccessChanged]);
  const mayRead =
    (!shared || authorized?.capabilities.includes('view_files') === true) &&
    features?.files.state !== 'forbidden';
  const mayWrite = features
    ? features.files.writable
    : client.files?.environment && environment.state.status === 'loading'
      ? false
      : !shared || authorized?.capabilities.includes('operate_agent') === true;
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
      <aside
        aria-label="Agent computer"
        id="agent-files-panel"
        className="agent-computer"
        hidden={!computerOpen}
        inert={!computerOpen || undefined}
      >
        <header className="agent-files-panel__header">
          <h2>Agent computer</h2>
          <Button size="sm" onClick={onClose}>
            Back to conversation
          </Button>
        </header>
        <div className="agent-computer__tabs" role="tablist" aria-label="Agent computer views">
          {(['files', 'activity', 'browser'] as const).map((tab, index, tabs) => (
            <button
              key={tab}
              type="button"
              role="tab"
              id={`computer-tab-${tab}`}
              aria-controls={`computer-page-${tab}`}
              aria-selected={panel === tab}
              tabIndex={panel === tab ? 0 : -1}
              onClick={() => select(tab)}
              onKeyDown={(event) => {
                if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
                event.preventDefault();
                const next =
                  event.key === 'Home'
                    ? 0
                    : event.key === 'End'
                      ? 2
                      : (index + (event.key === 'ArrowRight' ? 1 : 2)) % 3;
                select(tabs[next]);
                document.getElementById(`computer-tab-${tabs[next]}`)?.focus();
              }}
            >
              {tab === 'files' ? 'Files' : tab === 'activity' ? 'Operation output' : 'Browser'}
            </button>
          ))}
        </div>
        <div
          id="computer-page-files"
          role="tabpanel"
          aria-labelledby="computer-tab-files"
          className="agent-computer__page agent-computer__files"
          hidden={!filesOpen}
          inert={!filesOpen || undefined}
        >
          <p className="agent-computer__scope">
            Authorized Sandbox files (/home); private runtime state excluded.
          </p>
          {features && features.files.state !== 'available' ? (
            <FeatureAvailability name="Files" feature={features.files} />
          ) : null}
          {visited && mayRead && client.files ? (
            <FileExplorer
              key={`${agentId}:${shared}`}
              panel
              embedded
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
        <div
          id="computer-page-activity"
          role="tabpanel"
          aria-labelledby="computer-tab-activity"
          className="agent-computer__page agent-computer__output"
          hidden={panel !== 'activity'}
        >
          {shared && authorized?.capabilities.includes('view_history') !== true ? (
            <>
              <p role="status">
                {access.state.status === 'loading'
                  ? 'Checking operation output access…'
                  : access.state.status === 'error'
                    ? 'Operation output access could not be checked. Try again.'
                    : 'Operation output access is not permitted.'}
              </p>
              {access.state.status === 'error' ? (
                <Button size="sm" onClick={access.reload}>
                  Check output access again
                </Button>
              ) : null}
            </>
          ) : features?.operation_output.state === 'forbidden' ? (
            <FeatureAvailability name="Operation output" feature={features.operation_output} />
          ) : client.operationReplay &&
            client.files?.environment &&
            environment.state.status !== 'ready' ? (
            <>
              <p role="status">
                {environment.state.status === 'loading'
                  ? 'Checking operation history access…'
                  : 'Operation history access could not be checked. Try again.'}
              </p>
              {environment.state.status === 'error' ? (
                <Button size="sm" onClick={environment.reload}>
                  Check history access again
                </Button>
              ) : null}
            </>
          ) : features?.replay.state === 'available' &&
            client.operationReplay &&
            panel === 'activity' ? (
            <OperationReplay
              key={`${agentId}:${observation?.scope.subject}:${features.policy_revision}`}
              reader={client.operationReplay}
              agentId={agentId}
              scope={observation?.scope.subject ?? 'owner'}
              toolCalls={operations?.toolCalls}
              selection={replaySelection}
              onSelectionHandled={observation?.clearReplaySelection}
            />
          ) : (
            <OperationOutput
              toolCalls={operations?.toolCalls}
              activeRunId={operations?.activeRunId}
              active={panel === 'activity'}
            />
          )}
        </div>
        <div
          id="computer-page-browser"
          role="tabpanel"
          aria-labelledby="computer-tab-browser"
          className="agent-computer__page agent-computer__output"
          hidden={panel !== 'browser'}
        >
          {features?.browser.state === 'available' &&
          features.runtime_incarnation &&
          client.browser &&
          (!shared || authorized?.capabilities.includes('view_history') === true) ? (
            <BrowserPanel
              key={`${agentId}:${features.runtime_incarnation}:${features.policy_revision}:${observation?.scope.subject}`}
              client={client.browser}
              controlAccountId={observation?.scope.subject}
              controlClient={
                !shared && features.control.state === 'available' && features.control.writable
                  ? client.browserControl
                  : undefined
              }
              agentId={agentId}
              incarnation={features.runtime_incarnation}
              active={panel === 'browser'}
              onReload={environment.reload}
            />
          ) : features ? (
            <FeatureAvailability name="Browser" feature={features.browser} />
          ) : (
            <p role="status">
              {!client.files?.environment || environment.state.status === 'ready'
                ? 'Environment capabilities not reported.'
                : environment.state.status === 'loading'
                  ? 'Checking environment capabilities…'
                  : environment.state.transient
                    ? 'Environment capabilities are temporarily unavailable. Try again.'
                    : 'Environment capabilities are unavailable. Try again.'}
            </p>
          )}
          {!shared &&
          features?.browser.state === 'not_enabled' &&
          features.runtime_incarnation &&
          client.browser ? (
            <EnableBrowser
              key={`${agentId}:${features.runtime_incarnation}`}
              client={client.browser}
              agentId={agentId}
              incarnation={features.runtime_incarnation}
              onEnabled={environment.reload}
            />
          ) : null}
          {environment.state.status === 'error' ? (
            <Button size="sm" onClick={environment.reload}>
              Check environment again
            </Button>
          ) : null}
        </div>
      </aside>
    </>
  );
}
