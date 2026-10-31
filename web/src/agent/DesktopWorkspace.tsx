import type { ReactNode } from 'react';
import type { AgentDetail } from '../api/types';
import { Button } from '../ui/Button';
import { DesktopPanel } from './DesktopPanel';

/** Match the public desktop service's runtime availability gate. */
export function desktopRuntimeReady(detail: Pick<AgentDetail, 'status' | 'sandbox'>) {
  return (
    detail.sandbox.status === 'active' &&
    ['ready', 'working', 'waiting_approval'].includes(detail.status)
  );
}

/** Keep the conversation at the same tree position when the viewer is toggled. */
export function DesktopWorkspace({
  agentId,
  open,
  runtimeReady,
  running,
  onClose,
  children,
}: {
  readonly agentId: string;
  readonly open: boolean;
  readonly runtimeReady: boolean;
  readonly running: boolean;
  readonly onClose: () => void;
  readonly children: ReactNode;
}) {
  return (
    <div className="agent-workspace">
      <div className="agent-workspace__panes" data-desktop={open || undefined}>
        {children}
        {open ? (
          <DesktopPanel
            key={agentId}
            agentId={agentId}
            runtimeReady={runtimeReady}
            running={running}
            onClose={onClose}
          />
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
