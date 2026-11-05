/*
 * Task-home addition to the deepseek-harness-compatible icon interface.
 * The surrounding vendored set's MIT notice is in
 * ../../styles/vendor/LICENSE-deepseek-harness.txt.
 * This glyph is a local addition, not a copied upstream implementation.
 */
import type { IconProps } from './props';

/** First-party task-home glyph, Web_Style §8 and 2026-11-05. */
export const IconTableOutline16 = ({ size = 16, className }: IconProps) => (
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
    <rect x="2" y="2" width="12" height="12" rx="1.5" />
    <path d="M2 6h12M6 6v8M2 10h12" />
  </svg>
);
