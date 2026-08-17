import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Console } from '../Console';
import type { Capability, User } from '../auth/types';

/**
 * Operations as a view rather than a dialog — docs/Web_Style.md §14 and §1.
 *
 * What matters here is the frame: the segment appears only for a caller the
 * control plane says may operate, choosing it swaps the middle of the page
 * rather than opening anything over it, and nothing about it is a route.
 */

const DAILY: readonly Capability[] = ['create_session', 'use_own_sessions'];
const OPERATOR: readonly Capability[] = [...DAILY, 'read_activity_log'];

function user(capabilities: readonly Capability[]): User {
  return { id: 'sub_test', handle: 'tester', capabilities };
}

function renderConsole(capabilities: readonly Capability[]) {
  return render(<Console user={user(capabilities)} onSignOut={() => {}} />);
}

describe('the operations view', () => {
  it('is not offered to a caller who holds nothing it can act on', async () => {
    renderConsole(DAILY);
    const views = await screen.findByRole('group', { name: 'View' });

    expect(within(views).queryByRole('button', { name: 'Operations' })).toBeNull();
    expect(within(views).getByRole('button', { name: 'Sessions' })).toBeInTheDocument();
  });

  it('is offered to a caller the control plane says may operate', async () => {
    renderConsole(OPERATOR);
    const views = await screen.findByRole('group', { name: 'View' });

    expect(within(views).getByRole('button', { name: 'Operations' })).toBeInTheDocument();
  });

  it('swaps the middle of the page rather than opening over it', async () => {
    // §14: a page, not a dialog. A 560px dialog was why its rows truncated.
    const userEvents = userEvent.setup();
    const { container } = renderConsole(OPERATOR);
    await screen.findByRole('group', { name: 'View' });

    await userEvents.click(screen.getByRole('button', { name: 'Operations' }));

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Operations' })).toBeInTheDocument();
    });
    // No plate over the page, and the sessions columns are gone rather than
    // sitting behind a scrim.
    expect(container.querySelector('.dialog__scrim')).toBeNull();
    expect(container.querySelector('.console__body')).toBeNull();
    // Still one bar and one band: the window's edges do not move.
    expect(container.querySelectorAll('.topbar')).toHaveLength(1);
    expect(container.querySelectorAll('.statusband')).toHaveLength(1);
  });

  it('says which view is showing, and goes back', async () => {
    const userEvents = userEvent.setup();
    renderConsole(OPERATOR);
    await screen.findByRole('group', { name: 'View' });

    await userEvents.click(screen.getByRole('button', { name: 'Operations' }));
    expect(screen.getByRole('button', { name: 'Operations' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );

    await userEvents.click(screen.getByRole('button', { name: 'Sessions' }));
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
    await screen.findByRole('group', { name: 'View' });
    await userEvents.click(screen.getByRole('button', { name: 'Operations' }));
    await screen.findByRole('region', { name: 'Operations' });
    first.unmount();

    renderConsole(OPERATOR);
    await screen.findByRole('group', { name: 'View' });
    expect(screen.queryByRole('region', { name: 'Operations' })).toBeNull();
  });
});
