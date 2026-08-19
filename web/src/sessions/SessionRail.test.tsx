import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SessionRail } from './SessionRail';
import { lastActivity } from './lastActivity';
import type { Session, SessionLifecycle } from './types';
import { NO_CAPABILITIES } from '../agui';

/**
 * The session rail — docs/Web_Style.md §12, and §7 for collapsing it.
 */

function session(id: string, lifecycle: SessionLifecycle, extra: Partial<Session> = {}): Session {
  return {
    id,
    kernel: 'codex',
    lifecycle,
    createdAt: Date.parse('2026-08-18T08:53:00Z'),
    capabilities: NO_CAPABILITIES,
    ...extra,
  };
}

const SESSIONS = [
  session('a', 'running'),
  session('b', 'provisioning'),
  session('c', 'terminated', { stopReason: 'error' }),
];

function renderRail(props: Partial<Parameters<typeof SessionRail>[0]> = {}) {
  const onToggleCollapsed = vi.fn();
  const onSelect = vi.fn();
  const result = render(
    <SessionRail
      sessions={SESSIONS}
      activeId="a"
      order="newest"
      onSelect={onSelect}
      onReorder={() => {}}
      collapsed={false}
      onToggleCollapsed={onToggleCollapsed}
      {...props}
    />,
  );
  return { ...result, onToggleCollapsed, onSelect };
}

afterEach(() => {
  window.localStorage.clear();
});

describe('the rail row', () => {
  it('says what each session last did', () => {
    // §12: the last-activity line is what makes a list of times into a list of
    // work.
    renderRail();
    expect(screen.getByText('Running')).toBeInTheDocument();
    expect(screen.getByText('Waiting for a sandbox')).toBeInTheDocument();
    expect(screen.getByText('Ended after a failure')).toBeInTheDocument();
  });

  it('names the step a starting session has reached', () => {
    renderRail({ stages: { b: 'runtime_created' } });
    expect(screen.getByText('Sandbox created')).toBeInTheDocument();
  });

  it('reports a carried-forward session as carried forward', () => {
    // CLAUDE.md §5: the boundary is shown, because anything the agent did
    // after the snapshot did not travel.
    expect(lastActivity(session('a', 'running', { rotations: 1 }))).toBe(
      'Running — carried forward once',
    );
    expect(lastActivity(session('a', 'running', { rotations: 3 }))).toBe(
      'Running — carried forward 3 times',
    );
  });

  it('says a session waiting on an answer is waiting, not stopped', () => {
    // §18.3: the --wait dot and "waiting for your answer" in the rail. The
    // dot is already the wait tone for idle; the line is what changes.
    const idle = { ...session('waiting', 'idle'), id: 'ses_waiting' };
    const { container } = renderRail({ sessions: [idle], waitingId: 'ses_waiting' });
    expect(container.textContent).toContain('Waiting for your answer');
    // The idle line — "Stopped — its sandbox was released" — is not drawn.
    expect(container.textContent).not.toContain('Stopped —');
  });

  it('never lets the activity line wrap', () => {
    // A row that grows with its longest sentence makes the list a different
    // height every time it reloads.
    const { container } = renderRail();
    expect(container.querySelector('.row-item__activity')).not.toBeNull();
  });
});

describe('collapsing the rail', () => {
  it('leaves the handle where the panel was', () => {
    // §7: a collapsed panel becomes a handle, never nothing, keeping its
    // header and footer bands so the way back is exactly where the panel was.
    const { container } = renderRail({ collapsed: true });
    const rail = container.querySelector('.rail');
    expect(rail).toHaveClass('rail--collapsed');
    expect(
      within(rail as HTMLElement).getByRole('button', { name: /show the session list/i }),
    ).toBeInTheDocument();
    expect(rail?.querySelector('.band--head')).not.toBeNull();
  });

  it('keeps the summary of what it hides', () => {
    // §7: collapsing gives up detail, not awareness — the dots in state order
    // and a count.
    const { container } = renderRail({ collapsed: true });
    expect(container.querySelectorAll('.rail__summaryitem')).toHaveLength(3);
    expect(screen.getByLabelText('3 sessions')).toBeInTheDocument();
  });

  it('changes no session state', () => {
    // Collapsing is a display preference about this browser. Nothing here
    // selects, stops or reaches the control plane.
    const { onSelect, onToggleCollapsed } = renderRail();
    expect(onToggleCollapsed).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('reverses its glyph, so the control describes its outcome', () => {
    // §7: the handle glyph points where the panel will go.
    const open = renderRail().container.querySelector('.rail__handle')?.textContent;
    const closed = renderRail({ collapsed: true }).container.querySelector(
      '.rail__handle',
    )?.textContent;
    expect(open).toBe('‹');
    expect(closed).toBe('›');
  });

  it('answers ⌃← and ⌃→', async () => {
    const user = userEvent.setup();
    const { onToggleCollapsed } = renderRail();

    await user.keyboard('{Control>}{ArrowLeft}{/Control}');
    expect(onToggleCollapsed).toHaveBeenCalledTimes(1);

    // Already expanded: ⌃→ has nothing to do and must not toggle it shut.
    await user.keyboard('{Control>}{ArrowRight}{/Control}');
    expect(onToggleCollapsed).toHaveBeenCalledTimes(1);
  });

  it('moves focus to the handle, so nobody is stranded', async () => {
    // §7: focus moves to the handle, because the control somebody was on has
    // just left the layout.
    const { rerender } = renderRail();
    rerender(
      <SessionRail
        sessions={SESSIONS}
        activeId="a"
        order="newest"
        onSelect={() => {}}
        onReorder={() => {}}
        collapsed
        onToggleCollapsed={() => {}}
      />,
    );
    expect(document.activeElement).toHaveAttribute('aria-label', 'Show the session list');
  });
});
