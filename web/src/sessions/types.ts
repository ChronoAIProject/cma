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

import type { StateDotState } from '../ui/StateDot';
import type { SessionLifecycle } from '../sdk/types';

/** Which semantic token paints a lifecycle state. */
export type StateTone = 'live' | 'wait' | 'fail' | 'ended';

/**
 * The design's state vocabulary for one of ours — `Web_Style.md` §7.
 *
 * `undefined` for a session that has ended, and deliberately: the dot's four
 * states all assert something about right now — succeeding, needing you,
 * running, failed — and a terminated session asserts none of them. Painting
 * it green because green was the nearest colour would claim it finished well,
 * which is a different fact and one this layer does not have.
 */
export function stateDotFor(tone: StateTone): StateDotState | undefined {
  switch (tone) {
    case 'live':
      return 'ongoing';
    case 'wait':
      return 'warning';
    case 'fail':
      return 'error';
    case 'ended':
      return undefined;
    default: {
      const exhaustive: never = tone;
      return exhaustive;
    }
  }
}

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
