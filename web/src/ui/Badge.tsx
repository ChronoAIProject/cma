import type { ReactNode } from 'react';
import type { AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus } from '../api/types';
import { statusLabel, statusTone, type StatusTone } from '../vocabulary/status';
import { StateDot, type StateDotState } from './StateDot';
import './Badge.css';

const DOT: Readonly<Record<StatusTone, StateDotState | undefined>> = {
  active: 'ongoing',
  neutral: 'neutral',
  warning: 'warning',
  error: 'error',
  muted: undefined,
};

/** The dot for a tone, or none for a status that has ended or is held. */
export function toneDot(tone: StatusTone, size = 8) {
  const state = DOT[tone];
  return state === undefined ? null : <StateDot state={state} size={size} />;
}

/**
 * A status as a word with its dot — `docs/Web_Style.md` §12.1.
 *
 * The word is the signal; the dot only reinforces it, so a state reads the
 * same to somebody who cannot tell the colours apart. Free text takes an
 * explicit tone; a resource status takes its word and tone from the one
 * vocabulary every surface shares.
 */
export function Badge({
  tone,
  children,
  live = false,
}: {
  readonly tone: StatusTone;
  readonly children: ReactNode;
  /** Announce changes politely, for a status that moves while somebody reads. */
  readonly live?: boolean;
}) {
  return (
    <span className="badge" data-tone={tone} role={live ? 'status' : undefined}>
      {toneDot(tone)}
      <span className="badge__word">{children}</span>
    </span>
  );
}

export function StatusBadge({
  status,
  live = false,
}: {
  readonly status: AgentStatus | SandboxStatus | WorkspaceStatus | WorktreeStatus;
  readonly live?: boolean;
}) {
  return (
    <Badge tone={statusTone(status)} live={live}>
      {statusLabel(status)}
    </Badge>
  );
}
