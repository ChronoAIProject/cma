import { render, screen, waitFor } from '@testing-library/react';
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
    await screen.findByRole('button', { name: /new session/i });

    expect(screen.queryByRole('button', { name: 'Operations' })).toBeNull();
  });

  it('is offered to a caller the control plane says may operate', async () => {
    renderConsole(OPERATOR);

    expect(await screen.findByRole('button', { name: 'Operations' })).toBeInTheDocument();
  });

  it('swaps the middle of the page rather than opening over it', async () => {
    // §14: a page, not a dialog. A 560px dialog was why its rows truncated.
    const userEvents = userEvent.setup();
    const { container } = renderConsole(OPERATOR);
    await screen.findByRole('button', { name: 'Operations' });

    await userEvents.click(screen.getByRole('button', { name: 'Operations' }));

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

  it('says which view is showing, and goes back', async () => {
    // One row that is either showing or not. A separate way back would imply
    // the sidebar itself had gone somewhere.
    const userEvents = userEvent.setup();
    renderConsole(OPERATOR);
    const row = await screen.findByRole('button', { name: 'Operations' });

    await userEvents.click(row);
    expect(row).toHaveAttribute('aria-pressed', 'true');

    await userEvents.click(row);
    await waitFor(() => {
      expect(screen.queryByRole('region', { name: 'Operations' })).toBeNull();
    });
    expect(row).toHaveAttribute('aria-pressed', 'false');
  });

  it('does not persist, because a view is where somebody is', async () => {
    // The arrangement persists and this deliberately does not: reloading into
    // a surface nobody asked for is how a page frame starts behaving like a
    // route, and §1 says the frame is not navigation.
    const userEvents = userEvent.setup();
    const first = renderConsole(OPERATOR);
    await userEvents.click(await screen.findByRole('button', { name: 'Operations' }));
    await screen.findByRole('region', { name: 'Operations' });
    first.unmount();

    renderConsole(OPERATOR);
    await screen.findByRole('button', { name: 'Operations' });
    expect(screen.queryByRole('region', { name: 'Operations' })).toBeNull();
  });
});
