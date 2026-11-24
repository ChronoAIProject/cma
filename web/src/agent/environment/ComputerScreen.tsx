import type { AgentDetail } from '../../api/types';
import { useCmaClient } from '../../api/context';
import { detailReadSource } from '../../api/detailReads';
import { useTaskExperience } from '../../taskExperience';
import { Button } from '../../ui/Button';
import { DesktopPanel } from '../DesktopPanel';
import { desktopRuntimeReady } from '../DesktopWorkspace';
import { BrowserPanel, EnableBrowser } from './BrowserPanel';
import {
  EnvironmentAvailability,
  FeatureAvailability,
  type useAgentEnvironment,
} from './availability';

export function ComputerScreen({
  agentId,
  detail,
  shared,
  active,
  environment,
  lifecycleHeld,
  runtimeVersion,
  subject,
  lastScreen,
  connectionRevision,
  running,
  runId,
  controlsReady,
  mayViewHistory,
  onStopForControl,
  onClose,
  onReturnToConversation,
}: {
  readonly onReturnToConversation?: () => void;
  readonly agentId: string;
  readonly detail?: AgentDetail;
  readonly shared: boolean;
  readonly active: boolean;
  readonly environment: ReturnType<typeof useAgentEnvironment>;
  readonly lifecycleHeld: boolean;
  readonly runtimeVersion?: string;
  readonly subject?: string;
  readonly lastScreen?: string;
  readonly connectionRevision: number;
  readonly running: boolean;
  readonly runId?: string;
  readonly controlsReady: boolean;
  readonly mayViewHistory: boolean;
  readonly onStopForControl?: () => Promise<void>;
  readonly onClose: () => void;
}) {
  const client = useCmaClient();
  const policy = useTaskExperience(client, subject);
  const features = environment.state.status === 'ready' ? environment.state.value : undefined;
  const machineAvailable = !shared && detail?.desktop === true;
  const agentBrowserActive =
    lastScreen === 'Browser' &&
    features?.browser.state === 'available' &&
    !!features.runtime_incarnation &&
    !!client.browser;
  if (lifecycleHeld && !machineAvailable) return null;
  return (
    <>
      <>
        {!shared && detail === undefined && features?.browser.state === 'unsupported' ? (
          <p role="status">Checking Agent desktop availability…</p>
        ) : machineAvailable && !agentBrowserActive ? (
          active ? (
            <DesktopPanel
              key={agentId}
              reconnectRevision={connectionRevision}
              agentId={agentId}
              application="Desktop"
              runtimeReady={!lifecycleHeld && desktopRuntimeReady(detail)}
              runtimeVersion={runtimeVersion}
              hidden={lifecycleHeld}
              running={running}
              runId={runId}
              onStop={onStopForControl}
              controlUnavailable={!controlsReady}
              preparation={{ context: detailReadSource(client), owner: subject }}
              onClose={onClose}
              onReturnToConversation={onReturnToConversation}
              embedded
            />
          ) : null
        ) : features?.browser.state === 'available' &&
          features.runtime_incarnation &&
          client.browser &&
          (!shared || mayViewHistory) ? (
          <BrowserPanel
            key={`${agentId}:${features.runtime_incarnation}:${features.policy_revision}:${subject}:${connectionRevision}`}
            client={client.browser}
            controlAccountId={subject}
            controlClient={
              !shared && features.control.state === 'available' && features.control.writable
                ? client.browserControl
                : undefined
            }
            agentId={agentId}
            incarnation={features.runtime_incarnation}
            active={active}
            running={running}
            controlAdmission={policy.browser_takeover}
            onReload={environment.reload}
          />
        ) : features ? (
          <FeatureAvailability
            name="Browser"
            feature={features.browser}
            onRetry={environment.reload}
            policyDisabled={policy.confirmed && !policy.desktop_enable}
            onReturnToConversation={onReturnToConversation}
          />
        ) : !client.files?.environment ? (
          <p role="status">Environment capabilities not reported.</p>
        ) : (
          <EnvironmentAvailability subject="screen" environment={environment} />
        )}
      </>

      {!lifecycleHeld &&
      !shared &&
      !machineAvailable &&
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
      {!lifecycleHeld &&
      machineAvailable &&
      !agentBrowserActive &&
      environment.state.status === 'error' ? (
        <Button size="sm" onClick={environment.reload}>
          Check environment again
        </Button>
      ) : null}
    </>
  );
}
