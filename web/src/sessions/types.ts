/**
 * Session model, mirroring docs/API_Overview.md.
 *
 * This is a projection of the core Layer 2 model for display purposes. It must
 * not invent state the core does not have.
 */

import type { SessionCapabilities } from '../agui';

/**
 * Lifecycle: provisioning -> ready -> running <-> idle -> terminated.
 *
 * `rotating` sits across the top of that: a running session being moved onto
 * an updated runtime. It is a state of its own because the session is neither
 * usable nor stopped while it happens, and calling it either would be a lie
 * the owner acts on.
 */
export type SessionLifecycle =
  'provisioning' | 'ready' | 'running' | 'rotating' | 'idle' | 'terminated';

/**
 * Why a session stopped, where the control plane says.
 *
 * Only `error` is distinguished here, because it is the only one that changes
 * what the interface should do: a session that failed stays visible so its
 * owner can find out why.
 */
export type StopReason = 'error' | 'other';

/**
 * The two planes a session can be reached on.
 *
 * Exclusive by construction: `capabilities.rs` fixes the granted set at
 * creation and a session holds one or the other. Moving between them is a
 * rotation, not a toggle.
 */
export type SessionPlane = 'terminal' | 'conversation';

/** The kernels the control plane can run. */
export type Kernel = 'codex' | 'claude-code';

/**
 * One service a session's credential may reach.
 *
 * Identifiers always; a name and a description only when the caller's own
 * service catalogue offered them. A catalogue that could not be asked costs
 * the names and not the answer, so a service with no name is still a service
 * this session reaches.
 */
export interface SessionService {
  readonly id: string;
  /** What the owner recognises it by, when the catalogue said. */
  readonly name?: string;
  readonly description?: string;
  /**
   * Whether the platform mandates it.
   *
   * Drawn as fixed rather than as something the owner chose, matching how the
   * picker already treats the entries that cannot be unticked — for the same
   * stated reason: a person deciding what a session may touch should see the
   * whole answer.
   */
  readonly mandated: boolean;
}

export interface Session {
  readonly id: string;
  readonly kernel: Kernel;
  readonly lifecycle: SessionLifecycle;
  /** Epoch ms the session was created, for the tab's age readout. */
  readonly createdAt: number;
  /** Why it stopped, when it has. */
  readonly stopReason?: StopReason;
  /** Which runtime version is serving it. */
  readonly runtimeVersion?: string;
  /** How many times it has been moved onto a newer runtime. */
  readonly rotations?: number;
  /**
   * Epoch ms of the snapshot the current runtime resumed from.
   *
   * The honest part of a rotation: it says how much of the session was carried
   * across, so anything the agent did in the seconds after it can be
   * recognised as missing rather than silently absent.
   */
  readonly rotatedAt?: number;
  /**
   * Who started the session, as they should be shown.
   *
   * Absent when the control plane would not name them: naming somebody is its
   * own grant, and a caller who does not hold it is told nothing rather than
   * handed an identifier. Never fall back to the subject — it is not a name
   * and reads as a rendering fault when drawn as one.
   */
  readonly creator?: SessionCreator;
  /**
   * What this session's credential may reach.
   *
   * `undefined` and `[]` are different facts and must not be drawn alike.
   * Undefined means the control plane did not report it — an older build, or a
   * projection this one does not carry — and the honest answer is that we do
   * not know. Empty means this session reaches nothing, which is a real answer
   * somebody can act on.
   */
  readonly reach?: readonly SessionService[];
  /**
   * Which plane this session is drawn on — `Web_Style.md` §1 and §18.
   *
   * A property of the session, decided when it was created, and not a view and
   * not a preference. The client reads it and draws what it implies; there is
   * nothing here to toggle, because the two planes are exclusive by
   * construction in the core.
   *
   * Absent when the control plane did not say. A session whose plane is not
   * reported falls back to the terminal, which is what every session was
   * before the conversation plane existed — the safe direction, since a
   * terminal drawn for a conversation session shows an empty screen and a
   * conversation drawn for a terminal session shows nothing at all.
   */
  readonly plane?: SessionPlane;
  /**
   * What this session was granted, read once at the boundary.
   *
   * #116: components ask "may this session steer" rather than trying and
   * handling a refusal. Held on the session because it is a fact about the
   * session, decided at creation and never revisited.
   */
  readonly capabilities: SessionCapabilities;
}

/** How the person who started a session is drawn. */
export interface SessionCreator {
  readonly displayName?: string;
  readonly avatarUrl?: string;
}

/** Which semantic token paints a lifecycle state. */
export type StateTone = 'live' | 'wait' | 'fail' | 'ended';

/**
 * `idle` is the state most often misread. A session whose bot pod was
 * reclaimed to save resources is idle, and attaching to it revives it under
 * the same id — so it is a warning tone at worst, never danger.
 */
export function toneFor(lifecycle: SessionLifecycle): StateTone {
  switch (lifecycle) {
    case 'ready':
    case 'running':
      return 'live';
    case 'provisioning':
    case 'rotating':
    case 'idle':
      return 'wait';
    case 'terminated':
      return 'ended';
    default: {
      // Exhaustiveness: a new lifecycle state must be given a tone
      // deliberately rather than silently rendering as unknown.
      const exhaustive: never = lifecycle;
      return exhaustive;
    }
  }
}

/** Short label shown on the tab. Colour is never the only signal. */
export function labelFor(lifecycle: SessionLifecycle): string {
  switch (lifecycle) {
    case 'provisioning':
      return 'starting';
    case 'ready':
      return 'ready';
    case 'running':
      return 'running';
    case 'rotating':
      return 'updating';
    case 'idle':
      return 'idle';
    case 'terminated':
      return 'ended';
    default: {
      const exhaustive: never = lifecycle;
      return exhaustive;
    }
  }
}
