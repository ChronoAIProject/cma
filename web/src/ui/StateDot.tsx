import './StateDot.css';

/**
 * A state, as a dot — `docs/Web_Style.md` §7.
 *
 * Reinforcement, never the signal. The dot is `aria-hidden` and every caller
 * pairs it with the word, so the state reads the same to somebody who cannot
 * distinguish the colours — which is the whole reason the pill it replaces
 * always drew its word too.
 *
 * Three of the four states are a solid core inside a same-coloured halo. The
 * fourth is not a colour at all: `ongoing` is a chase around the eight outer
 * cells of a 3×3 grid, because "running" is the one state that is a claim
 * about right now, and a still dot cannot tell you whether it is still true.
 */

/** What the dot says. */
export type StateDotState = 'done' | 'warning' | 'ongoing' | 'error';

/** The outer cells of a 3×3 grid, clockwise from top-left, on a 10px box. */
const CHASE_CELLS: readonly (readonly [number, number])[] = [
  [0, 0],
  [4, 0],
  [8, 0],
  [8, 4],
  [8, 8],
  [4, 8],
  [0, 8],
  [0, 4],
];

interface StateDotProps {
  readonly state: StateDotState;
  /** Outer edge in px. */
  readonly size?: number;
  /** Extra class for placement only. */
  readonly placement?: string;
}

export function StateDot({ state, size = 10, placement }: StateDotProps) {
  if (state === 'ongoing') {
    return (
      <svg
        className={['statedot__chase', placement ?? ''].filter(Boolean).join(' ')}
        data-state="ongoing"
        width={size}
        height={size}
        viewBox="0 0 10 10"
        /*
         * Crisp edges, deliberately. The cells are 2px squares on a 10px grid
         * and antialiasing them turns a pixel chase into a smudge.
         */
        shapeRendering="crispEdges"
        aria-hidden="true"
      >
        {CHASE_CELLS.map(([x, y], index) => (
          <rect
            key={`${x}-${y}`}
            className="statedot__cell"
            x={x}
            y={y}
            width="2"
            height="2"
            /*
             * A negative delay starts each cell part-way through the cycle, so
             * the chase is already running at mount rather than assembling
             * itself over the first second.
             */
            style={{ animationDelay: `${String((index - CHASE_CELLS.length) * 125)}ms` }}
          />
        ))}
      </svg>
    );
  }

  return (
    <span
      className={['statedot', placement ?? ''].filter(Boolean).join(' ')}
      data-state={state}
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  );
}
