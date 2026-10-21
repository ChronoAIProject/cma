import type { IconProps } from './icons';

/*
 * Glyphs drawn for CMA — `docs/Web_Style.md` §8.
 *
 * `icons/` holds only glyphs vendored from upstream under their licence, and
 * says so in every file. A glyph that set lacks is drawn here instead, to the
 * same contract: `{ size, className }`, painted with `currentColor`.
 */

/** A shield, for the composer's permission mode. */
export const IconShieldOutline16 = ({ size = 16, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path
      d="M8 1.6 13.2 3.5V7.4C13.2 10.5 11.1 13.1 8 14.4 4.9 13.1 2.8 10.5 2.8 7.4V3.5L8 1.6Z"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinejoin="round"
    />
  </svg>
);

/** A push pin, for keeping a collapsible header open. */
export const IconPinOutline16 = ({ size = 16, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path
      d="M9.9 1.9 14.1 6.1 12.6 6.6 10.1 9.1 10.3 12 9.1 13.2 2.8 6.9 4 5.7 6.9 5.9 9.4 3.4 9.9 1.9Z"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinejoin="round"
    />
    <path d="M5.9 10.1 2.2 13.8" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
  </svg>
);
