/* First-party CMA computer glyph for the Workspace reference.
 * Neighboring deepseek-harness icons retain their provenance and
 * LICENSE-deepseek-harness.txt; this geometric monitor is CMA-authored.
 */
import type { IconProps } from './props';
export const IconComputerOutline16 = ({ size = 16, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    aria-hidden="true"
  >
    <rect x="1" y="2" width="14" height="9" rx="1" stroke="currentColor" />
    <path d="M8 11v3M5 14h6" stroke="currentColor" />
  </svg>
);
