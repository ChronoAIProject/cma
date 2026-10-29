import type { ReactNode } from 'react';
import { Button } from '../ui/Button';
import { DesktopPanel } from './DesktopPanel';

/** Keep the conversation at the same tree position when the viewer is toggled. */
export function DesktopWorkspace({
  agentId,
  open,
  running,
  onClose,
  children,
}: {
  readonly agentId: string;
  readonly open: boolean;
  readonly running: boolean;
  readonly onClose: () => void;
  readonly children: ReactNode;
}) {
  return (
    <div className="agent-workspace">
      <div className="agent-workspace__panes" data-desktop={open || undefined}>
        {children}
        {open ? (
          <DesktopPanel key={agentId} agentId={agentId} running={running} onClose={onClose} />
        ) : null}
      </div>
    </div>
  );
}

export function DesktopToggle({
  open,
  onToggle,
}: {
  readonly open: boolean;
  readonly onToggle: () => void;
}) {
  return (
    <div className="agent-surface__desktop-action">
      <Button size="sm" aria-expanded={open} aria-controls="machine-desktop" onClick={onToggle}>
        {open ? 'Hide machine desktop' : 'Open machine desktop'}
      </Button>
    </div>
  );
}
