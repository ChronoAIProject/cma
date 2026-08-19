import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SessionRail } from './SessionRail';
import { SessionHeader } from './SessionHeader';
import type { Session } from './types';
import { NO_CAPABILITIES } from '../agui';

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
  capabilities: NO_CAPABILITIES,
};

function sheet(name: string): string {
  return readFileSync(resolve(process.cwd(), 'src', name), 'utf8');
}

function rule(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(^|\\})\\s*${escaped}\\s*\\{([^}]*)\\}`, 'm').exec(css)?.[2] ?? '';
}

describe('the band grid', () => {
  it('keeps the centre column header on the band', () => {
    // The centre column is the one that still shares the page's head rule.
    // §9 gave the sidebar its own column with its own proportions, so its
    // list no longer lines up with anything across the page and no longer
    // pretends to.
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
      ['sessions/SessionDrawer.css', '.drawer__head'],
    ] as const) {
      expect(rule(sheet(name), selector), selector).not.toMatch(
        /padding(-block|-top|-bottom)?:|height:/,
      );
    }
  });

  it('takes its width from the column it sits in', () => {
    /*
     * §9 moved the width decision out of this list and into the frame's
     * solver, which is the whole reason the column can be dragged. A width
     * declared here again would be one the solver could not honour, and the
     * two would disagree at every size.
     */
    const css = sheet('sessions/SessionRail.css');
    expect(rule(css, '.rail')).not.toContain('width:');
  });

  it('gives the rail one header row and no second band', () => {
    /*
     * It had two: a head band, and a second one beneath carrying the
     * new-session key, the filters and the sort. All three moved — the key to
     * the sidebar, the other two onto the view-options card — and what that
     * bought is the header dropping to a single row, which is height the list
     * gets.
     */
    const { container } = render(
      <SessionRail
        sessions={[SESSION]}
        activeId={null}
        order="newest"
        onSelect={() => {}}
        onReorder={() => {}}
        arrangement="list"
        onArrange={() => {}}
      />,
    );
    expect(container.querySelectorAll('.rail__header')).toHaveLength(1);
    expect(container.querySelector('.rail__chrome')).toBeNull();

    // The rail draws no footer of its own: the sidebar's foot carries one.
    expect(container.querySelector('.band--foot')).toBeNull();
  });

  it('carries the band subject at the one size §5 excepts', () => {
    const { container } = render(<SessionHeader session={SESSION} />);
    expect(container.querySelector('.band__subject')).not.toBeNull();
    expect(rule(sheet('styles/components.css'), '.band__subject')).toContain(
      'font-size: var(--text-subject)',
    );
    // Everything else in a band is mono at the label size — one size per band.
    for (const selector of ['.band__label', '.band__value']) {
      expect(rule(sheet('styles/components.css'), selector)).toContain(
        'font-size: var(--text-label)',
      );
    }
  });
});
