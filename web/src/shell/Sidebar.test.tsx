import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';
import { BUILD } from './build';
import { creationAbilityFor, type Allowance } from '../sessions/availability';

/**
 * The sidebar — `docs/Web_Style.md` §10.
 *
 * It absorbed the app bar and the status band, so what is checked here is
 * that nothing they carried was quietly dropped on the way: the build, the
 * count, what the canvas is not showing, and the way to every control that
 * used to live along the top of the window.
 */

function renderSidebar(props: Partial<Parameters<typeof Sidebar>[0]> = {}) {
  const handlers = {
    onToggleCollapsed: vi.fn(),
    onCreate: vi.fn(),
    onSearch: vi.fn(),
  };
  render(
    <Sidebar
      collapsed={false}
      creation={{ canCreate: true }}
      account={<button type="button">Account</button>}
      {...handlers}
      {...props}
    >
      <p>the list</p>
    </Sidebar>,
  );
  return handlers;
}

describe('the sidebar', () => {
  it('carries the build the page was served from', () => {
    // The status band's one fact somebody quotes in a report.
    renderSidebar();
    expect(screen.getByText(BUILD)).toBeInTheDocument();
  });

  it('says what the list is showing, and what it is not', () => {
    /*
     * The canvas drops sessions past its attachment budget, and truncation
     * that says nothing reads as breakage. It sits with the list because the
     * list is what it is about.
     */
    renderSidebar({ status: '14 sessions · 3 live · 2 not attached' });
    expect(screen.getByText(/2 not attached/)).toBeInTheDocument();
  });

  it('starts a session from the mark as well as from the key', async () => {
    // The mark is a shortcut, and it carries no hover chrome: a fill on a
    // wordmark reads as the product's name being a control.
    const user = userEvent.setup();
    const handlers = renderSidebar();

    await user.click(screen.getByRole('button', { name: /new session/i }));
    expect(handlers.onCreate).toHaveBeenCalledOnce();
  });

  it('opens the command plate from the search row', async () => {
    const user = userEvent.setup();
    const handlers = renderSidebar();

    await user.click(screen.getByRole('button', { name: /find a session/i }));
    expect(handlers.onSearch).toHaveBeenCalledOnce();
    expect(handlers.onToggleCollapsed).not.toHaveBeenCalled();
  });
});

describe('starting another session', () => {
  const enabled: readonly { readonly name: string; readonly allowance: Allowance | undefined }[] = [
    { name: 'under the limit', allowance: { limit: 3, live: 2, exempt: false } },
    { name: 'exempt', allowance: { limit: 3, live: 3, exempt: true } },
    { name: 'with no configured limit', allowance: { live: 12, exempt: false } },
    { name: 'after the advisory read failed', allowance: undefined },
  ];

  it.each(enabled)('enables both creation shortcuts $name', ({ allowance }) => {
    renderSidebar({ creation: creationAbilityFor(allowance) });
    expect(screen.getByRole('button', { name: 'CMA' })).toBeEnabled();
    expect(screen.getByRole('button', { name: /new session/i })).toBeEnabled();
    expect(document.querySelector('.control-reason')).toBeNull();
  });

  it('disables both shortcuts at the limit and draws their shared reason once', async () => {
    const user = userEvent.setup();
    const creation = creationAbilityFor({ limit: 3, live: 3, exempt: false });
    const handlers = renderSidebar({ creation });
    const brand = screen.getByRole('button', { name: 'CMA' });
    const key = screen.getByRole('button', { name: /new session/i });

    expect(brand).toBeDisabled();
    expect(key).toBeDisabled();
    expect(brand).toHaveAttribute('aria-describedby', 'sidebar-creation-reason');
    expect(key).toHaveAttribute('aria-describedby', 'sidebar-creation-reason');
    expect(screen.getAllByText(creation.reason ?? '')).toHaveLength(1);

    await user.click(brand);
    await user.click(key);
    expect(handlers.onCreate).not.toHaveBeenCalled();
  });
});

