import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PageFrame } from './PageFrame';
import { ViewSwitch } from './ViewSwitch';

/**
 * The page frame — docs/Web_Style.md §6.
 *
 * The property worth asserting is not how the bar looks but that there is
 * exactly one of it, and one status band, on every page. The frontend grew a
 * second bar once already, when the console assembled its own chrome beside
 * the one App rendered.
 */

function renderFrame(props: Partial<Parameters<typeof PageFrame>[0]> = {}) {
  return render(
    <PageFrame showing="4 sessions · 1 live" {...props}>
      <div data-testid="surface" />
    </PageFrame>,
  );
}

describe('the page frame', () => {
  it('draws one app bar and one status band', () => {
    const { container } = renderFrame();
    expect(container.querySelectorAll('.topbar')).toHaveLength(1);
    expect(container.querySelectorAll('.statusband')).toHaveLength(1);
  });

  it('draws the same bar and band whether or not the page has views', () => {
    // §6: the window's top and bottom edges never move as somebody switches
    // view, so the frame's own markup must not depend on what it is given.
    const withoutViews = renderFrame().container.querySelector('.topbar')?.className;
    const withViews = renderFrame({
      views: (
        <ViewSwitch
          views={[{ name: 'sessions', label: 'Sessions' }]}
          active="sessions"
          onSelect={() => {}}
        />
      ),
    }).container.querySelector('.topbar')?.className;

    expect(withoutViews).toBe(withViews);
  });

  it('reads zone, then what the page shows, then the build', () => {
    const { container } = renderFrame();
    const band = container.querySelector('.statusband');
    const text = [...(band?.children ?? [])].map((node) => node.textContent);

    expect(text.slice(0, 2)).toEqual(['Zone', 'SGT']);
    expect(text).toContain('4 sessions · 1 live');
    expect(text.slice(-2)).toEqual(['Build', 'dev']);
  });

  it('states what it is not showing when the page says so', () => {
    // §13 requires the canvas to say how many sessions are unattached and why,
    // because silent truncation reads as breakage.
    renderFrame({ note: '3 sessions unattached' });
    expect(screen.getByText('3 sessions unattached')).toBeInTheDocument();
  });

  it('carries the command field disabled, saying why', () => {
    // §6 names the field; nothing in the document says what it does. §7:
    // controls are disabled, never removed, and every disabled control states
    // why. Accepting text it would silently drop is the worse answer.
    renderFrame();
    const command = screen.getByLabelText('Command');
    expect(command).toBeDisabled();
    expect(command).toHaveAttribute('placeholder', expect.stringContaining('not yet available'));
  });

  it('gives the page its own surface in the body', () => {
    const { container } = renderFrame();
    const body = container.querySelector('.pageframe__body');
    expect(body?.querySelector('[data-testid="surface"]')).not.toBeNull();
  });
});

describe('the view switch', () => {
  it('marks the active view by pressed state, not colour alone', () => {
    render(
      <ViewSwitch
        views={[
          { name: 'sessions', label: 'Sessions' },
          { name: 'operations', label: 'Operations' },
        ]}
        active="operations"
        onSelect={() => {}}
      />,
    );
    expect(screen.getByRole('button', { name: 'Operations' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(screen.getByRole('button', { name: 'Sessions' })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
  });

  it('draws a single view as a single segment rather than hiding it', () => {
    render(
      <ViewSwitch
        views={[{ name: 'sessions', label: 'Sessions' }]}
        active="sessions"
        onSelect={() => {}}
      />,
    );
    expect(screen.getAllByRole('button')).toHaveLength(1);
  });
});
