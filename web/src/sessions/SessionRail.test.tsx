import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SessionRail } from './SessionRail';
import { lastActivity } from './lastActivity';
import { tabLabel } from './sessionTime';
import type { Session, SessionLifecycle } from './types';
import { NO_CAPABILITIES } from '@chronoai/cma-sdk';

const COLLAPSED_KEY = 'cma:rail-collapsed';
const LEGACY_COLLAPSED_KEY = 'chrono-bot:rail-collapsed';

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

/** Open a group, since only Active starts open. */
async function expand(user: ReturnType<typeof userEvent.setup>, group: string) {
  const head = screen.getByRole('button', { name: new RegExp(`^${group}`, 'i') });
  if (head.getAttribute('aria-expanded') === 'false') {
    await user.click(head);
  }
}

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
  vi.restoreAllMocks();
});

describe('the rail row', () => {
  it('says what each session last did', async () => {
    // §12: the last-activity line is what makes a list of times into a list of
    // work. The finished one is in Terminated, which starts closed.
    const user = userEvent.setup();
    renderRail();
    expect(screen.getByText('Running')).toBeInTheDocument();
    expect(screen.getByText('Waiting for a sandbox')).toBeInTheDocument();

    await expand(user, 'Terminated');
    expect(screen.getByText('Ended after a failure')).toBeInTheDocument();
  });

  it('says a session waiting on its owner is waiting on them, not on a sandbox', () => {
    /*
     * #361. It is `provisioning` either way, so the row said the same thing
     * for a runtime being built and for a person who has to go and sign in
     * somewhere else — sending them to wait for a thing that was waiting for
     * them. Same precedent as the approval line above: say who it waits on.
     */
    renderRail({ signingIn: { b: true } });
    expect(screen.getByText('Waiting for you to sign in')).toBeInTheDocument();
    expect(screen.queryByText('Waiting for a sandbox')).toBeNull();
  });

  it('still names the sandbox step when nobody is being waited on', () => {
    // A stage outranks nothing here: without a sign-in the row is unchanged.
    renderRail({ stages: { b: 'runtime_created' }, signingIn: {} });
    expect(screen.getByText('Sandbox created')).toBeInTheDocument();
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

  it('says a session waiting on an answer is waiting, not stopped', async () => {
    // §18.3: the --wait dot and "waiting for your answer" in the rail. The
    // dot is already the wait tone for idle; the line is what changes.
    const user = userEvent.setup();
    const idle = { ...session('waiting', 'idle'), id: 'ses_waiting' };
    const { container } = renderRail({ sessions: [idle], waitingId: 'ses_waiting' });
    // Paused starts closed, and a session parked on an answer is `idle`.
    await expand(user, 'Paused');
    expect(container.textContent).toContain('Waiting for your answer');
    // The idle line — "Stopped — its sandbox was released" — is not drawn.
    expect(container.textContent).not.toContain('Stopped —');
  });

  it('reads requires_action directly before its approval event is folded', () => {
    const waiting = session('waiting', 'idle', { stopReason: 'requires_action' });
    expect(lastActivity(waiting)).toBe('Waiting for your answer');
  });

  it('never lets the activity line wrap', () => {
    // A row that grows with its longest sentence makes the list a different
    // height every time it reloads.
    const { container } = renderRail();
    expect(container.querySelector('.row-item__activity')).not.toBeNull();
  });
});

describe('the Chat rail', () => {
  const chats = [
    session('a', 'running', {
      chat: { worktreeName: 'chat-alpha', worktreePath: '/work/chat-alpha', title: 'Release plan' },
    }),
    session('b', 'idle', {
      stopReason: 'idle_timeout',
      chat: { worktreeName: 'chat-beta', worktreePath: '/work/chat-beta' },
    }),
    session('c', 'idle', {
      stopReason: 'end_turn',
      chat: { worktreeName: 'chat-gamma', worktreePath: '/work/chat-gamma' },
    }),
  ];

  it('has only Active and Paused groups and reserves Paused for inactivity timeout', async () => {
    const user = userEvent.setup();
    renderRail({ surface: 'chat', sessions: chats });

    expect(screen.getByText('Chats')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Active, 2 chats' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Paused, 1 chat' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Terminated/ })).toBeNull();

    await expand(user, 'Paused');
    expect(within(screen.getByRole('tablist', { name: 'Paused' })).getByText('chat-beta')).toBe(
      screen.getByText('chat-beta'),
    );
    expect(within(screen.getByRole('tablist', { name: 'Active' })).getByText('chat-gamma')).toBe(
      screen.getByText('chat-gamma'),
    );
  });

  it('prefers a generated title and falls back to the worktree name', () => {
    renderRail({ surface: 'chat', sessions: chats });

    expect(screen.getByText('Release plan')).toBeInTheDocument();
    expect(screen.getByText('chat-gamma')).toBeInTheDocument();
    expect(screen.queryByText('chat-alpha')).toBeNull();
  });

  it('keeps Agent labels time-based for sessions that happen to carry chat metadata', () => {
    const agent = chats[0] as Session;
    renderRail({ surface: 'agent', sessions: [agent] });

    expect(screen.getByText(tabLabel(agent.createdAt))).toBeInTheDocument();
    expect(screen.queryByText('Release plan')).toBeNull();
    expect(screen.getByRole('button', { name: 'Terminated, 0 sessions' })).toBeInTheDocument();
  });

  it('names an empty Chat collection without borrowing Agent copy', () => {
    renderRail({ surface: 'chat', sessions: [] });

    expect(screen.getByText('No chats yet.')).toBeInTheDocument();
    expect(screen.queryByText('No sessions yet.')).toBeNull();
  });
});

