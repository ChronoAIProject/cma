import { describe, expect, it } from 'vitest';
import { placeMenu } from './menuPlacement';

/**
 * Where a portalled menu goes — `docs/Web_Style.md` §7.
 *
 * Every case here is a menu that would otherwise open partly off-screen,
 * which is not something a render test notices: jsdom lays nothing out, and
 * in a browser it only shows up near an edge somebody has to think to try.
 */

/** A rectangle, the way the DOM hands one over. */
function rect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

const VIEWPORT = { width: 1000, height: 800 };
const LIST = { width: 218, height: 200 };

describe('placing a menu', () => {
  it('hangs from the anchor with a gap', () => {
    const at = placeMenu(rect(100, 100, 120, 36), LIST, VIEWPORT, 'bottom', 'start');
    expect(at).toEqual({ left: 100, top: 140 });
  });

  it('opens upward when asked, clearing the anchor', () => {
    // The list's own height is the offset, so this is the case that is wrong
    // by exactly the list height when it is wrong at all.
    const at = placeMenu(rect(100, 400, 120, 36), LIST, VIEWPORT, 'top', 'start');
    expect(at).toEqual({ left: 100, top: 196 });
  });

  it('lines up its trailing edge with the anchor when aligned to the end', () => {
    const at = placeMenu(rect(600, 100, 120, 36), LIST, VIEWPORT, 'bottom', 'end');
    expect(at.left).toBe(720 - LIST.width);
  });

  it('opens beside the anchor, not below it, on the right side', () => {
    const at = placeMenu(rect(100, 100, 120, 36), LIST, VIEWPORT, 'right', 'start');
    expect(at).toEqual({ left: 224, top: 100 });
  });

  it('is nudged back rather than flipped at the right edge', () => {
    /*
     * Flipping to the other side of the anchor would move the list somewhere
     * the pointer is not, which is worse than a few pixels of offset: the
     * list should still read as belonging to the control it came from.
     */
    const at = placeMenu(rect(950, 100, 40, 36), LIST, VIEWPORT, 'bottom', 'start');
    expect(at.left).toBe(VIEWPORT.width - LIST.width - 12);
    expect(at.left).toBeLessThan(950);
  });

  it('is nudged down at the top edge and up at the bottom', () => {
    const high = placeMenu(rect(100, 10, 120, 36), LIST, VIEWPORT, 'top', 'start');
    expect(high.top).toBe(12);

    const low = placeMenu(rect(100, 780, 120, 36), LIST, VIEWPORT, 'bottom', 'start');
    expect(low.top).toBe(VIEWPORT.height - LIST.height - 12);
  });

  it('places but does not clamp a list it has not measured yet', () => {
    /*
     * A zero-size list clamped against the right edge would be pinned to the
     * margin and then visibly jump once it measured. Placement without
     * clamping is the honest answer for one frame.
     */
    const at = placeMenu(
      rect(950, 100, 40, 36),
      { width: 0, height: 0 },
      VIEWPORT,
      'bottom',
      'start',
    );
    expect(at.left).toBe(950);
  });
});
