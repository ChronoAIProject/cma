import type { AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus } from '../api/types';
import type { CharacterPose } from './poses';
import type { CharacterTone } from './stage';

export type ResourceStatus = AgentStatus | SandboxStatus | WorkspaceStatus | WorktreeStatus;

/** How the character presents one recorded status. */
export interface CharacterPresence {
  readonly pose: CharacterPose;
  readonly tone: CharacterTone;
  /**
   * Whether the pose moves in a dense list. Only states that show work or ask for a person
   * move there; settled and ended states draw their still moment, so the rows that do move
   * are worth looking at (Web_Style.md §28).
   */
  readonly animate: boolean;
}

const quiet = (pose: CharacterPose, tone: CharacterTone = 'brand'): CharacterPresence => ({
  pose,
  tone,
  animate: false,
});
const live = (pose: CharacterPose, tone: CharacterTone = 'brand'): CharacterPresence => ({
  pose,
  tone,
  animate: true,
});

const PRESENCE = {
  provisioning: live('preparing'),
  installing: live('preparing'),
  preparing: live('preparing'),
  creating: live('preparing'),
  starting: live('preparing'),
  credential_rotating: live('preparing'),
  rotating: live('preparing'),
  active: quiet('idle'),
  ready: quiet('idle'),
  working: live('thinking'),
  waiting_approval: live('approval', 'warning'),
  pausing: live('paused'),
  paused: quiet('paused'),
  reviving: live('resuming'),
  failed: quiet('failed', 'error'),
  stopped: quiet('paused'),
  retiring: quiet('paused'),
  retired: quiet('paused'),
  removing: quiet('paused'),
  removed: quiet('paused'),
  deleted: quiet('paused'),
} as const satisfies Record<ResourceStatus, CharacterPresence>;

/** The character for a recorded resource status, after any parent lifecycle is applied. */
export function characterForStatus(status: ResourceStatus): CharacterPresence {
  return PRESENCE[status];
}

/** What the live turn is doing right now, as the pose that shows it. */
export function characterForTurn({
  pendingTools,
  replying,
}: {
  readonly pendingTools: number;
  readonly replying: boolean;
}): CharacterPose {
  if (pendingTools > 0) return 'working';
  return replying ? 'replying' : 'thinking';
}
