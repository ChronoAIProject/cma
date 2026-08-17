import { useEffect, useState } from 'react';

/**
 * Which of §16's four widths the window is at — docs/Web_Style.md.
 *
 * The number of columns changes and the bands never do: 52px header and 40px
 * footer hold at every width, so the page reads as the same shape on a phone
 * as on a desk.
 *
 * Named rather than numeric because the layout decisions are named in §16, and
 * a component asking "is the window narrower than 960px" would be a component
 * that has to be re-read against the document every time either changes.
 */
export type Width =
  /** ≥ 75rem — three columns; nothing collapses on its own. */
  | 'wide'
  /** 60–75rem — the rail arrives collapsed to its 48px strip. */
  | 'medium'
  /** < 60rem — the rail leaves the layout and becomes a drawer over it. */
  | 'narrow'
  /** < 30rem — the bar drops to 48px and gives its chrome to the drawer. */
  | 'compact';

/** §16's boundaries, in px at the browser's default root size. */
const WIDE = 75 * 16;
const MEDIUM = 60 * 16;
const COMPACT = 30 * 16;

export function widthFor(px: number): Width {
  if (px < COMPACT) {
    return 'compact';
  }
  if (px < MEDIUM) {
    return 'narrow';
  }
  if (px < WIDE) {
    return 'medium';
  }
  return 'wide';
}

/** Whether this width has room for the rail as a column of its own. */
export function railIsColumn(width: Width): boolean {
  return width === 'wide' || width === 'medium';
}

/**
 * Whether the canvas is worth offering — §16.
 *
 * "A grid of terminals in a narrow window is several terminals nobody can
 * read." Offered at 60rem and above only; below that the option is disabled
 * with its reason rather than removed, per §7.
 */
export function canvasFits(width: Width): boolean {
  return width === 'wide' || width === 'medium';
}

/**
 * The window's width band, kept current.
 *
 * Read from `matchMedia` rather than from a resize listener: the browser
 * already knows when a boundary is crossed, and a listener firing on every
 * pixel of a drag would re-render the console a hundred times to answer a
 * question with four possible answers.
 */
export function useWidth(): Width {
  const [width, setWidth] = useState<Width>(() =>
    typeof window === 'undefined' ? 'wide' : widthFor(window.innerWidth),
  );

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') {
      // Nothing to listen to. The width measured at mount is the answer, which
      // is still better than assuming the widest case.
      return;
    }
    const queries = [WIDE, MEDIUM, COMPACT].map((px) =>
      window.matchMedia(`(min-width: ${String(px)}px)`),
    );
    const update = () => {
      setWidth(widthFor(window.innerWidth));
    };
    for (const query of queries) {
      query.addEventListener('change', update);
    }
    update();
    return () => {
      for (const query of queries) {
        query.removeEventListener('change', update);
      }
    };
  }, []);

  return width;
}
