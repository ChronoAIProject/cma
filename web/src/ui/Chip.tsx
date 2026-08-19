import './Chip.css';

/**
 * The status pill from docs/Web_Style.md §2 and §7: a dot and a word.
 *
 * One implementation rather than a class two stylesheets each declare: the
 * pill carries state, and a second copy is how one of them ends up 1px
 * different from the specification nobody rereads.
 *
 * The dot and its tone are reinforcement and never the signal. The pill
 * always draws the word, so it reads the same to somebody who cannot
 * distinguish the colours.
 */

/** Which semantic token the pill takes. */
export type Tone = 'live' | 'wait' | 'fail' | 'ended';

interface ChipProps {
  /** The state word. Always drawn — colour never carries meaning alone. */
  readonly children: string;
  readonly tone?: Tone;
  /**
   * Sits inside a line of prose rather than in a row of its own.
   *
   * Adds leading space. Without it a pill after a word touches that word.
   */
  readonly inline?: boolean;
}

export function Chip({ children, tone = 'ended', inline = false }: ChipProps) {
  return (
    <span className={`status status--${tone}${inline ? ' chip--inline' : ''}`}>
      <span className="status__dot" aria-hidden="true" />
      {children}
    </span>
  );
}
