import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Composer } from './Composer';
import { toCapabilities, type ConcurrentInput, type SessionCapabilities } from '../agui';

/**
 * The composer, and interrupt — `Web_Style.md` §18.4.
 */

function capabilities(overrides: Partial<SessionCapabilities> = {}): SessionCapabilities {
  return {
    ...toCapabilities({ conforms_to: ['…/conf/conversation'], concurrent_input: 'reject' }),
    ...overrides,
  };
}

function renderComposer(props: Partial<Parameters<typeof Composer>[0]> = {}) {
  const onSend = vi.fn();
  const onInterrupt = vi.fn();
  const result = render(
    <Composer
      capabilities={capabilities()}
      turnInFlight={false}
      onSend={onSend}
      onInterrupt={onInterrupt}
      {...props}
    />,
  );
  return { ...result, onSend, onInterrupt };
}

describe('sending', () => {
  it('sends on Enter and opens a line on Shift+Enter', async () => {
    const user = userEvent.setup();
    const { onSend } = renderComposer();

    await user.type(screen.getByLabelText('Message'), 'refactor auth');
    await user.keyboard('{Enter}');
    expect(onSend).toHaveBeenCalledWith('refactor auth');

    await user.type(screen.getByLabelText('Message'), 'one{Shift>}{Enter}{/Shift}two');
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it('will not send an empty prompt', () => {
    renderComposer();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  });
});

describe('what it does mid-turn', () => {
  it.each([
    ['reject', 'A turn is in progress. Wait for it to finish.', false],
    ['enqueue', 'This will be sent after the turn in progress.', true],
    ['interrupt', 'Sending this will stop the turn in progress.', true],
    ['rollback', 'Sending this will discard the turn in progress.', true],
  ] as const)('follows the %s policy', async (policy: ConcurrentInput, says, sendable) => {
    // §18.4 and `API_Overview.md` §6 rule 5: the policy is chosen per session
    // at creation and reported in its capability document. The composer reads
    // it; it does not invent one and it does not disable itself on a guess.
    const user = userEvent.setup();
    const { onSend } = renderComposer({
      capabilities: capabilities({ concurrentInput: policy }),
      turnInFlight: true,
    });

    // The sentence is on the surface, not in a tooltip (§7).
    expect(screen.getByText(says)).toBeInTheDocument();

    await user.type(screen.getByLabelText('Message'), 'and also this');
    await user.keyboard('{Enter}');

    if (sendable) {
      expect(onSend).toHaveBeenCalledWith('and also this');
    } else {
      expect(onSend).not.toHaveBeenCalled();
      expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    }
  });

  it('accepts text even on the policy that will not send it', async () => {
    // §18.4: on `reject` the composer "accepts text, disables the send key,
    // and says a turn is in progress". Refusing the keystrokes as well would
    // lose what somebody typed while they waited.
    const user = userEvent.setup();
    renderComposer({
      capabilities: capabilities({ concurrentInput: 'reject' }),
      turnInFlight: true,
    });

    await user.type(screen.getByLabelText('Message'), 'queued in my head');
    expect(screen.getByLabelText('Message')).toHaveValue('queued in my head');
  });
});

describe('steering', () => {
  it('shows no affordance at all where it was not granted', () => {
    // §18.4: not a disabled one. A control that can never work is not a
    // control that is currently unavailable — there is nothing about the
    // session's state to explain and nothing that would ever enable it.
    renderComposer({ capabilities: capabilities({ steer: false }), turnInFlight: true });
    expect(screen.queryByText(/steer/i)).toBeNull();
  });

  it('shows it where it was granted', () => {
    renderComposer({ capabilities: capabilities({ steer: true }), turnInFlight: true });
    expect(screen.getByText(/can be steered/i)).toBeInTheDocument();
  });
});

describe('interrupt', () => {
  it('is offered only while a turn is in flight', () => {
    const granted = capabilities({ interrupt: true });
    const idle = renderComposer({ capabilities: granted, turnInFlight: false });
    expect(idle.queryByRole('button', { name: 'Interrupt' })).toBeNull();
    idle.unmount();

    renderComposer({ capabilities: granted, turnInFlight: true });
    expect(screen.getByRole('button', { name: 'Interrupt' })).toBeInTheDocument();
  });

  it('is absent on a session whose kernel cannot withdraw a prompt', () => {
    // Refused by name rather than left waiting for something that will not
    // happen.
    renderComposer({ capabilities: capabilities({ interrupt: false }), turnInFlight: true });
    expect(screen.queryByRole('button', { name: 'Interrupt' })).toBeNull();
  });

  it('stops the turn, and the surface reflects it without a reload', async () => {
    const user = userEvent.setup();
    const { onInterrupt, rerender } = renderComposer({
      capabilities: capabilities({ interrupt: true }),
      turnInFlight: true,
    });

    await user.click(screen.getByRole('button', { name: 'Interrupt' }));
    expect(onInterrupt).toHaveBeenCalledTimes(1);

    // The turn ends; the control goes with it, from state rather than a reload.
    rerender(
      <Composer
        capabilities={capabilities({ interrupt: true })}
        turnInFlight={false}
        onSend={vi.fn()}
        onInterrupt={onInterrupt}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Interrupt' })).toBeNull();
  });
});

describe('a session that cannot take input', () => {
  it('refuses and says why, rather than being inert', () => {
    // §11 and §18.4. A composer that has quietly stopped working is the thing
    // people report as broken.
    renderComposer({ refusedReason: 'This session is moving to an updated runtime.' });

    expect(screen.getByLabelText('Message')).toBeDisabled();
    expect(screen.getByText('This session is moving to an updated runtime.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  });

  it('never sends while it is refusing', async () => {
    const user = userEvent.setup();
    const { onSend } = renderComposer({ refusedReason: 'This session has ended.' });
    await user.click(screen.getByRole('button', { name: 'Send' }));
    expect(onSend).not.toHaveBeenCalled();
  });
});
