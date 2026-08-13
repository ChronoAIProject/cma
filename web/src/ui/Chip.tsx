import './Chip.css';

/**
 * The status chip from docs/Web_Style.md section 6.
 *
 * One implementation rather than a class two stylesheets each declare: the chip
 * carries state, and a second copy is how one of them ends up 1px different
 * from the specification nobody rereads.
 *
 * The tone is reinforcement and never the signal. The chip always draws the
 * word, so it reads the same to somebody who cannot distinguish the colours.
 */

/** Which semantic token the chip takes. */
export type Tone = 'ok' | 'warn' | 'danger' | 'neutral';

interface ChipProps {
  /** The state word. Always drawn — colour never carries meaning alone. */
  readonly children: string;
  readonly tone?: Tone;
  /**
   * Sits inside a line of prose rather than in a row of its own.
   *
   * Adds leading space. Without it a chip after a word touches that word.
   */
  readonly inline?: boolean;
}

export function Chip({ children, tone = 'neutral', inline = false }: ChipProps) {
  return <span className={`chip chip--${tone}${inline ? ' chip--inline' : ''}`}>{children}</span>;
}