describe('the collapsed rail', () => {
  it('keeps every control reachable rather than hiding them', () => {
    /*
     * A panel that disappears takes its own way back with it. The rail is
     * what keeps "show me the other sessions" reachable at any width, so
     * every control survives the collapse — only their labels do not.
     */
    renderSidebar({ collapsed: true });

    expect(screen.getByRole('button', { name: 'Show the session list' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /find a session/i })).toBeInTheDocument();
    // The account control survives too, and its tests live with it: it used to
    // be replaced by a static glyph here, leaving no way to sign out.
    expect(screen.getByRole('button', { name: 'Account' })).toBeInTheDocument();
  });

  it('drops the build and the status line, which have no room to be read', () => {
    // Truncated to nothing is worse than absent: a chip showing two characters
    // of a revision is a chip somebody will misread.
    renderSidebar({ collapsed: true, status: '14 sessions' });
    expect(screen.queryByText(BUILD)).toBeNull();
    expect(screen.queryByText(/14 sessions/)).toBeNull();
  });

  it('opens the command plate, closed or open', async () => {
    /*
     * It used to expand the sidebar instead, on the reasoning that results are
     * a list and a list needs a column. The plate carries its own list over
     * whatever is behind it, and a control that does something else depending
     * on a state somewhere else is one nobody can predict — pressing search
     * and being handed a sidebar is not a slower way to search.
     */
    const user = userEvent.setup();
    const handlers = renderSidebar({ collapsed: true });

    await user.click(screen.getByRole('button', { name: /find a session/i }));
    expect(handlers.onSearch).toHaveBeenCalledOnce();
    expect(handlers.onToggleCollapsed).not.toHaveBeenCalled();
  });
});

describe('the brand row is set the way the brand is', () => {
  it("sets the wordmark at the lockup's weight and tracking, never held apart", () => {
    /*
     * `docs/Web_Style.md` §21. The wordmark was on +0.04em — positive
     * tracking, which is the opposite sign from the lockup and is a treatment
     * for small caps and label runs. Held apart, a three-letter product name
     * at reading size reads as a category label.
     *
     * Asserted against the sheet rather than a rendered node because jsdom
     * resolves no custom property, so a computed style here would report the
     * token name back rather than what a browser paints.
     */
    const css = readFileSync(resolve(process.cwd(), 'src/shell/Sidebar.css'), 'utf8');
    const rule = /\.sidebar__name\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).toMatch(/font-weight:\s*700/);
    expect(rule).toMatch(/letter-spacing:\s*var\(--track-display\)/);
    // The mutation this kills: any positive tracking creeping back in.
    expect(rule).not.toMatch(/letter-spacing:\s*0?\.\d/);
  });

  it('gives the mark one size that serves the open rail and the closed one', () => {
    // 32px inside the collapsed rail's 36px square, so no second rule is
    // needed for the closed state — and a second rule is how the two drift.
    const css = readFileSync(resolve(process.cwd(), 'src/shell/Sidebar.css'), 'utf8');
    const mark = /\.sidebar__mark\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(mark).toMatch(/width:\s*32px/);
    expect(mark).toMatch(/height:\s*32px/);
    expect(mark).toMatch(/color:\s*var\(--dsw-alias-brand-primary\)/);
    expect(css).toMatch(
      /\.sidebar__mark svg\s*\{[^}]*display:\s*block[^}]*width:\s*100%[^}]*height:\s*100%/,
    );
    expect(css).not.toMatch(/\.sidebar--collapsed\s+\.sidebar__mark/);
  });

  it('inlines the mark so the appearance can ink it', () => {
    renderSidebar();
    const wrapper = document.querySelector('.sidebar__mark');
    expect(wrapper).toHaveAttribute('aria-hidden', 'true');
    expect(wrapper).toBeInstanceOf(HTMLSpanElement);
    expect(wrapper?.querySelector('svg')).not.toBeNull();
    expect(wrapper?.querySelector('img')).toBeNull();
  });
});
