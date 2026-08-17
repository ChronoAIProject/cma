import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SessionRail } from './SessionRail';
import { SessionHeader } from './SessionHeader';
import type { Session } from './types';

/**
 * The band grid — docs/Web_Style.md §5.
 *
 * Every column header is one head band tall and every footer one foot band, so
 * when two panels sit side by side their hairlines land on the same line and
 * the page reads as one surface. jsdom does not lay out, so what is asserted
 * here is the thing that makes the alignment hold: heights come from the band
 * classes, and no band adds padding to the block axis.
 */

const SESSION: Session = {
  id: 'ses_test',
  kernel: 'codex',
  lifecycle: 'running',
  createdAt: Date.parse('2026-08-18T08:53:00Z'),
};

function sheet(name: string): string {
  return readFileSync(resolve(process.cwd(), 'src', name), 'utf8');
}

function rule(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(^|\\})\\s*${escaped}\\s*\\{([^}]*)\\}`, 'm').exec(css)?.[2] ?? '';
}

describe('the band grid', () => {
  it('gives every column header the head band', () => {
    const rail = render(
      <SessionRail
        sessions={[SESSION]}
        activeId={null}
        order="newest"
        onSelect={() => {}}
        onCreate={() => {}}
        onReorder={() => {}}
        collapsed={false}
        onToggleCollapsed={() => {}}
      />,
    );
    expect(rail.container.querySelector('.rail__head')).toHaveClass('band', 'band--head');

    const header = render(<SessionHeader session={SESSION} />);
    expect(header.container.firstElementChild).toHaveClass('band', 'band--head');
  });

  it('sets band heights rather than accumulating them', () => {
    // §5: a band's height is SET on the band. Padding is what lets two aligned
    // columns drift apart when one gains a longer label, so no band rule may
    // add padding on the block axis.
    const components = sheet('styles/components.css');
    expect(rule(components, '.band--head')).toContain('height: var(--band-head)');
    expect(rule(components, '.band--foot')).toContain('height: var(--band-foot)');
    expect(rule(components, '.band')).not.toMatch(/padding(-block|-top|-bottom)?:/);

    for (const [name, selector] of [
      ['sessions/SessionRail.css', '.rail__head'],
      ['sessions/SessionHeader.css', '.sessionheader'],
      ['sessions/SessionDetails.css', '.details__head'],
    ] as const) {
      expect(rule(sheet(name), selector), selector).not.toMatch(
        /padding(-block|-top|-bottom)?:|height:/,
      );
    }
  });

  it('aligns the band leading cell with the rail rather than with its own text', () => {
    // The cell is the rail's share of the one 40px band, so it takes the
    // rail's width from the page. A cell sized from its content would put a
    // second vertical line a few pixels from the rail's, which reads as a
    // misprint; a cell pinned to the open width would leave it 264px adrift
    // the moment the rail collapsed.
    const band = sheet('shell/StatusBand.css');
    expect(rule(band, '.statusband__lead')).not.toContain('width:');
    // Follows the rail, and is absent on a view that has no rail: a vertical
    // rule drawn under nothing is worse than none.
    const console = sheet('Console.tsx');
    expect(console).toContain("'var(--rail-collapsed)'");
    expect(console).toContain("'var(--rail-width)'");
    expect(console).toMatch(/view === 'operations'\s*\?\s*undefined/);
    expect(rule(sheet('sessions/SessionRail.css'), '.rail')).toContain('width: var(--rail-width)');
    expect(rule(sheet('sessions/SessionRail.css'), '.rail--collapsed')).toContain(
      'width: var(--rail-collapsed)',
    );

    // Written at two-class specificity on purpose: the element carries .band
    // and .statusband, both set padding-inline, and equal specificity would
    // leave the winner decided by whichever stylesheet the bundler emitted
    // last — a 12px misalignment that comes and goes with import order.
    expect(band).toContain('.band.statusband {');
  });

  it('gives the rail a second band rather than a taller header', () => {
    // §5: a column needing more chrome adds a band beneath the first. It never
    // grows its header, or the head rule stops running straight across.
    const { container } = render(
      <SessionRail
        sessions={[SESSION]}
        activeId={null}
        order="newest"
        onSelect={() => {}}
        onCreate={() => {}}
        onReorder={() => {}}
        collapsed={false}
        onToggleCollapsed={() => {}}
      />,
    );
    const chrome = container.querySelector('.rail__chrome');
    expect(chrome).toHaveClass('band');
    expect(chrome).not.toHaveClass('band--head');
    expect(chrome?.querySelector('.rail__new')).not.toBeNull();

    // The rail draws no footer of its own: it ends at the status band's rule.
    expect(container.querySelector('.band--foot')).toBeNull();
  });

  it('carries the band subject at the one size §5 excepts', () => {
    const { container } = render(<SessionHeader session={SESSION} />);
    expect(container.querySelector('.band__subject')).not.toBeNull();
    expect(rule(sheet('styles/components.css'), '.band__subject')).toContain(
      'font-size: var(--text-lg)',
    );
    // Everything else in a band is mono xs — one type size per band.
    for (const selector of ['.band__label', '.band__value']) {
      expect(rule(sheet('styles/components.css'), selector)).toContain('font-size: var(--text-xs)');
    }
  });
});
