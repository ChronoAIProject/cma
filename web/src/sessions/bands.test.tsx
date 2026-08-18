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

  it('draws the foot band undivided', () => {
    // §6: one band across the whole window — no per-column cells, because the
    // columns no longer draw their own footers. The rail still takes its own
    // width from the tokens; the band no longer mirrors it.
    const band = sheet('shell/StatusBand.css');
    expect(band).not.toContain('.statusband__lead');
    expect(band).not.toContain('.statusband__cell');
    expect(rule(sheet('sessions/SessionRail.css'), '.rail')).toContain('width: var(--rail-width)');
    expect(rule(sheet('sessions/SessionRail.css'), '.rail--collapsed')).toContain(
      'width: var(--rail-collapsed)',
    );
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
    expect(chrome?.querySelector('.button--block')).not.toBeNull();

    // The rail draws no footer of its own: it ends at the status band's rule.
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
