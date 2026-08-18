import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
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
    const text = [...(band?.querySelectorAll('.band__label, .band__value') ?? [])].map(
      (node) => node.textContent,
    );

    expect(text).toEqual(['Zone', 'SGT', '4 sessions · 1 live', 'Build', 'dev']);
  });

  it('draws the band undivided, whatever columns the page has', () => {
    // §6: one band across the whole window. It no longer divides into
    // per-column cells, because the columns no longer draw their own footers.
    const { container } = renderFrame();
    expect(container.querySelector('.statusband__lead')).toBeNull();
    expect(container.querySelector('.statusband__cell')).toBeNull();
  });

  it('states what it is not showing when the page says so', () => {
    // §13 requires the canvas to say how many sessions are unattached and why,
    // because silent truncation reads as breakage.
    renderFrame({ note: '3 sessions unattached' });
    expect(screen.getByText('3 sessions unattached')).toBeInTheDocument();
  });

  it('draws the command trigger as a real control, never disabled', () => {
    // §6: the trigger is a working control with its key hint. A page with no
    // command bar to open draws no trigger — absent, never a dead field.
    expect(renderFrame().container.querySelector('.topbar__trigger')).toBeNull();

    const onCommand = vi.fn();
    renderFrame({ onCommand });
    const trigger = screen.getByRole('button', {
      name: /find a session, or type a command/i,
    });
    expect(trigger).toBeEnabled();
    expect(trigger.textContent).toContain('⌘K');
  });

  it('carries the live count beside the account control', () => {
    // §6's bar order ends: … the live count, the account control. The count
    // is data — absent when the page has no sessions to count.
    expect(renderFrame().container.querySelector('.topbar__live')).toBeNull();
    const { container } = renderFrame({ live: 2 });
    expect(container.querySelector('.topbar__live')?.textContent).toContain('2 live');
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