describe('the view options', () => {
  it('folds ordering and arranging onto one card', async () => {
    /*
     * They were affordances across two bands. They are answers to "what am I
     * looking at", and putting them on one card is what lets the header drop
     * to a single row — which is height the list gets.
     *
     * Filtering is not among them any more: the rail groups by state, and
     * collapsing a group is the same gesture a filter was.
     */
    const user = userEvent.setup();
    renderRail();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    const card = screen.getByRole('menu', { name: 'View options' });

    for (const label of [/newest first/i, /oldest first/i, /^List$/, /^Canvas$/]) {
      expect(within(card).getByRole('menuitemradio', { name: label })).toBeInTheDocument();
    }
  });

  it('marks the current answer in every group, not just one', async () => {
    // One selection could only mark one of the questions on the card.
    const user = userEvent.setup();
    renderRail({ order: 'oldest', arrangement: 'canvas' });

    await user.click(screen.getByRole('button', { name: 'View options' }));

    expect(screen.getByRole('menuitemradio', { name: /oldest first/i })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('menuitemradio', { name: /^Canvas$/ })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('carries no filter, because the groups are the filter', async () => {
    /*
     * Choosing "Stopped" and collapsing everything but Paused are the same
     * gesture done twice, and keeping both let the rail hold two states that
     * disagreed — a group expanded inside a filter that excluded it.
     */
    const user = userEvent.setup();
    renderRail();

    await user.click(screen.getByRole('button', { name: 'View options' }));
    const card = screen.getByRole('menu', { name: 'View options' });

    expect(within(card).queryByRole('menuitemradio', { name: /^All / })).toBeNull();
    expect(within(card).queryByRole('menuitemradio', { name: /^Live / })).toBeNull();
    expect(within(card).queryByRole('menuitemradio', { name: /^Stopped / })).toBeNull();
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
    { id: 'stop', label: 'Pause session' },
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
    await user.click(await screen.findByRole('menuitem', { name: 'Pause session' }));

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

/*
 * Grouping by state — #381.
 *
 * The rail answers "which are running, which are set aside, which are
 * finished" without being asked. A filter answered it by hiding the rest.
 */
describe('the groups', () => {
  const MIXED = [
    session('a', 'running'),
    session('b', 'idle'),
    session('c', 'terminated'),
    session('d', 'ready'),
  ];

  it('opens Active and closes the rest', () => {
    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    for (const shut of ['Paused', 'Terminated']) {
      expect(screen.getByRole('button', { name: new RegExp(`^${shut}`) })).toHaveAttribute(
        'aria-expanded',
        'false',
      );
    }
    // Two active sessions are listed; the closed groups list nothing.
    expect(screen.getAllByRole('tab')).toHaveLength(2);
  });

  it('says how many are inside a group that is closed', () => {
    // A count on the header is the answer to "where did it go" without
    // opening anything.
    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: 'Paused, 1 session' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Terminated, 1 session' })).toBeInTheDocument();
  });

  it('draws an empty group rather than dropping it', () => {
    // A group that disappears is one somebody looks for.
    renderRail({ sessions: [session('a', 'running')] });

    expect(screen.getByRole('button', { name: 'Paused, 0 sessions' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Terminated, 0 sessions' })).toBeInTheDocument();
  });

  it('opens and closes a group when its header is pressed', async () => {
    const user = userEvent.setup();
    renderRail({ sessions: MIXED });

    await user.click(screen.getByRole('button', { name: 'Paused, 1 session' }));
    expect(screen.getAllByRole('tab')).toHaveLength(3);

    await user.click(screen.getByRole('button', { name: 'Paused, 1 session' }));
    expect(screen.getAllByRole('tab')).toHaveLength(2);
  });

  it('remembers which groups were closed under the canonical key', async () => {
    // A preference about how a list is drawn, like the arrangement beside it.
    const user = userEvent.setup();
    const first = renderRail({ sessions: MIXED });
    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    await user.click(screen.getByRole('button', { name: /^Active/ }));
    expect(JSON.parse(window.localStorage.getItem(COLLAPSED_KEY) ?? '[]')).toEqual([
      'paused',
      'terminated',
      'active',
    ]);
    expect(window.localStorage.getItem(LEGACY_COLLAPSED_KEY)).toBeNull();
    first.unmount();

    renderRail({ sessions: MIXED });
    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });

  it('migrates historical collapsed groups', () => {
    window.localStorage.setItem(LEGACY_COLLAPSED_KEY, JSON.stringify(['active']));

    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    expect(window.localStorage.getItem(COLLAPSED_KEY)).toBe(JSON.stringify(['active']));
  });

  it('prefers valid canonical collapsed groups when both keys exist', () => {
    window.localStorage.setItem(COLLAPSED_KEY, JSON.stringify(['paused']));
    window.localStorage.setItem(LEGACY_COLLAPSED_KEY, JSON.stringify(['active']));

    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    expect(screen.getByRole('button', { name: /^Paused/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });

  it('falls back to historical collapsed groups when canonical data is malformed', () => {
    window.localStorage.setItem(COLLAPSED_KEY, '{"nope":true}');
    window.localStorage.setItem(LEGACY_COLLAPSED_KEY, JSON.stringify(['active']));

    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    expect(window.localStorage.getItem(COLLAPSED_KEY)).toBe(JSON.stringify(['active']));
  });

  it('ignores a stored value this build does not recognise', () => {
    // Web storage is untrusted: another script on the origin, or a build
    // somebody has since rolled back from, may have written it.
    window.localStorage.setItem(COLLAPSED_KEY, '{"nope":true}');
    window.localStorage.setItem(LEGACY_COLLAPSED_KEY, 'not json');
    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    expect(screen.getByRole('button', { name: /^Paused/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });

  it('uses defaults instead of crashing when storage reads are blocked', () => {
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });

    renderRail({ sessions: MIXED });

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
  });

  it('still toggles a group when storage writes are blocked', async () => {
    const user = userEvent.setup();
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });
    renderRail({ sessions: MIXED });

    await user.click(screen.getByRole('button', { name: /^Active/ }));

    expect(screen.getByRole('button', { name: /^Active/ })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });

  it('orders within a group by the one control that sets it', async () => {
    // Sorted once across the rail, then split: three orders would be three
    // things that can disagree with the single control that chooses them.
    const user = userEvent.setup();
    renderRail({ sessions: MIXED, order: 'oldest' });
    await expand(user, 'Active');

    const times = screen.getAllByRole('tab').map((tab) => tab.textContent ?? '');
    expect(times).toHaveLength(2);
  });
});

/*
 * While the list is being read — #375.
 *
 * "No sessions yet." is an answer. Before the read returns there is not one,
 * and saying it anyway is not a slower version of the truth but a different
 * claim — one that sends somebody to start a session they may already have.
 */
describe('while the list is being read', () => {
  it('does not claim there are none', () => {
    renderRail({ sessions: [], loading: true });

    expect(screen.queryByText(/no sessions yet/i)).toBeNull();
  });

  it('draws the shape of what is coming', () => {
    // Rows where rows will be, rather than a sentence in an empty column.
    const { container } = renderRail({ sessions: [], loading: true });

    expect(screen.getByLabelText('Reading your sessions')).toHaveAttribute('aria-busy', 'true');
    expect(container.querySelectorAll('.rail__ghost').length).toBeGreaterThan(0);
  });

  it('says there are none once it knows', () => {
    renderRail({ sessions: [], loading: false });

    expect(screen.getByText(/no sessions yet/i)).toBeInTheDocument();
    expect(screen.queryByLabelText('Reading your sessions')).toBeNull();
  });

  it('draws no groups until it has something to group', () => {
    // Three empty groups over a list that has not arrived says the answer is
    // "nothing", which is the claim this state exists to avoid making.
    renderRail({ sessions: [], loading: true });

    expect(screen.queryByRole('button', { name: /^Active/ })).toBeNull();
  });
});
