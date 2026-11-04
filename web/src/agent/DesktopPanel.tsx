import { useEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import { StateDot } from '../ui/StateDot';
import { DesktopFeedback, useDesktopNotice } from './DesktopFeedback';
import { useDesktopConnection } from './useDesktopConnection';
import type { DesktopPreparationScope } from './desktopPreparation';
import './DesktopPanel.css';

export function DesktopPanel({
  agentId,
  runtimeReady,
  running,
  onClose,
  preparation,
  embedded = false,
}: {
  readonly agentId: string;
  readonly runtimeReady: boolean;
  readonly running: boolean;
  readonly onClose: () => void;
  readonly preparation?: DesktopPreparationScope;
  readonly embedded?: boolean;
}) {
  const panel = useRef<HTMLElement>(null);
  const [fullScreen, setFullScreen] = useState(false);
  const [fullScreenError, setFullScreenError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  const [control, setControl] = useState(false);
  const { screen, phase, error, focus } = useDesktopConnection(
    agentId,
    attempt,
    control && !running,
    runtimeReady,
    preparation,
  );
  const ready = phase === 'ready';
  const [previousGate, setPreviousGate] = useState({ running, ready });
  if (running !== previousGate.running || ready !== previousGate.ready) {
    setPreviousGate({ running, ready });
    // A completed turn or reconnected stream never silently restores input.
    if (running || !ready) setControl(false);
  }
  const controller = running ? 'agent' : control ? 'user' : 'view';
  const notice = useDesktopNotice(phase, controller);
  useEffect(() => {
    const changed = () => setFullScreen(document.fullscreenElement === panel.current);
    document.addEventListener('fullscreenchange', changed);
    return () => document.removeEventListener('fullscreenchange', changed);
  }, []);
  return (
    <section
      ref={panel}
      id="machine-desktop"
      className="agent-desktop"
      aria-label="Machine desktop"
      data-controller={ready ? controller : undefined}
      data-embedded={embedded || undefined}
    >
      <header className="agent-desktop__header">
        <div>
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
                  ? 'Unavailable'
                  : phase === 'waiting'
                    ? 'Waiting'
                    : 'Connecting'}
            </span>
          </div>
          <p role="status">
            {phase === 'waiting'
              ? 'Waiting for your machine to be ready…'
              : phase === 'connecting'
                ? 'Connecting to your machine…'
                : phase === 'recovering'
                  ? 'Restoring the desktop display…'
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
          {fullScreenError ? <p role="status">{fullScreenError}</p> : null}
        </div>
        <div className="agent-desktop__actions">
          <Button
            size="sm"
            disabled={running || !ready}
            onClick={() => {
              const next = !control;
              setControl(next);
              if (next) focus();
            }}
          >
            {control && !running
              ? 'Stop controlling'
              : embedded
                ? 'Take control'
                : 'Control desktop'}
          </Button>
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
      <div className="agent-desktop__viewport" aria-busy={!ready && phase !== 'error'}>
        <div
          ref={screen}
          className="agent-desktop__frame"
          data-ready={ready || undefined}
          aria-label="Linux desktop screen"
        />
        <DesktopFeedback phase={phase} error={error} notice={notice} />
      </div>
    </section>
  );
}
