import { describe, expect, it } from 'vitest';
import {
  CENTER_MIN,
  DETAILS_DEFAULT,
  DETAILS_MAX,
  DETAILS_MIN,
  SIDEBAR_COLLAPSED,
  SIDEBAR_DEFAULT,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  clampWidth,
  computeColumns,
} from './columns';

/**
 * The concession chain — `docs/Web_Style.md` §9.
 *
 * Tested as arithmetic rather than through a render, because that is what it
 * is: jsdom lays nothing out, so a layout test here would assert only that
 * numbers were passed along. What matters is which column gives way, in what
 * order, and what happens when the window comes back.
 */

describe('clamping a dragged width', () => {
  it('holds a width inside its range', () => {
    expect(clampWidth(100, SIDEBAR_MIN, SIDEBAR_MAX)).toBe(SIDEBAR_MIN);
    expect(clampWidth(9999, SIDEBAR_MIN, SIDEBAR_MAX)).toBe(SIDEBAR_MAX);
    expect(clampWidth(300, SIDEBAR_MIN, SIDEBAR_MAX)).toBe(300);
  });

  it('rounds, so a drag never leaves a fractional column', () => {
    // Sub-pixel widths make a 1px border render as a grey smear on one edge
    // and not the other.
    expect(clampWidth(300.4, SIDEBAR_MIN, SIDEBAR_MAX)).toBe(300);
    expect(clampWidth(300.6, SIDEBAR_MIN, SIDEBAR_MAX)).toBe(301);
  });
});

describe('solving the three columns', () => {
  it('gives everything its preferred width when there is room', () => {
    const at = computeColumns(1600, SIDEBAR_DEFAULT, DETAILS_DEFAULT);
    expect(at.sidebar).toBe(SIDEBAR_DEFAULT);
    expect(at.details).toBe(DETAILS_DEFAULT);
    expect(at.center).toBe(1600 - SIDEBAR_DEFAULT - DETAILS_DEFAULT);
  });

  it('gives the centre every pixel nothing else claimed', () => {
    // The centre is where the work is. It takes the slack rather than leaving
    // a gap, at every width where there is any.
    for (const viewport of [1200, 1440, 1600, 2560]) {
      const at = computeColumns(viewport, SIDEBAR_DEFAULT, DETAILS_DEFAULT);
      expect(at.sidebar + at.center + at.details).toBe(viewport);
    }
  });

  it('shrinks the details column before anything else', () => {
    /*
     * The squeeze band is narrow and worth stating: everything fits at
     * 344 + 360 + 640 = 1344, and the details column cannot go under
     * 344 + 300 + 640 = 1284. So shrinking happens between those two, and
     * below 1284 there is nothing left to shrink.
     */
    const viewport = SIDEBAR_DEFAULT + CENTER_MIN + (DETAILS_DEFAULT + DETAILS_MIN) / 2;
    const at = computeColumns(viewport, SIDEBAR_DEFAULT, DETAILS_DEFAULT);
    expect(at.sidebar).toBe(SIDEBAR_DEFAULT);
    expect(at.center).toBe(CENTER_MIN);
    expect(at.details).toBe(viewport - SIDEBAR_DEFAULT - CENTER_MIN);
    expect(at.details).toBeLessThan(DETAILS_DEFAULT);
    expect(at.details).toBeGreaterThanOrEqual(DETAILS_MIN);
  });

  it('closes the details column the moment shrinking would go under its floor', () => {
    // One pixel either side of the boundary the test above names.
    expect(
      computeColumns(SIDEBAR_DEFAULT + CENTER_MIN + DETAILS_MIN, SIDEBAR_DEFAULT, DETAILS_DEFAULT)
        .details,
    ).toBe(DETAILS_MIN);
    expect(
      computeColumns(
        SIDEBAR_DEFAULT + CENTER_MIN + DETAILS_MIN - 1,
        SIDEBAR_DEFAULT,
        DETAILS_DEFAULT,
      ).details,
    ).toBe(0);
  });

  it('closes the details column rather than taking it under its minimum', () => {
    // A 200px details column is not a narrow details column; it is one that
    // cannot show what it exists to show.
    const at = computeColumns(1000, SIDEBAR_DEFAULT, DETAILS_DEFAULT);
    expect(at.details).toBe(0);
    expect(at.sidebar).toBe(SIDEBAR_DEFAULT);
    expect(at.center).toBe(1000 - SIDEBAR_DEFAULT);
  });

  it('never takes width from the sidebar', () => {
    /*
     * It is the way to every other resource. A layout that took it away to
     * make room would be one where the fix for "this is too narrow" is
     * itself unreachable.
     */
    for (const viewport of [400, 700, 1000, 1280]) {
      expect(computeColumns(viewport, SIDEBAR_DEFAULT, DETAILS_DEFAULT).sidebar).toBe(
        SIDEBAR_DEFAULT,
      );
    }
  });

  it('lets the centre go under its floor only when there is nothing left', () => {
    // The last resort, and it is still a layout rather than a negative width.
    const at = computeColumns(500, SIDEBAR_DEFAULT, DETAILS_DEFAULT);
    expect(at.details).toBe(0);
    expect(at.center).toBe(500 - SIDEBAR_DEFAULT);
    expect(at.center).toBeLessThan(CENTER_MIN);
    expect(at.center).toBeGreaterThan(0);
  });

  it('never returns a negative column', () => {
    // A viewport narrower than the sidebar itself is not a real layout, but a
    // negative width is a broken one rather than a cramped one.
    const at = computeColumns(100, SIDEBAR_DEFAULT, DETAILS_DEFAULT);
    expect(at.center).toBe(0);
  });

  it('resolves a closed sidebar to its rail rather than to nothing', () => {
    const at = computeColumns(1600, 0, DETAILS_DEFAULT);
    expect(at.sidebar).toBe(SIDEBAR_COLLAPSED);
    expect(at.center).toBe(1600 - SIDEBAR_COLLAPSED - DETAILS_DEFAULT);
  });

  it('leaves a closed details column closed at every width', () => {
    for (const viewport of [800, 1440, 2560]) {
      expect(computeColumns(viewport, SIDEBAR_DEFAULT, 0).details).toBe(0);
    }
  });

  it('restores what narrowing took away, because narrowing wrote nothing down', () => {
    /*
     * The property the whole solver exists for. A closed details column is a
     * derived zero, not a preference of zero — so the preference survives the
     * squeeze and the window coming back reopens it at the width its owner
     * chose, without anybody having stored a "was open before" flag.
     */
    const preference = 480;
    const squeezed = computeColumns(900, SIDEBAR_DEFAULT, preference);
    expect(squeezed.details).toBe(0);

    const restored = computeColumns(1600, SIDEBAR_DEFAULT, preference);
    expect(restored.details).toBe(preference);
  });

  it('re-clamps a preference that arrived out of range', () => {
    // Preferences cross a storage boundary; a stale one written by an older
    // build must not reach the layout as a width it cannot draw.
    const at = computeColumns(2000, 9999, 9999);
    expect(at.sidebar).toBe(SIDEBAR_MAX);
    expect(at.details).toBe(DETAILS_MAX);
  });
});
