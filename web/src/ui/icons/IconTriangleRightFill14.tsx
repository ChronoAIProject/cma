/*
 * Vendored from deepseek-harness (deepseek-ai/deepseek-harness),
 * packages/client/ui-primitives/src/icons/index.tsx, at the commit current on
 * 2026-08-20. Copied under its MIT licence, whose full text and copyright
 * notice are in ../../styles/vendor/LICENSE-deepseek-harness.txt.
 *
 * One glyph per file. The upstream set is a single 872-line module, which is
 * over this repository's file ceiling and would also make every icon a reason
 * to touch one file. The path is the glyph.
 */
import type { IconProps } from './props';

/** ic_ds_triangle_right_fill_14 — tree expand arrow; points right, consumers rotate it 90° for the open state. */
export const IconTriangleRightFill14 = ({ size = 14, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 14 14"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path
      d="M4.25 2.82782L4.25 11.1722C4.25 11.6622 4.84243 11.9076 5.18891 11.5611L9.36109 7.38891C9.57588 7.17412 9.57588 6.82588 9.36109 6.61109L5.18891 2.43891C4.84243 2.09243 4.25 2.33782 4.25 2.82782Z"
      fill="currentColor"
    />
  </svg>
);
