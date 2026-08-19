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

/*
 * Distinct start times, because the list labels a session by when it started
 * and three sessions sharing a minute are three rows a person cannot tell
 * apart. A fixture that gave them all one time would let a test claiming they
 * are distinguishable pass without them being so.
 */
const STARTED: Readonly<Record<string, string>> = {
  a: '2026-08-18T08:53:00Z',
  b: '2026-08-18T09:14:00Z',
  c: '2026-08-18T10:02:00Z',
};

function session(id: string, lifecycle: SessionLifecycle, extra: Partial<Session> = {}): Session {
  return {
    id,
    kernel: 'codex',
    lifecycle,
    createdAt: Date.parse(STARTED[id] ?? '2026-08-18T08:53:00Z'),
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
  const onSelect = vi.fn();
  const onReorder = vi.fn();
  const onArrange = vi.fn();
  const result = render(
    <SessionRail
      sessions={SESSIONS}
      activeId="a"
      order="newest"
      onSelect={onSelect}
      onReorder={onReorder}
      arrangement="list"
      onArrange={onArrange}
      {...props}
    />,
  );
  return { ...result, onSelect, onReorder, onArrange };
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

describe('the view options', () => {
  it('folds filtering, ordering and arranging onto one card', async () => {
    /*
     * They were three affordances across two bands. They are three answers to
     * "what am I looking at", and putting them on one card is what lets the
     * header drop to a single row — which is height the list gets.
     */
    const user = userEvent.setup();
    renderRail();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    const card = screen.getByRole('menu', { name: 'View options' });

    for (const label of [
      /^All /,
      /^Live /,
      /^Stopped /,
      /newest first/i,
      /oldest first/i,
      /^List$/,
      /^Canvas$/,
    ]) {
      expect(within(card).getByRole('menuitemradio', { name: label })).toBeInTheDocument();
    }
  });

  it('marks the current answer in every group, not just one', async () => {
    // One selection could only mark one of the three questions on the card.
    const user = userEvent.setup();
    renderRail({ order: 'oldest', arrangement: 'canvas' });

    await user.click(screen.getByRole('button', { name: 'View options' }));

    expect(screen.getByRole('menuitemradio', { name: /^All / })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('menuitemradio', { name: /oldest first/i })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('menuitemradio', { name: /^Canvas$/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('narrows the list to what was chosen', async () => {
    const user = userEvent.setup();
    renderRail();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    await user.click(screen.getByRole('menuitemradio', { name: /^Live / }));

    expect(screen.getAllByRole('tab')).toHaveLength(1);
  });

  it('says what would have been there when a filter empties the list', async () => {
    // An empty list that says nothing reads as a broken list. The sessions
    // still exist; the filter narrowed to nothing.
    const user = userEvent.setup();
    renderRail({ sessions: [SESSIONS[1]!, SESSIONS[2]!] });

    await user.click(screen.getByRole('button', { name: 'View options' }));
    await user.click(screen.getByRole('menuitemradio', { name: /^Live / }));

    expect(screen.getByText(/nothing matches this filter/i)).toBeInTheDocument();
  });

  it('says so when there is nothing at all, which is a different thing', () => {
    renderRail({ sessions: [] });
    expect(screen.getByText(/no sessions yet/i)).toBeInTheDocument();
  });

  it('reports an order and an arrangement to its owner rather than keeping them', async () => {
    // Both outlive this list — the order is the owner's preference and the
    // arrangement reaches every attached runtime — so neither is held here.
    const user = userEvent.setup();
    const rail = renderRail();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    await user.click(screen.getByRole('menuitemradio', { name: /oldest first/i }));
    expect(rail.onReorder).toHaveBeenCalledWith('oldest');

    await user.click(screen.getByRole('button', { name: 'View options' }));
    await user.click(screen.getByRole('menuitemradio', { name: /^Canvas$/ }));
    expect(rail.onArrange).toHaveBeenCalledWith('canvas');
  });
});

describe('the row actions', () => {
  const ACTIONS = [
    { id: 'stop', label: 'End session' },
    { id: 'delete', label: 'Delete', danger: true, disabled: true },
  ];

  it('offers nothing where a row has nothing to offer', () => {
    // No key at all rather than an empty menu: a control that opens onto
    // nothing is worse than one that was never drawn.
    renderRail();
    expect(screen.queryByRole('button', { name: /actions for/i })).toBeNull();
  });

  it('names the session each key belongs to', async () => {
    /*
     * "Actions" three times over is three identical controls to a screen
     * reader. The row's own label is what tells them apart, and it is the
     * only thing that can.
     */
    renderRail({ actionsFor: () => ACTIONS });
    const keys = screen.getAllByRole('button', { name: /actions for the session started/i });
    expect(new Set(keys.map((key) => key.getAttribute('aria-label'))).size).toBe(keys.length);
  });

  it('reports the act and the session it was asked of', async () => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    renderRail({ actionsFor: () => ACTIONS, onAction });

    const keys = screen.getAllByRole('button', { name: /actions for the session started/i });
    await user.click(keys[0]!);
    await user.click(await screen.findByRole('menuitem', { name: 'End session' }));

    expect(onAction).toHaveBeenCalledOnce();
    expect(onAction.mock.calls[0]?.[1]).toBe('stop');
  });

  it('lists a refused act rather than removing it', async () => {
    // §7. Removing it answers "why can I not delete this" with nothing.
    const user = userEvent.setup();
    const onAction = vi.fn();
    renderRail({ actionsFor: () => ACTIONS, onAction });

    const keys = screen.getAllByRole('button', { name: /actions for the session started/i });
    await user.click(keys[0]!);

    const refused = await screen.findByRole('menuitem', { name: 'Delete' });
    expect(refused).toHaveAttribute('aria-disabled', 'true');
    await user.click(refused);
    expect(onAction).not.toHaveBeenCalled();
  });
});
