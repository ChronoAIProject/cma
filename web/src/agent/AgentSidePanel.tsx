import { screenOperation } from './screenOperations';
import { changedFiles, homeFilePath } from './fileChanges';
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
import { IconCloseOutline16, IconRefreshOutline14, IconFullscreenOutline16 } from '../ui/icons';
import './files/AgentFiles.css';
import type { ConversationState } from './model';
import { OperationOutput } from './environment/OperationOutput';
import './environment/AgentComputer.css';
import { FeatureAvailability, useAgentEnvironment } from './environment/availability';
import { useComputerObservation, type AgentComputerObservation } from './environment/observation';
import { BrowserPanel, EnableBrowser } from './environment/BrowserPanel';
import { useTaskExperience } from '../taskExperience';
import { agentInitialized } from './modelReadiness';
import { DesktopPanel } from './DesktopPanel';
import { desktopRuntimeReady } from './DesktopWorkspace';

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
  viewerId,
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
  readonly viewerId?: string;
}) {
  const client = useCmaClient();
  const policy = useTaskExperience(client, viewerId ?? observation?.scope.subject);
  const filesOpen = panel === 'files';
  const computerOpen = panel !== undefined && panel !== 'details';
  const machineAvailable = !shared && detail?.desktop === true;
  const computerRef = useRef<HTMLElement>(null);
  const [connectionRevision, reconnect] = useState(0);
  const [fullScreen, setFullScreen] = useState(false);
  const [screenError, setScreenError] = useState<string>();
  useEffect(() => {
    const changed = () => setFullScreen(document.fullscreenElement === computerRef.current);
    document.addEventListener('fullscreenchange', changed);
    return () => document.removeEventListener('fullscreenchange', changed);
  }, []);
  const [visitedScope, setVisitedScope] = useState<{
    client: typeof client;
    agentId: string;
    subject?: string;
    generation: number;
  }>();
  const subject = viewerId ?? observation?.scope.subject;
  const visited =
    visitedScope?.client === client &&
    visitedScope.agentId === agentId &&
    visitedScope.subject === subject;
  const environment = useAgentEnvironment(client.files, agentId, computerOpen || visited, subject);
  const features = environment.state.status === 'ready' ? environment.state.value : undefined;
  const readiness =
    detail &&
    (agentInitialized(detail.status, detail.readyAt) ? 'initialized' : detail.sandbox.status);
  const observedReadiness = useRef(readiness);
  const reloadEnvironment = environment.reload;
  useEffect(() => {
    if (observedReadiness.current === readiness) return;
    observedReadiness.current = readiness;
    reloadEnvironment();
  }, [readiness, reloadEnvironment]);
  const select = (tab: AgentPanel) => onSelect?.(tab);
  const observed = useComputerObservation(observation);
  const operations = shared ? observed : (conversation ?? observed);
  const terminalAvailable = operations?.toolCalls.some((tool) => tool.kind === 'execute') === true;
  useEffect(() => {
    if (panel === 'activity' && !terminalAvailable) onSelect?.('browser');
  }, [panel, terminalAvailable, onSelect]);
  const lastScreen = [...(operations?.toolCalls ?? [])]
    .reverse()
    .map(screenOperation)
    .find(Boolean);
  const running =
    detail?.status !== 'waiting_approval' &&
    (operations ? operations.activeRunId !== undefined : detail?.status === 'working');
  // Shared file access is independently renewed and never inferred from a visible toolbar.
  const access = useAgentAccess(
    client.resourceViews,
    shared && (visited || computerOpen || observation) ? agentId : undefined,
  );
  const authorized = access.state.status === 'ready' ? access.state.value.access : undefined;
  const reloadAccess = access.reload;
  const accessChanged = useCallback(() => {
    reloadAccess();
    onAccessChanged();
  }, [reloadAccess, onAccessChanged]);
  const mayRead =
    (!shared || authorized?.capabilities.includes('view_files') === true) &&
    (!client.files?.environment ||
      (environment.state.status === 'ready' &&
        (!features || features.files.state === 'available')));
  const mayRetain =
    (!shared ||
      authorized?.capabilities.includes('view_files') === true ||
      access.state.status === 'loading' ||
      (access.state.status === 'error' && access.state.transient)) &&
    (!client.files?.environment ||
      environment.state.status === 'loading' ||
      (environment.state.status === 'error' && environment.state.transient) ||
      (environment.state.status === 'ready' && features?.files.state !== 'forbidden'));
  if (filesOpen && mayRead && !visited)
    setVisitedScope({ client, agentId, subject, generation: (visitedScope?.generation ?? 0) + 1 });
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
        ref={computerRef}
        aria-label="Agent computer"
        id="agent-files-panel"
        className="agent-computer"
        hidden={!computerOpen}
        inert={!computerOpen || undefined}
      >
        <header className="agent-computer__header">
          <h2 className="sr-only">Agent computer</h2>
          <div className="agent-computer__tabs" role="tablist" aria-label="Agent computer views">
            {(
              ['browser', ...(terminalAvailable ? (['activity'] as const) : []), 'files'] as const
            ).map((tab, index, tabs) => (
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
                        ? tabs.length - 1
                        : (index + (event.key === 'ArrowRight' ? 1 : tabs.length - 1)) %
                          tabs.length;
                  select(tabs[next]);
                  document.getElementById(`computer-tab-${tabs[next]}`)?.focus();
                }}
              >
                {tab === 'files' ? 'Files' : tab === 'activity' ? 'Terminal' : 'Screen'}
              </button>
            ))}
          </div>
          <div className="agent-computer__actions">
            {panel === 'browser' ? (
              <>
                <Button
                  size="sm"
                  icon={<IconRefreshOutline14 />}
                  aria-label="Reconnect"
                  title="Reconnect"
                  onClick={() => {
                    reconnect((revision) => revision + 1);
                    environment.reload();
                  }}
                />
                <Button
                  size="sm"
                  icon={<IconFullscreenOutline16 />}
                  aria-label={fullScreen ? 'Exit full screen' : 'Full screen'}
                  title={fullScreen ? 'Exit full screen' : 'Full screen'}
                  onClick={() => {
                    const request = fullScreen
                      ? document.exitFullscreen?.()
                      : computerRef.current?.requestFullscreen?.();
                    if (!request) setScreenError('Full screen is unavailable in this browser.');
                    else
                      void request.catch(() => setScreenError('Full screen could not be opened.'));
                  }}
                />
              </>
            ) : null}
            <Button
              size="sm"
              icon={<IconCloseOutline16 />}
              aria-label="Close computer"
              title="Close computer"
              onClick={() => {
                if (document.fullscreenElement === computerRef.current)
                  void document.exitFullscreen?.();
                onClose();
              }}
            />
          </div>
        </header>
        {screenError ? <p role="status">{screenError}</p> : null}
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
          {visited && mayRetain && client.files ? (
            <FileExplorer
              initialDirectory={homeFilePath(
                conversation?.runtimeMetadata?.workingDirectory ??
                  detail?.runtimeMetadata?.workingDirectory ??
                  detail?.worktree.path ??
                  '',
              )}
              changes={changedFiles(
                operations?.toolCalls,
                conversation?.runtimeMetadata?.workingDirectory ??
                  detail?.runtimeMetadata?.workingDirectory ??
                  detail?.worktree.path,
              )}
              key={`${agentId}:${shared}:${subject}:${visitedScope?.generation}`}
              panel
              embedded
              active={mayRead}
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
          ) : client.files?.environment && environment.state.status !== 'ready' ? (
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
          ) : (
            <OperationOutput
              toolCalls={operations?.toolCalls}
              activeRunId={running ? operations?.activeRunId : undefined}
              active={panel === 'activity'}
              observation={observation}
              workingDirectory={
                conversation?.runtimeMetadata?.workingDirectory ??
                detail?.runtimeMetadata?.workingDirectory ??
                detail?.worktree.path
              }
            />
          )}
        </div>
        <div
          id="computer-page-browser"
          role="tabpanel"
          aria-labelledby="computer-tab-browser"
          className="agent-computer__page agent-computer__screen"
          hidden={panel !== 'browser'}
        >
          <>
            {machineAvailable ? (
              panel === 'browser' ? (
                <DesktopPanel
                  key={`${agentId}:${connectionRevision}`}
                  agentId={agentId}
                  application={lastScreen?.application ?? 'Desktop'}
                  runtimeReady={desktopRuntimeReady(detail)}
                  running={running}
                  preparation={{ context: client, owner: subject }}
                  onClose={onClose}
                  embedded
                />
              ) : null
            ) : features?.browser.state === 'available' &&
              features.runtime_incarnation &&
              client.browser &&
              (!shared || authorized?.capabilities.includes('view_history') === true) ? (
              <BrowserPanel
                key={`${agentId}:${features.runtime_incarnation}:${features.policy_revision}:${observation?.scope.subject}:${connectionRevision}`}
                client={client.browser}
                controlAccountId={subject}
                controlClient={
                  !shared && features.control.state === 'available' && features.control.writable
                    ? client.browserControl
                    : undefined
                }
                agentId={agentId}
                incarnation={features.runtime_incarnation}
                active={panel === 'browser'}
                running={running}
                controlAdmission={policy.browser_takeover}
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
          </>

          {!shared &&
          policy.desktop_enable &&
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
