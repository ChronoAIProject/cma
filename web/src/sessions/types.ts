/**
 * Session model, mirroring docs/API_Overview.md.
 *
 * This is a projection of the core Layer 2 model for display purposes. It must
 * not invent state the core does not have.
 */

/** Lifecycle: provisioning -> ready -> running <-> idle -> terminated. */
export type SessionLifecycle = 'provisioning' | 'ready' | 'running' | 'idle' | 'terminated';

/** The kernels the control plane can run. */
export type Kernel = 'codex' | 'claude-code';

export interface Session {
  readonly id: string;
  readonly kernel: Kernel;
  readonly lifecycle: SessionLifecycle;
  /** Epoch ms the session was created, for the tab's age readout. */
  readonly createdAt: number;
}

/** Which semantic token paints a lifecycle state. */
export type StateTone = 'ok' | 'warn' | 'danger' | 'neutral';

/**
 * `idle` is the state most often misread. A session whose bot pod was
 * reclaimed to save resources is idle, and attaching to it revives it under
 * the same id — so it is a warning tone at worst, never danger.
 */
export function toneFor(lifecycle: SessionLifecycle): StateTone {
  switch (lifecycle) {
    case 'ready':
    case 'running':
      return 'ok';
    case 'provisioning':
    case 'idle':
      return 'warn';
    case 'terminated':
      return 'neutral';
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
