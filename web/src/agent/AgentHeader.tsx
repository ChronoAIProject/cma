import type { ReactNode, Ref } from 'react';
import type { AgentStatus } from '../api/types';
import { Button } from '../ui/Button';
import { StateDot, type StateDotState } from '../ui/StateDot';
import { IconInspectOutline12 } from '../ui/icons';
import { statusLabel } from '../vocabulary/status';

function statusDot(status: AgentStatus): StateDotState {
  if (status === 'failed' || status === 'deleted') return 'error';
  if (status === 'paused' || status === 'waiting_approval') return 'warning';
  if (status === 'working' || status === 'starting') return 'ongoing';
  return 'done';
}

/** Presentation shared by personal and authorized organization conversations. */
export function AgentHeader({
  title,
  status,
  lineage,
  headingRef,
  detailsOpen,
  detailsUnavailable = false,
  onDetailsOpenChange,
  children,
}: {
  readonly title: string;
  readonly status: AgentStatus;
  readonly lineage: ReactNode;
  readonly headingRef?: Ref<HTMLHeadingElement>;
  readonly detailsOpen: boolean;
  readonly detailsUnavailable?: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly children?: ReactNode;
}) {
  return (
    <header className="agent-header">
      <nav className="agent-header__lineage" aria-label="Agent location">
        {lineage}
      </nav>
      <div className="agent-header__title">
        <div>
          <h1 ref={headingRef} tabIndex={-1}>
            {title}
          </h1>
          <span className="agent-header__status">
            <StateDot state={statusDot(status)} size={9} />
            {statusLabel(status)}
          </span>
        </div>
        <Button
          size="sm"
          variant="toolbar"
          icon={<IconInspectOutline12 />}
          aria-label={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
          title={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
          aria-pressed={detailsOpen}
          disabled={detailsUnavailable}
          onClick={() => onDetailsOpenChange(!detailsOpen)}
        />
      </div>
      {children}
    </header>
  );
}
