import type { SandboxResourceAvailability } from './sandboxInput';
import { ComputerScreen } from './environment/ComputerScreen';
import { ComputerLifecycleNotice } from './environment/ComputerLifecycleNotice';
import { screenApplication } from './screenOperations';
import { changedFiles, homeFilePath } from './fileChanges';
import { agentWorkingDirectory } from './files/filePath';
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
import {
  EnvironmentAvailability,
  FileAvailability,
  FeatureAvailability,
  useAgentEnvironment,
} from './environment/availability';
import {
  useComputerObservation,
  useConversationReturn,
  type AgentComputerObservation,
} from './environment/observation';
import {
  useComputerFullscreen,
  type ComputerFullscreenFallback,
} from './environment/useComputerFullscreen';
import { agentInitialized } from './modelReadiness';

export type AgentPanel = 'details' | 'files' | 'activity' | 'browser' | undefined;

/** Sibling of the conversation: resizing or switching this panel never remounts a stream. */
export function AgentSidePanel({
  agentId,
  shared,
  detail,
  panel,
  onClose,
  onAccessChanged,
  beginFileDownload,
  onOpenSandbox,
  pending,
  historyLoading,
  controlReady,
  publicationScope,
  conversation,
  onSelect,
  observation,
  viewerId,
  onStopForControl,
  resourceAvailability,
  canManageSandbox = false,
  fullscreenFallback,
}: {
  readonly canManageSandbox?: boolean;
  readonly onStopForControl?: () => Promise<void>;
  readonly resourceAvailability?: SandboxResourceAvailability;
  readonly agentId: string;
  readonly shared: boolean;
  readonly detail?: AgentDetail;
  readonly panel: AgentPanel;
  readonly onClose: () => void;
  readonly onAccessChanged: (downloadMessage?: string) => void;
  readonly beginFileDownload?: () => (message: string) => void;
  readonly onOpenSandbox?: () => void;
  readonly pending?: AgentCommand;
  readonly historyLoading?: boolean;
  readonly controlReady?: boolean;
  readonly publicationScope: ArtifactPublicationScope;
  readonly conversation?: ConversationState;
  readonly onSelect?: (panel: AgentPanel, expected?: AgentPanel) => void;
  readonly observation?: AgentComputerObservation;
  readonly viewerId?: string;
  readonly fullscreenFallback?: ComputerFullscreenFallback;
}) {
  const client = useCmaClient();
  const filesOpen = panel === 'files';
  const computerOpen = panel !== undefined && panel !== 'details';
  const computerRef = useRef<HTMLElement>(null);
  const [connectionRevision, reconnect] = useState(0);
  const fullscreen = useComputerFullscreen(computerRef, fullscreenFallback);
  const { expanded, fullScreen, screenError, exitFullScreen } = fullscreen;
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
  const lifecycleHeld = resourceAvailability !== undefined && resourceAvailability.state !== 'live';
  const environment = useAgentEnvironment(
    client.files,
    agentId,
    computerOpen && !lifecycleHeld,
    subject,
    resourceAvailability?.state === 'live' ? resourceAvailability.runtimeVersion : undefined,
  );
  const lifecycleNotice = lifecycleHeld ? (
    <ComputerLifecycleNotice
      availability={resourceAvailability}
      onOpenSandbox={!shared && canManageSandbox ? onOpenSandbox : undefined}
    />
  ) : null;
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
  const [errorPanel, setErrorPanel] = useState(panel);
  if (errorPanel !== panel) {
    setErrorPanel(panel);
    fullscreen.clearScreenError();
  }
  const select = (tab: AgentPanel) => onSelect?.(tab);
  const currentObservation =
    observation?.scope.client === client &&
    observation.scope.subject === subject &&
    observation.scope.agentId === agentId
      ? observation
      : undefined;
  const observed = useComputerObservation(currentObservation);
  const onReturnToConversation = useConversationReturn(currentObservation);
  const operations = shared ? observed : (conversation ?? observed);
  const workingDirectory = agentWorkingDirectory(
    conversation?.runtimeMetadata?.workingDirectory,
    detail?.id === agentId ? detail : undefined,
  );
  const terminalAvailable = operations?.toolCalls.some((tool) => tool.kind === 'execute') === true;
  const historyLoaded =
    !historyLoading && (conversation !== undefined || observed?.historyLoaded === true);
  useEffect(() => {
    if (historyLoaded && panel === 'activity' && !terminalAvailable)
      onSelect?.('browser', 'activity');
  }, [historyLoaded, panel, terminalAvailable, onSelect]);
  const lastScreen = [...(operations?.toolCalls ?? [])]
    .reverse()
    .map(screenApplication)
    .find(Boolean);
  const controlsReady = controlReady ?? historyLoaded;
  const running =
    operations?.activeRunId !== undefined || (!controlsReady && detail?.status === 'working');
  // Shared file access is independently renewed and never inferred from a visible toolbar.
  const access = useAgentAccess(
    client.resourceViews,
    shared && (visited || computerOpen || observation) ? agentId : undefined,
    subject,
  );
  const authorized = access.state.status === 'ready' ? access.state.value.access : undefined;
  const reloadAccess = access.reload;
  const accessChanged = useCallback(
    (downloadMessage?: string) => {
      reloadAccess();
      onAccessChanged(downloadMessage);
    },
    [reloadAccess, onAccessChanged],
  );
  const mayRead =
    computerOpen &&
    !lifecycleHeld &&
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
  const fileAvailability = lifecycleNotice ?? (
    <FileAvailability
      shared={shared}
      access={access}
      environment={environment}
      reported={!!client.files?.environment}
      onReturnToConversation={onReturnToConversation}
    />
  );
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
            workingDirectory={workingDirectory}
            open={panel === 'details'}
            onClose={onClose}
            onAccessChanged={onAccessChanged}
            publicationScope={publicationScope}
          />
        ) : (
          <AgentDetails
            viewerId={subject}
            availability={resourceAvailability}
            detail={detail}
            workingDirectory={workingDirectory}
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
        data-expanded={expanded || undefined}
        hidden={!computerOpen}
        inert={!computerOpen || undefined}
        onKeyDownCapture={(event) => {
          if (event.key !== 'Escape' || document.fullscreenElement !== computerRef.current) return;
          event.preventDefault();
          event.stopPropagation();
          exitFullScreen();
        }}
      >
        <header className="agent-computer__header">
          <h2 className="sr-only">Agent computer</h2>
          <div className="agent-computer__tabs" role="tablist" aria-label="Agent computer views">
            {(
              [
                'browser',
                ...(terminalAvailable || (panel === 'activity' && !historyLoaded)
                  ? (['activity'] as const)
                  : []),
                'files',
              ] as const
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
                {tab === 'files' ? 'Files' : tab === 'activity' ? 'Operations' : 'Screen'}
              </button>
            ))}
          </div>
          <div className="agent-computer__actions">
            {panel === 'browser' &&
            !lifecycleHeld &&
            ((!shared && detail?.desktop === true) ||
              !features ||
              ['available', 'temporarily_unavailable'].includes(features.browser.state)) ? (
              <Button
                size="sm"
                icon={<IconRefreshOutline14 />}
                aria-label="Reconnect"
                title="Reconnect"
                onClick={() => {
                  fullscreen.clearScreenError();
                  reconnect((revision) => revision + 1);
                  environment.reload();
                }}
              />
            ) : null}
            {panel === 'browser' || expanded || fullScreen ? (
              <Button
                size="sm"
                icon={<IconFullscreenOutline16 />}
                aria-label={fullscreen.screenSizeLabel}
                title={fullscreen.screenSizeLabel}
                onClick={fullscreen.toggleScreenSize}
              />
            ) : null}
            <Button
              size="sm"
              icon={<IconCloseOutline16 />}
              aria-label="Close computer"
              title="Close computer"
              onClick={() => {
                if (document.fullscreenElement === computerRef.current) exitFullScreen();
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
          {mayRead && client.files ? (
            <details className="agent-computer__scope">
              <summary>About file access</summary>
              <p>
                Browse authorized files under /home/. Full paths identify their location in the
                Sandbox. Private runtime directories are excluded.
              </p>
            </details>
          ) : null}
          {visited && mayRetain && !mayRead ? fileAvailability : null}
          {visited && mayRetain && client.files ? (
            <FileExplorer
              initialDirectory={homeFilePath(workingDirectory ?? '')}
              navigationMemory={currentObservation?.fileNavigation}
              changes={changedFiles(operations?.toolCalls, workingDirectory)}
              key={`${agentId}:${shared}:${subject}:${currentObservation?.scope.authEpoch}:${visitedScope?.generation}`}
              panel
              embedded
              active={mayRead}
              client={client.files}
              agentId={agentId}
              readOnly={!mayWrite}
              onClose={onClose}
              onAccessChanged={accessChanged}
              beginDownload={shared && computerOpen ? beginFileDownload : undefined}
            />
          ) : filesOpen ? (
            <aside className="agent-files-panel" aria-label="Agent files">
              <header className="agent-files-panel__header">
                <h2>Files</h2>
                <Button size="sm" onClick={onClose}>
                  Close files
                </Button>
              </header>
              <div className="agent-files-panel__body">{fileAvailability}</div>
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
          ) : !lifecycleHeld && features && features.operation_output.state !== 'available' ? (
            <FeatureAvailability name="Operation output" feature={features.operation_output} />
          ) : !lifecycleHeld &&
            client.files?.environment &&
            environment.state.status !== 'ready' ? (
            <EnvironmentAvailability subject="operation" environment={environment} />
          ) : (
            <OperationOutput
              toolCalls={historyLoaded || terminalAvailable ? operations?.toolCalls : undefined}
              activeRunId={running ? operations?.activeRunId : undefined}
              active={panel === 'activity'}
              observation={observation}
              workingDirectory={workingDirectory}
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
          {lifecycleNotice}
          <ComputerScreen
            onReturnToConversation={onReturnToConversation}
            agentId={agentId}
            detail={detail}
            shared={shared}
            active={panel === 'browser'}
            environment={environment}
            lifecycleHeld={lifecycleHeld}
            runtimeVersion={
              resourceAvailability?.state === 'live'
                ? resourceAvailability.runtimeVersion
                : undefined
            }
            subject={subject}
            lastScreen={lastScreen}
            connectionRevision={connectionRevision}
            running={running}
            runId={operations?.activeRunId}
            controlsReady={controlsReady}
            mayViewHistory={!shared || authorized?.capabilities.includes('view_history') === true}
            onStopForControl={onStopForControl}
            onClose={onClose}
          />
        </div>
      </aside>
    </>
  );
}
