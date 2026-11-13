/*
 * Task-home addition to the deepseek-harness-compatible icon interface.
 * The surrounding vendored set's MIT notice is in
 * ../../styles/vendor/LICENSE-deepseek-harness.txt.
 * This glyph is a local addition, not a copied upstream implementation.
 */
import type { IconProps } from './props';

/** First-party task-home glyph, Web_Style §8 and 2026-11-05. */
export const IconFileOutline16 = ({ size = 16, className }: IconProps) => (
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
    <path d="M9 2H3v12h10V6L9 2Zm0 0v4h4M5 9h6M5 11.5h4" />
  </svg>
);
