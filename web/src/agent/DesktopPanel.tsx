import { useRef } from 'react';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import './DesktopPanel.css';

export function DesktopPanel({
  agentId,
  onClose,
}: {
  readonly agentId: string;
  readonly onClose: () => void;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const enterFullscreen = () => {
    const element = frame.current;
    if (element?.requestFullscreen) void element.requestFullscreen();
  };
  return (
    <section className="agent-desktop" aria-label="Machine desktop">
      <header className="agent-desktop__header">
        <div>
          <h2>Machine desktop</h2>
          <p>Click and type in the Linux desktop running beside your Agent.</p>
        </div>
        <div className="agent-desktop__actions">
          <Button size="sm" onClick={enterFullscreen}>
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
      <iframe
        ref={frame}
        title="Machine desktop"
        className="agent-desktop__frame"
        src={`/api/v1/agents/${encodeURIComponent(agentId)}/desktop`}
        allow="fullscreen"
      />
    </section>
  );
}
