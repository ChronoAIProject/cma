/*
 * Task-home addition to the deepseek-harness-compatible icon interface.
 * The surrounding vendored set's MIT notice is in
 * ../../styles/vendor/LICENSE-deepseek-harness.txt.
 * This glyph is a local addition, not a copied upstream implementation.
 */
import type { IconProps } from './props';

/** First-party task-home glyph, Web_Style §8 and 2026-11-05. */
export const IconCodeBracketsOutline16 = ({ size = 16, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.5"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path d="m5 4-4 4 4 4m6-8 4 4-4 4M9.5 2l-3 12" />
  </svg>
);
