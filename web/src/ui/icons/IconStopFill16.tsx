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

/** ic_ds_stop_fill_16 */
export const IconStopFill16 = ({ size = 16, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path
      d="M2 4.88C2 3.68009 2 3.08013 2.30557 2.65954C2.40426 2.52371 2.52371 2.40426 2.65954 2.30557C3.08013 2 3.68009 2 4.88 2H11.12C12.3199 2 12.9199 2 13.3405 2.30557C13.4763 2.40426 13.5957 2.52371 13.6944 2.65954C14 3.08013 14 3.68009 14 4.88V11.12C14 12.3199 14 12.9199 13.6944 13.3405C13.5957 13.4763 13.4763 13.5957 13.3405 13.6944C12.9199 14 12.3199 14 11.12 14H4.88C3.68009 14 3.08013 14 2.65954 13.6944C2.52371 13.5957 2.40426 13.4763 2.30557 13.3405C2 12.9199 2 12.3199 2 11.12V4.88Z"
      fill="currentColor"
    />
  </svg>
);
