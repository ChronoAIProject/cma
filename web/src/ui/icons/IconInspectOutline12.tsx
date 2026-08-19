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

/** inspect_outline_12 (shared tool-row trajectory affordance glyph) */
export const IconInspectOutline12 = ({ size = 12, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden
  >
    <path
      d="M16 8L10.8571 12V10.552L14.1383 8L10.8571 5.448V4L16 8ZM5.14286 10.552L1.86171 8L5.14286 5.448V4L0 8L5.14286 12V10.552ZM9.02514 4L5.59657 12H6.84057L10.2691 4H9.02514Z"
      fill="currentColor"
    />
  </svg>
);
