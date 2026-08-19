import './Mark.css';

/**
 * The product mark — docs/Web_Style.md §17.
 *
 * Two by two, one cell lit. It is a clock quarter and a fleet with one session
 * live at the same time, and it inherits the checkered logic of the glyph it
 * replaces.
 *
 * Four squares rather than a glyph or an image: it needs no font, no SVG and
 * no asset, survives a 16px favicon, and prints in one colour. The face it
 * used to be drawn in was a pixel face §4 has since retired; a mark that
 * depended on a typeface would have gone dark with it.
 *
 * The two colours are custom properties on the element, so a variant —
 * knockout, on-accent, light — is the same geometry with two values
 * overridden rather than a class per surface.
 */

/**
 * How far apart the cells sit, per §17's ramp.
 *
 * The gap scales with the mark so the cells never fuse. Read as a ramp rather
 * than a formula because §17 states four sizes and no ratio fits them: at 16px
 * a second pixel of gap costs a quarter of each cell.
 */
export function gapFor(size: number): number {
  if (size >= 96) {
    return 8;
  }
  if (size >= 40) {
    return 4;
  }
  if (size >= 18) {
    return 2;
  }
  return 1;
}

interface MarkProps {
  /** Edge length in px. Defaults to the app bar's. */
  readonly size?: number;
}

export function Mark({ size = 18 }: MarkProps) {
  const gap = gapFor(size);
  return (
    <span
      className="mark"
      aria-hidden="true"
      style={{ width: `${size}px`, height: `${size}px`, gap: `${gap}px` }}
    >
      <span className="mark__cell" />
      <span className="mark__cell mark__cell--lit" />
      <span className="mark__cell" />
      <span className="mark__cell" />
    </span>
  );
}
