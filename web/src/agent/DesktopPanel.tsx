import { useEffect, useRef, useState } from 'react';
import type RFB from '@novnc/novnc';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import './DesktopPanel.css';

export function DesktopPanel({
  agentId,
  running,
  onClose,
}: {
  readonly agentId: string;
  readonly running: boolean;
  readonly onClose: () => void;
}) {
  const screen = useRef<HTMLDivElement>(null);
  const connection = useRef<RFB>(undefined);
  const [status, setStatus] = useState('Connecting…');
  const [attempt, setAttempt] = useState(0);
  const [control, setControl] = useState(false);
  useEffect(() => {
    let disposed = false;
    let rfb: RFB | undefined;
    void import('@novnc/novnc')
      .then(({ default: Client }) => {
        if (disposed || !screen.current) return;
        const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
        rfb = new Client(
          screen.current,
          `${scheme}://${location.host}/api/v1/agents/${encodeURIComponent(agentId)}/desktop/ws`,
        );
        rfb.scaleViewport = true;
        rfb.resizeSession = true;
        rfb.viewOnly = true;
        connection.current = rfb;
        rfb.addEventListener('connect', () => {
          if (!disposed) setStatus('Connected');
        });
        rfb.addEventListener('disconnect', () => {
          if (!disposed)
            setStatus('Disconnected. Close any other desktop connection and reconnect.');
        });
        rfb.addEventListener('securityfailure', () => {
          if (!disposed) setStatus('Desktop connection refused.');
        });
      })
      .catch(() => {
        if (!disposed) setStatus('Could not load the desktop viewer.');
      });
    return () => {
      disposed = true;
      connection.current = undefined;
      rfb?.disconnect();
    };
  }, [agentId, attempt]);
  useEffect(() => {
    if (connection.current) connection.current.viewOnly = !control || running;
  }, [control, running]);
  return (
    <section className="agent-desktop" aria-label="Machine desktop">
      <header className="agent-desktop__header">
        <div>
          <h2>Machine desktop</h2>
          <p role="status">
            {status}
            {running ? ' · Agent is working; desktop input is paused.' : ''}
          </p>
        </div>
        <div className="agent-desktop__actions">
          <Button
            size="sm"
            disabled={running || status !== 'Connected'}
            onClick={() => setControl(!control)}
          >
            {control && !running ? 'Stop controlling' : 'Control desktop'}
          </Button>
          <Button
            size="sm"
            onClick={() => {
              setControl(false);
              setStatus('Connecting…');
              setAttempt(attempt + 1);
            }}
          >
            Reconnect
          </Button>
          <Button
            size="sm"
            onClick={() => {
              void screen.current
                ?.requestFullscreen?.()
                .catch(() => setStatus('Full screen is unavailable in this browser.'));
            }}
          >
            Full screen
          </Button>
          <Button
            size="sm"
            icon={<IconCloseOutline16 />}
            aria-label="Close machine desktop"
            onClick={onClose}
          />
        </div>
      </header>
      <div ref={screen} className="agent-desktop__frame" aria-label="Linux desktop screen" />
    </section>
  );
}
