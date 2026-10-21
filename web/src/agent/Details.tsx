import { useEffect, useRef } from 'react';
import type { AgentDetail } from '../api/types';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import type { AgentCommand } from './controller';
import './Details.css';
import { DetailsServiceAccess } from './DetailsServiceAccess';
import { openSandboxTab } from '../sandbox/sandboxTabRoute';
import { SkillsSettings } from './SkillsSettings';

interface AgentDetailsProps {
  readonly detail?: AgentDetail;
  readonly pending?: AgentCommand;
  readonly historyLoading?: boolean;
  readonly open?: boolean;
  readonly onClose?: () => void;
  /** Opens the Agent's Sandbox page; the drawer picks which tab. */
  readonly onOpenSandbox?: () => void;
}

export function AgentDetails({
  detail,
  pending,
  historyLoading = false,
  open = false,
  onClose,
  onOpenSandbox,
}: AgentDetailsProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    heading.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [open]);
  if (detail === undefined) {
    return <aside className="agent-details" aria-label="Agent details" aria-busy="true" />;
  }

  return (
    <aside className="agent-details" aria-label="Agent details">
      <header className="agent-details__header">
        <h1 ref={heading} tabIndex={-1}>
          Details
        </h1>
        {onClose === undefined ? null : (
          <Button
            size="sm"
            icon={<IconCloseOutline16 />}
            aria-label="Close Agent details"
            title="Close Agent details"
            onClick={onClose}
          />
        )}
      </header>
      <div className="agent-details__scroll">
        <div className="agent-details__band">
          <DetailsServiceAccess detail={detail} onOpenSandbox={onOpenSandbox} />
          <SkillsSettings
            key={`skills:${detail.id}`}
            agentId={detail.id}
            active={open}
            busy={detail.status !== 'ready' || pending !== undefined || historyLoading}
          />
          {onOpenSandbox === undefined ? null : (
            <Button onClick={() => openSandboxTab(onOpenSandbox, detail.sandbox.id, 'activity')}>
              Sandbox operation progress
            </Button>
          )}
        </div>
      </div>
    </aside>
  );
}
