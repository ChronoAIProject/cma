import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  MAX_LIVE_NODES,
  gridColumns,
  isCanvasArrow,
  nextNodeIndex,
  selectLiveWindow,
} from './layout';

const ids = (n: number) => Array.from({ length: n }, (_, i) => `s-${i}`);

describe('selectLiveWindow', () => {
  it('attaches everything when the fleet is under the cap', () => {
    const live = selectLiveWindow(ids(4), []);

    expect(live.size).toBe(4);
    expect([...live]).toEqual(['s-0', 's-1', 's-2', 's-3']);
  });

  it('never attaches more than the cap', () => {
    const live = selectLiveWindow(ids(40), []);

    expect(live.size).toBe(MAX_LIVE_NODES);
  });

  it('always keeps the most recently focused session attached', () => {
    // The 30th session is far outside the list-order window, but it is the one
    // someone is typing into.
    const live = selectLiveWindow(ids(40), ['s-29']);

    expect(live.has('s-29')).toBe(true);
    expect(live.size).toBe(MAX_LIVE_NODES);
  });

  it('evicts by least recent focus rather than by position', () => {
    const live = selectLiveWindow(ids(13), ['s-12', 's-11', 's-10'], 3);

    expect([...live]).toEqual(['s-12', 's-11', 's-10']);
  });

  it('ignores remembered sessions that no longer exist', () => {
    const live = selectLiveWindow(['s-1'], ['s-gone', 's-1'], 2);

    expect([...live]).toEqual(['s-1']);
  });

  it('attaches nothing for an empty fleet', () => {
    expect(selectLiveWindow([], ['s-1']).size).toBe(0);
  });

  it('refuses a cap that would attach nothing', () => {
    // A zero or negative budget would leave the focused node without a
    // terminal, which is worse than overspending by one.
    expect(selectLiveWindow(ids(3), ['s-2'], 0).size).toBe(1);
    expect([...selectLiveWindow(ids(3), ['s-2'], -5)]).toEqual(['s-2']);
  });
});

describe('gridColumns', () => {
  it('keeps the arrangement square-ish', () => {
    expect(gridColumns(1)).toBe(1);
    expect(gridColumns(2)).toBe(2);
    expect(gridColumns(4)).toBe(2);
    expect(gridColumns(5)).toBe(3);
    expect(gridColumns(9)).toBe(3);
    expect(gridColumns(10)).toBe(4);
  });

  it('never returns a column count a grid cannot use', () => {
    expect(gridColumns(0)).toBe(1);
    expect(gridColumns(-3)).toBe(1);
    expect(gridColumns(Number.NaN)).toBe(1);
  });
});

describe('isCanvasArrow', () => {
  it('claims only the navigation keys', () => {
    expect(isCanvasArrow('ArrowLeft')).toBe(true);
    expect(isCanvasArrow('End')).toBe(true);
    // Left to the browser and to the terminal: Tab moves out of the canvas and
    // Enter belongs to the focused control.
    expect(isCanvasArrow('Tab')).toBe(false);
    expect(isCanvasArrow('a')).toBe(false);
  });
});

describe('nextNodeIndex', () => {
  it('steps along a row', () => {
    expect(nextNodeIndex(0, 'ArrowRight', 9, 3)).toBe(1);
    expect(nextNodeIndex(1, 'ArrowLeft', 9, 3)).toBe(0);
  });

  it('steps a whole row for vertical keys', () => {
    expect(nextNodeIndex(0, 'ArrowDown', 9, 3)).toBe(3);
    expect(nextNodeIndex(7, 'ArrowUp', 9, 3)).toBe(4);
  });

  it('clamps at the edges instead of wrapping', () => {
    expect(nextNodeIndex(0, 'ArrowLeft', 9, 3)).toBe(0);
    expect(nextNodeIndex(0, 'ArrowUp', 9, 3)).toBe(0);
    expect(nextNodeIndex(8, 'ArrowRight', 9, 3)).toBe(8);
    expect(nextNodeIndex(8, 'ArrowDown', 9, 3)).toBe(8);
  });

  it('jumps to the ends', () => {
    expect(nextNodeIndex(4, 'Home', 9, 3)).toBe(0);
    expect(nextNodeIndex(4, 'End', 9, 3)).toBe(8);
  });

  it('recovers from an index that is not on the canvas', () => {
    // Focus can point at a session that was just removed; navigating from
    // nowhere must land somewhere rather than throw.
    expect(nextNodeIndex(-1, 'ArrowRight', 3, 2)).toBe(1);
    expect(nextNodeIndex(99, 'ArrowLeft', 3, 2)).toBe(0);
  });

  it('reports no destination on an empty canvas', () => {
    expect(nextNodeIndex(0, 'ArrowRight', 0, 1)).toBe(-1);
  });

  it('survives a nonsense column count', () => {
    expect(nextNodeIndex(0, 'ArrowDown', 4, 0)).toBe(1);
  });
});

describe('the node geometry', () => {
  it('is unchanged, because a node size is what its runtime draws at', () => {
    // §18: a 420x280 node asks for roughly 47 columns, and a node that changed
    // width as siblings came and went would re-wrap everything the agent has
    // drawn. Output already wrapped at 47 columns stays wrapped at 47 columns,
    // in the transcript afterwards as well — so this is not a styling value
    // and moving it is not a styling change.
    const tokens = readFileSync(resolve(process.cwd(), 'src/styles/tokens.css'), 'utf8');
    expect(tokens).toContain('--canvas-node-w: 420px');
    expect(tokens).toContain('--canvas-node-h: 280px');

    const css = readFileSync(resolve(process.cwd(), 'src/canvas/SessionCanvas.css'), 'utf8');
    expect(css).toContain('var(--canvas-node-w)');
    expect(css).toContain('var(--canvas-node-h)');
  });

  it('draws focus in accent and hover in weight, so the two cannot be confused', () => {
    // §18's table: hover changes weight within the neutral ramp; focus
    // changes hue and takes the --focus-node halo.
    const css = readFileSync(resolve(process.cwd(), 'src/canvas/SessionCanvas.css'), 'utf8');
    expect(css).toMatch(/\.node:hover\s*\{\s*border-color: var\(--dsw-alias-border-l2\);\s*\}/);
    expect(css).toMatch(
      /\.node\[data-focused='true'\]\s*\{[^}]*border-color: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--focus-node\)/,
    );
  });

  it('contains a terminal inside the fixed node while xterm measures it', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/canvas/SessionCanvas.css'), 'utf8');
    expect(css).toMatch(/\.node__body\s*\{[^}]*overflow: hidden/);
    expect(css).toMatch(/\.node \.terminal\s*\{[^}]*min-width: 0/);
  });
});
