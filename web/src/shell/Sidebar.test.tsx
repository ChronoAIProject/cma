import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Sidebar } from './Sidebar';
import { BUILD } from './build';

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

  it('offers operations only where the caller can operate', () => {
    // Presentation, never protection — the API refuses on its own — but a
    // control nobody can use should not be drawn.
    renderSidebar();
    expect(screen.queryByRole('button', { name: 'Operations' })).toBeNull();

    renderSidebar({ onOperations: vi.fn() });
    expect(screen.getByRole('button', { name: 'Operations' })).toBeInTheDocument();
  });

  it('says when operations is the view showing', () => {
    renderSidebar({ onOperations: vi.fn(), operationsActive: true });
    expect(screen.getByRole('button', { name: 'Operations' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
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

describe('the collapsed rail', () => {
  it('keeps every control reachable rather than hiding them', () => {
    /*
     * A panel that disappears takes its own way back with it. The rail is
     * what keeps "show me the other sessions" reachable at any width, so
     * every control survives the collapse — only their labels do not.
     */
    renderSidebar({ collapsed: true, onOperations: vi.fn() });

    expect(screen.getByRole('button', { name: 'Show the session list' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /find a session/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Operations' })).toBeInTheDocument();
  });

  it('drops the build and the status line, which have no room to be read', () => {
    // Truncated to nothing is worse than absent: a chip showing two characters
    // of a revision is a chip somebody will misread.
    renderSidebar({ collapsed: true, status: '14 sessions' });
    expect(screen.queryByText(BUILD)).toBeNull();
    expect(screen.queryByText(/14 sessions/)).toBeNull();
  });

  it('expands rather than opening a plate over a rail', async () => {
    // The results are a list, and a list needs the column the rail gave up.
    const user = userEvent.setup();
    const handlers = renderSidebar({ collapsed: true });

    await user.click(screen.getByRole('button', { name: /find a session/i }));
    expect(handlers.onToggleCollapsed).toHaveBeenCalledOnce();
    expect(handlers.onSearch).not.toHaveBeenCalled();
  });
});
