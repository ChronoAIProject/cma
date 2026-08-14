import './BouncingText.css';

/**
 * Text whose characters lift in sequence, left to right.
 *
 * Used while a sandbox is being provisioned, which can take a minute. A static
 * line for that long reads as a page that has stopped working; the motion says
 * something is still happening without claiming to know how far along it is,
 * which a progress bar would.
 *
 * The animation is decoration. The text is a normal string to assistive
 * technology and to anyone who has asked for less motion — per Web_Style.md,
 * information is never carried by the movement alone.
 */

interface BouncingTextProps {
  readonly text: string;
  /** Seconds between one character starting and the next. */
  readonly stagger?: number;
}

/** How far apart the characters start, in seconds. */
const DEFAULT_STAGGER = 0.045;

export function BouncingText({ text, stagger = DEFAULT_STAGGER }: BouncingTextProps) {
  return (
    <span className="bounce">
      {/*
       * The whole string is exposed once, so a screen reader announces a
       * sentence rather than spelling it out one letter at a time.
       */}
      <span className="sr-only">{text}</span>
      <span aria-hidden="true">
        {Array.from(text).map((character, index) => (
          <span
            // Characters repeat, so the index is the only stable identity here.
            key={`${character}-${String(index)}`}
            className="bounce__char"
            style={{ animationDelay: `${(index * stagger).toFixed(3)}s` }}
          >
            {/* A space cannot be lifted visibly and would collapse in flow. */}
            {character === ' ' ? ' ' : character}
          </span>
        ))}
      </span>
    </span>
  );
}
