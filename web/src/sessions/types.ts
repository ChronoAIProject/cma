/**
 * Session model, mirroring docs/API_Overview.md.
 *
 * This is a projection of the core Layer 2 model for display purposes. It must
 * not invent state the core does not have.
 */

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

/** The kernels the control plane can run. */
export type Kernel = 'codex' | 'claude-code';

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
