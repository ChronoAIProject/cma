import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SessionRail } from './SessionRail';
import { SessionHeader } from './SessionHeader';
import type { Session } from './types';
import { NO_CAPABILITIES } from '@chronoai/cma-sdk';

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

describe('the column headers', () => {
  it('leaves the band vocabulary behind entirely', () => {
    /*
     * The bands existed so column headers landed on one rule across a page
     * that had a bar above them. §9 gave every column its own proportions and
     * deleted the bar, so a header still claiming to be a band would be
     * claiming an alignment nothing holds it to.
     */
    const header = render(<SessionHeader session={SESSION} />);
    expect(header.container.firstElementChild).toHaveClass('sessionheader');
    expect(header.container.firstElementChild).not.toHaveClass('band');
  });

  it('has no band vocabulary left to share', () => {
    /*
     * The bands existed so headers across a page landed on one rule under one
     * app bar. §9 deleted the bar and gave every column its own proportions,
     * and this is the last of it: a shared class nothing renders is a rule
     * the next person will reach for and find does nothing.
     */
    expect(sheet('styles/components.css')).not.toMatch(/^\.band/m);
  });

  it('lets the centre column header set its own proportions', () => {
    /*
     * The one deliberate exception, and the reason it stopped being one: §9
     * gave the sidebar its own column, so this header no longer has to line
     * up with a list beside it. It belongs to the column beneath it and pads
     * itself accordingly.
     */
    expect(rule(sheet('sessions/SessionHeader.css'), '.sessionheader')).toContain(
      'padding: 10px 20px',
    );
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

  it('says which session the column is showing, without claiming to be it', () => {
    /*
     * A crumb, not a heading. The surface beneath is the content, and a
     * heading here would put this row in the page's outline as the thing
     * somebody came to read.
     */
    const { container } = render(<SessionHeader session={SESSION} />);
    const crumb = container.querySelector('.sessionheader__crumb');
    expect(crumb).not.toBeNull();
    expect(crumb?.tagName).toBe('SPAN');
    expect(container.querySelector('h1, h2, h3')).toBeNull();
  });
});
