import { ComputerRecoveryPanel } from './environment/ComputerRecoveryPanel';
import { Modal } from '../ui/Modal';
import { useEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import { StateDot } from '../ui/StateDot';
import { DesktopFeedback, useDesktopNotice } from './DesktopFeedback';
import { DesktopLocalTime } from './DesktopLocalTime';
import { useDesktopConnection } from './useDesktopConnection';
import type { DesktopPreparationScope } from './desktopPreparation';
import './DesktopPanel.css';

export function DesktopPanel({
  agentId,
  runtimeReady,
  runtimeVersion,
  running,
  onClose,
  onReturnToConversation,
  preparation,
  embedded = false,
  hidden = false,
  application = 'Desktop',
  onStop,
  runId,
  controlUnavailable = false,
  reconnectRevision = 0,
}: {
  readonly reconnectRevision?: number;
  readonly onStop?: () => Promise<void>;
  readonly runId?: string;
  readonly controlUnavailable?: boolean;
  readonly agentId: string;
  readonly runtimeReady: boolean;
  readonly runtimeVersion?: string;
  readonly running: boolean;
  readonly onClose: () => void;
  readonly onReturnToConversation?: () => void;
  readonly preparation?: DesktopPreparationScope;
  readonly embedded?: boolean;
  readonly hidden?: boolean;
  readonly application?: string;
}) {
  const panel = useRef<HTMLElement>(null);
  const [fullScreen, setFullScreen] = useState(false);
  const [fullScreenError, setFullScreenError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  const [control, setControl] = useState(false);
  const [takeover, setTakeover] = useState<{
    run?: string;
    phase: 'confirm' | 'stopping' | 'waiting' | 'failed';
    token?: object;
    error?: string;
  }>();
  const [takeoverError, setTakeoverError] = useState<string>();
  const { screen, phase, error, stop } = useDesktopConnection(
    agentId,
    attempt + reconnectRevision,
    control && !running && !controlUnavailable,
    runtimeReady,
    preparation,
    runtimeVersion,
  );
  const ready = phase === 'ready';
  const [previousGate, setPreviousGate] = useState({ running, ready, controlUnavailable });
  if (
    running !== previousGate.running ||
    ready !== previousGate.ready ||
    controlUnavailable !== previousGate.controlUnavailable
  ) {
    if ((running && !previousGate.running) || !ready || controlUnavailable) {
      setControl(false);
      if (!ready || controlUnavailable || (running && !previousGate.running)) {
        setTakeover(undefined);
      }
    }
    setPreviousGate({ running, ready, controlUnavailable });
    // A completed turn or reconnected stream never silently restores input.
  }
  if (takeover?.phase === 'waiting' && !running && ready && !controlUnavailable) {
    setControl(true);
    setTakeover(undefined);
  }
  if (takeover && running && takeover.run !== runId) {
    setTakeover(undefined);
    setTakeoverError('A new Agent turn started. Take control again when you are ready.');
  }
  const confirmTakeover = async () => {
    if (!onStop || takeover?.phase !== 'confirm') return;
    const token = {};
    const requestedRun = runId;
    setTakeoverError(undefined);
    setTakeover({ run: requestedRun, phase: 'stopping', token });
    try {
      await onStop();
      setTakeover((current) =>
        current?.token === token ? { ...current, phase: 'waiting' } : current,
      );
    } catch (cause) {
      const error =
        cause instanceof Error
          ? cause.message
          : 'The Agent could not be stopped. Control was not granted.';
      setTakeover((current) =>
        current?.token === token ? { ...current, phase: 'failed', error } : current,
      );
    }
  };
  const controller = control ? 'user' : running ? 'agent' : 'view';
  const notice = useDesktopNotice(phase, controller);
  useEffect(() => {
    const changed = () => setFullScreen(document.fullscreenElement === panel.current);
    document.addEventListener('fullscreenchange', changed);
    return () => document.removeEventListener('fullscreenchange', changed);
  }, []);
  return (
    <section
      ref={panel}
      hidden={hidden}
      inert={hidden || undefined}
      id="machine-desktop"
      className="agent-desktop"
      aria-label="Machine desktop"
      data-controller={ready ? controller : undefined}
      data-embedded={embedded || undefined}
    >
      <header className="agent-desktop__header">
        <div>
          {embedded && ready ? (
            <p className="workspace-control-status" role="status">
              <span
                className={`workspace-control-dot${running && !control ? ' workspace-control-dot--running' : ''}`}
                aria-hidden="true"
              />
              {control
                ? 'You are in control'
                : running
                  ? `Agent is in control · Working in ${application}`
                  : 'Agent paused · Desktop is idle'}
            </p>
          ) : (
            <>
              <div className="agent-desktop__title">
                <h2>{embedded ? 'Shared desktop' : 'Machine desktop'}</h2>
                <span className="agent-desktop__control-label">
                  <StateDot
                    state={phase === 'error' ? 'error' : !ready || running ? 'ongoing' : 'neutral'}
                  />
                  {ready
                    ? running
                      ? 'Agent working'
                      : control
                        ? 'You control'
                        : 'Watching'
                    : phase === 'error'
                      ? stop === 'exhausted'
                        ? 'Disconnected'
                        : 'Unavailable'
                      : phase === 'waiting'
                        ? 'Waiting'
                        : phase === 'recovering'
                          ? 'Reconnecting…'
                          : 'Connecting'}
                </span>
              </div>
              <p role="status">
                {phase === 'waiting'
                  ? 'Waiting for your machine to be ready…'
                  : phase === 'connecting'
                    ? 'Connecting to your machine…'
                    : phase === 'recovering'
                      ? `Reconnecting…${error ? ` ${error}` : ''}`
                      : phase === 'loading'
                        ? 'Loading the desktop screen…'
                        : phase === 'error'
                          ? error
                          : 'Connected'}
                {ready &&
                  (running
                    ? ' · Agent is working. Watch its actions here.'
                    : control
                      ? ' · You are controlling the desktop.'
                      : ' · Ask the Agent to use this desktop, or take control.')}
              </p>
            </>
          )}
          {!hidden ? <DesktopLocalTime /> : null}
          {fullScreenError ? <p role="status">{fullScreenError}</p> : null}
        </div>
        <div className="agent-desktop__actions">
          <Button
            size="sm"
            variant="primary"
            placement="workspace-take-control"
            disabled={
              !ready ||
              controlUnavailable ||
              takeover?.phase === 'stopping' ||
              takeover?.phase === 'waiting'
            }
            title={
              controlUnavailable ? 'Checking Agent activity before taking control.' : undefined
            }
            onClick={() => {
              setTakeoverError(undefined);
              if (!control && running) {
                setTakeover({ run: runId, phase: 'confirm' });
                return;
              }
              setTakeover(undefined);
              const next = !control;
              setControl(next);
            }}
          >
            {control ? 'Return to Agent' : embedded ? 'Take control' : 'Control desktop'}
          </Button>
          {!embedded ? (
            <>
              <Button
                size="sm"
                disabled={!runtimeReady}
                onClick={() => {
                  setControl(false);
                  setAttempt(attempt + 1);
                }}
              >
                Reconnect
              </Button>
              <Button
                size="sm"
                onClick={() => {
                  setFullScreenError(undefined);
                  const request = fullScreen
                    ? document.exitFullscreen?.()
                    : panel.current?.requestFullscreen?.();
                  void request?.catch(() =>
                    setFullScreenError('Full screen is unavailable in this browser.'),
                  );
                }}
              >
                {fullScreen ? 'Exit full screen' : 'Full screen'}
              </Button>
            </>
          ) : null}
          {!embedded ? (
            <Button
              size="sm"
              icon={<IconCloseOutline16 />}
              aria-label="Close machine desktop"
              onClick={onClose}
            />
          ) : null}
        </div>
      </header>
      {takeoverError || takeover?.error ? (
        <p role="alert">{takeoverError ?? takeover?.error}</p>
      ) : null}
      {takeover?.phase === 'waiting' ? <p role="status">Waiting for the Agent to stop…</p> : null}
      <Modal
        open={takeover?.phase === 'confirm' || takeover?.phase === 'stopping'}
        title="Take control"
        description="Taking control stops the Agent's current turn"
        dismissible={takeover?.phase !== 'stopping'}
        onClose={() => {
          setTakeover(undefined);
        }}
      >
        <Button
          disabled={takeover?.phase === 'stopping'}
          onClick={() => {
            setTakeover(undefined);
          }}
        >
          Cancel
        </Button>
        <Button
          variant="primary"
          disabled={!onStop || takeover?.phase === 'stopping'}
          onClick={() => void confirmTakeover()}
        >
          {takeover?.phase === 'stopping' ? 'Stopping Agent…' : 'Stop Agent and take control'}
        </Button>
        {!onStop ? <p role="status">Stopping this Agent is unavailable with this access.</p> : null}
      </Modal>
      <div
        className="agent-desktop__viewport"
        data-recovery={phase === 'error' || undefined}
        aria-busy={!ready && phase !== 'error'}
      >
        <div
          ref={screen}
          className="agent-desktop__frame"
          data-ready={ready || undefined}
          aria-label="Linux desktop screen"
        />
        {phase === 'error' ? (
          <ComputerRecoveryPanel
            stop={stop}
            error={error}
            reconnectDisabled={!runtimeReady}
            onReturnToConversation={onReturnToConversation}
            onReconnect={() => {
              setControl(false);
              setAttempt(attempt + 1);
            }}
          />
        ) : (
          <DesktopFeedback phase={phase} error={error} notice={notice} />
        )}
      </div>
    </section>
  );
}
