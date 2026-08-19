/**
 * How a session's state is drawn — `Web_Style.md` §11.
 *
 * The model itself is the SDK's (`sdk/types.ts`) and is re-exported here so
 * this stays the one import a surface needs. What is left is presentation:
 * which semantic token paints a lifecycle, and what a tab calls it. Neither is
 * anything the control plane said, which is exactly why they did not travel
 * with the model.
 */

export * from '../sdk/types';

import type { SessionLifecycle } from '../sdk/types';

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
