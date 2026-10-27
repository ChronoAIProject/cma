import './StateDot.css';

/**
 * A state, as a dot — `docs/Web_Style.md` §12.1.
 *
 * Reinforcement, never the signal. The dot is `aria-hidden` and every caller
 * pairs it with the word, so the state reads the same to somebody who cannot
 * distinguish the colours — which is the whole reason the pill it replaces
 * always drew its word too.
 *
 * Three of the four states are a solid core inside a same-coloured halo. The
 * fourth is not colour alone: `ongoing` is a live core sending one expanding
 * ring, because "running" is a claim about right now and a still dot cannot
 * tell you whether it is still true.
 */

/** What the dot says. `neutral` is settled and usable, and claims nothing more. */
export type StateDotState = 'done' | 'neutral' | 'warning' | 'ongoing' | 'error';

interface StateDotProps {
  readonly state: StateDotState;
  /** Outer edge in px. */
  readonly size?: number;
  /** Extra class for placement only. */
  readonly placement?: string;
}

export function StateDot({ state, size = 10, placement }: StateDotProps) {
  return (
    <span
      className={['statedot', placement ?? ''].filter(Boolean).join(' ')}
      data-state={state}
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      {state === 'ongoing' ? <span className="statedot__ping" /> : null}
    </span>
  );
}
