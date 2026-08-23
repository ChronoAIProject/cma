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

/** ic_ds_right_up_outline_16 */
export const IconRightUpOutline16 = ({ size = 16, className }: IconProps) => (
  <svg
    width={size}
    height={size}
    className={className}
    viewBox="0 0 16 16"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
  >
    <path
      d="M13.588429 5.147807C13.588429 4.739638 13.587271 4.403003 13.582013 4.118684L1.703098 15.99968L0.85155 15.148178L0 14.294485L11.878915 2.413442C11.594721 2.408199 11.257569 2.409154 10.849776 2.409154H2.400594V0.000001H10.849776C11.644471 0.000001 12.338899 -0.001059 12.901622 0.059909C13.486363 0.123352 14.071136 0.265493 14.598303 0.648292C14.886598 0.857751 15.141981 1.110984 15.351433 1.399281C15.734578 1.926807 15.876362 2.512925 15.939743 3.098105C16.000775 3.660718 15.99968 4.353347 15.99968 5.147807V13.599133H13.588429V5.147807Z"
      fill="currentColor"
    />
  </svg>
);
