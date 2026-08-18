import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SessionActions } from './SessionActions';
import { switchFor } from './planeSwitch';
import type { KernelOffer, Session, SessionLifecycle, SessionPlane } from './types';
import { NO_CAPABILITIES } from '../agui';

/**
 * Switching a session between planes — `Web_Style.md` §18.6.
 *
 * The move costs something real, so what these pin is the honesty around it:
 * it is offered only where it can work, it says what it costs before it is
 * asked for, and a session that cannot be moved right now says why rather than
 * disappearing.
 */

const BOTH: readonly KernelOffer[] = [
  { id: 'codex', modes: ['terminal', 'conversation'] },
  { id: 'claude-code', modes: ['conversation'] },
];

function session(
  lifecycle: SessionLifecycle = 'running',
  plane: SessionPlane = 'terminal',
  kernel: Session['kernel'] = 'codex',
): Session {
  return {
    id: 'sesn_1',
    kernel,
    lifecycle,
    createdAt: 0,
    plane,
    capabilities: NO_CAPABILITIES,
  };
}

function renderActions(
  s: Session = session(),
  kernels: readonly KernelOffer[] = BOTH,
  onChangePlane = vi.fn(),
) {
  render(
    <SessionActions
      session={s}
      kernels={kernels}
      onChangePlane={onChangePlane}
      onReconnect={vi.fn()}
      onRevive={vi.fn()}
      onStop={vi.fn()}
      onDetails={vi.fn()}
    />,
  );
  return { onChangePlane };
}

describe('what a session offers', () => {
  it('offers the plane it is not on', () => {
    expect(switchFor(session('running', 'terminal'), BOTH)?.to).toBe('conversation');
    expect(switchFor(session('running', 'conversation'), BOTH)?.to).toBe('terminal');
  });

  it('offers nothing where the kernel does not advertise the other plane', () => {
    // §18.6: absent, not disabled. A control that can never work is not one
    // that is currently unavailable.
    expect(switchFor(session('running', 'conversation', 'claude-code'), BOTH)).toBeUndefined();
  });

  it('offers nothing when the deployment’s runtimes are unknown', () => {
    // Guessing from an unread list would offer a move the control plane
    // refuses, which is worse than offering none.
    expect(switchFor(session(), [])).toBeUndefined();
  });

  it('treats a session that never said which plane it is on as a terminal', () => {
    const unstated: Session = { ...session(), plane: undefined };
    expect(switchFor(unstated, BOTH)?.to).toBe('conversation');
  });

  it('can be asked for only while there is a runtime to move', () => {
    for (const lifecycle of ['ready', 'running'] as const) {
      expect(switchFor(session(lifecycle), BOTH)?.available).toBe(true);
    }
    for (const lifecycle of ['provisioning', 'rotating', 'idle', 'terminated'] as const) {
      expect(switchFor(session(lifecycle), BOTH)?.available).toBe(false);
    }
  });
});

describe('the affordance in the session header', () => {
  it('names the plane it moves to, in the overflow menu', async () => {
    // §18.6: a key in the overflow menu naming the plane it moves to.
    const user = userEvent.setup();
    renderActions();
    await user.click(screen.getByRole('button', { name: /more actions/i }));
    expect(screen.getByRole('menuitem', { name: /conversation/i })).toBeInTheDocument();
  });

  it('is not drawn at all where the kernel cannot serve the other plane', async () => {
    const user = userEvent.setup();
    renderActions(session('running', 'conversation', 'claude-code'));
    await user.click(screen.getByRole('button', { name: /more actions/i }));
    expect(screen.queryByRole('menuitem', { name: /terminal/i })).toBeNull();
  });

  it('states both costs before it moves anything', () => {
    // §18.6: the two things that are true and are not obvious. Neither is
    // recoverable by pressing the key again, which is why they are said first.
    renderActions();
    expect(screen.queryByText(/turn in flight is lost/i)).toBeNull();
  });

  it('confirms inline, in place of the header’s own controls', async () => {
    const user = userEvent.setup();
    const { onChangePlane } = renderActions();

    await user.click(screen.getByRole('button', { name: /more actions/i }));
    await user.click(screen.getByRole('menuitem', { name: /conversation/i }));

    expect(screen.getByText(/turn in flight is lost/i)).toBeInTheDocument();
    expect(screen.getByText(/not interactive while it moves/i)).toBeInTheDocument();
    // In place of, not beside: §7 forbids a second dialog over a dialog, and
    // leaving the other keys reachable mid-question invites the wrong one.
    expect(screen.queryByRole('button', { name: /^details$/i })).toBeNull();
    expect(onChangePlane).not.toHaveBeenCalled();
  });

  it('moves only when the confirmation is answered', async () => {
    const user = userEvent.setup();
    const { onChangePlane } = renderActions();

    await user.click(screen.getByRole('button', { name: /more actions/i }));
    await user.click(screen.getByRole('menuitem', { name: /conversation/i }));
    await user.click(screen.getByRole('button', { name: /move to conversation/i }));

    expect(onChangePlane).toHaveBeenCalledWith('conversation');
  });

  it('lets somebody back out, changing nothing', async () => {
    const user = userEvent.setup();
    const { onChangePlane } = renderActions();

    await user.click(screen.getByRole('button', { name: /more actions/i }));
    await user.click(screen.getByRole('menuitem', { name: /conversation/i }));
    await user.click(screen.getByRole('button', { name: /stay here/i }));

    expect(onChangePlane).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: /^details$/i })).toBeInTheDocument();
  });

  it('says why it cannot be asked for right now, rather than vanishing', async () => {
    // The opposite case to a kernel that will never serve it: this one works
    // again, and a control that disappears and returns is one somebody hunts
    // for.
    const user = userEvent.setup();
    renderActions(session('rotating'));

    await user.click(screen.getByRole('button', { name: /more actions/i }));
    const key = screen.getByRole('menuitem', { name: /conversation/i });
    expect(key).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByText(/already being moved/i)).toBeInTheDocument();
  });

  it('offers nothing when the console cannot act on it', async () => {
    // No handler, no control. A key that reports a choice nothing acts on is
    // the worst kind of affordance.
    render(
      <SessionActions
        session={session()}
        kernels={BOTH}
        onReconnect={vi.fn()}
        onRevive={vi.fn()}
        onStop={vi.fn()}
        onDetails={vi.fn()}
      />,
    );
    await userEvent.setup().click(screen.getByRole('button', { name: /more actions/i }));
    expect(screen.queryByRole('menuitem', { name: /conversation/i })).toBeNull();
  });
});
