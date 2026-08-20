import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Console } from '../Console';
import type { Capability, User } from '../auth/types';

/**
 * Operations as a view rather than a dialog — docs/Web_Style.md §14 and §1.
 *
 * What matters here is the frame: the entry appears only for a caller the
 * control plane says may operate, choosing it swaps the middle of the page
 * rather than opening anything over it, and nothing about it is a route.
 *
 * It is reached from the account menu. It was a segment on the app bar until
 * §9 retired the bar, then a row of its own in the sidebar foot — which made
 * the foot a two-item navigation strip beside an account control that omitted
 * the one place an operator goes.
 */

const DAILY: readonly Capability[] = ['create_session', 'use_own_sessions'];
const OPERATOR: readonly Capability[] = [...DAILY, 'read_activity_log'];

function user(capabilities: readonly Capability[]): User {
  return { id: 'sub_test', handle: 'tester', capabilities };
}

function renderConsole(capabilities: readonly Capability[]) {
  return render(<Console user={user(capabilities)} onSignOut={() => {}} />);
}

/** Open the account menu, where the entry lives. */
async function openAccount(userEvents: ReturnType<typeof userEvent.setup>) {
  await userEvents.click(await screen.findByRole('button', { name: /account menu/i }));
}

/** The entry, by the name it carries for the view currently showing. */
function operationsEntry(name: 'Operations' | 'Back to sessions' = 'Operations') {
  return screen.getByRole('menuitem', { name });
}

describe('the operations view', () => {
  it('is not offered to a caller who holds nothing it can act on', async () => {
    const userEvents = userEvent.setup();
    renderConsole(DAILY);
    await screen.findByRole('button', { name: /new session/i });
    await openAccount(userEvents);

    expect(screen.queryByRole('menuitem', { name: /operations/i })).toBeNull();
  });

  it('is offered to a caller the control plane says may operate', async () => {
    const userEvents = userEvent.setup();
    renderConsole(OPERATOR);
    await openAccount(userEvents);

    expect(operationsEntry()).toBeInTheDocument();
  });

  it('swaps the middle of the page rather than opening over it', async () => {
    // §14: a page, not a dialog. A 560px dialog was why its rows truncated.
    const userEvents = userEvent.setup();
    const { container } = renderConsole(OPERATOR);
    await openAccount(userEvents);

    await userEvents.click(operationsEntry());

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Operations' })).toBeInTheDocument();
    });
    // No plate over the page, and the session surface is gone rather than
    // sitting behind a mask.
    expect(container.querySelector('.modal__mask')).toBeNull();
    expect(container.querySelector('.console__surface')).toBeNull();
    // One frame, and the sidebar has not moved: the window's edges hold.
    expect(container.querySelectorAll('.frame')).toHaveLength(1);
    expect(container.querySelectorAll('.sidebar')).toHaveLength(1);
  });

  it('names where it goes, and goes back', async () => {
    /*
     * One entry that names its destination rather than marking itself as
     * current. A check mark would make it a value among alternatives — the
     * shared menu gives every row `menuitemradio` the moment a selection is
     * passed — and these rows are actions.
     */
    const userEvents = userEvent.setup();
    renderConsole(OPERATOR);
    await openAccount(userEvents);

    await userEvents.click(operationsEntry());
    await screen.findByRole('region', { name: 'Operations' });

    await openAccount(userEvents);
    await userEvents.click(operationsEntry('Back to sessions'));
    await waitFor(() => {
      expect(screen.queryByRole('region', { name: 'Operations' })).toBeNull();
    });
  });

  it('does not persist, because a view is where somebody is', async () => {
    // The arrangement persists and this deliberately does not: reloading into
    // a surface nobody asked for is how a page frame starts behaving like a
    // route, and §1 says the frame is not navigation.
    const userEvents = userEvent.setup();
    const first = renderConsole(OPERATOR);
    await openAccount(userEvents);
    await userEvents.click(operationsEntry());
    await screen.findByRole('region', { name: 'Operations' });
    first.unmount();

    renderConsole(OPERATOR);
    await screen.findByRole('button', { name: /account menu/i });
    expect(screen.queryByRole('region', { name: 'Operations' })).toBeNull();
  });
});
