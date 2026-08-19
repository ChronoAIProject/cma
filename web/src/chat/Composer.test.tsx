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
  const onSend = vi.fn().mockResolvedValue(true);
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

  it('shows it where it was granted, as a tag on the composer row', () => {
    renderComposer({ capabilities: capabilities({ steer: true }), turnInFlight: true });
    expect(screen.getByText('steerable')).toBeInTheDocument();
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
    // §11 and §18.4: a dashed-border card carrying the reason — never a
    // quietly dead field, which is the thing people report as broken.
    const refused = renderComposer({
      refusedReason: 'This session is moving to an updated runtime.',
    });

    expect(refused.container.querySelector('.composer--refused')).not.toBeNull();
    expect(screen.getByText('This session is moving to an updated runtime.')).toBeInTheDocument();
  });

  it('offers nothing to send while it is refusing', () => {
    // The refusal card replaces the field and the keys entirely: a Send key
    // beside a refusal would be a control that can never work (§18.4).
    renderComposer({ refusedReason: 'This session has ended.' });
    expect(screen.queryByRole('button', { name: /send/i })).toBeNull();
    expect(screen.queryByLabelText('Message')).toBeNull();
  });
});

describe('a refused turn keeps what was written', () => {
  /*
   * The failure this guards is the expensive one: on a long run the message
   * that gets refused is the carefully worded correction, and clearing the
   * field means retyping it from memory.
   */
  it('leaves the draft alone when the service did not take it', async () => {
    const user = userEvent.setup();
    const onSend = vi.fn().mockResolvedValue(false);
    renderComposer({ onSend });

    await user.type(screen.getByRole('textbox'), 'the careful correction');
    await user.keyboard('{Enter}');

    expect(onSend).toHaveBeenCalledWith('the careful correction');
    expect(screen.getByRole('textbox')).toHaveValue('the careful correction');
  });

  it('leaves the draft alone when the send threw', async () => {
    const user = userEvent.setup();
    renderComposer({ onSend: vi.fn().mockRejectedValue(new Error('the network went')) });

    await user.type(screen.getByRole('textbox'), 'still here');
    await user.keyboard('{Enter}');

    expect(screen.getByRole('textbox')).toHaveValue('still here');
  });

  it('clears the draft once the turn is accepted', async () => {
    // The other half: keeping it after a successful send would put the last
    // message back into the next one.
    const user = userEvent.setup();
    renderComposer({ onSend: vi.fn().mockResolvedValue(true) });

    await user.type(screen.getByRole('textbox'), 'taken');
    await user.keyboard('{Enter}');

    expect(screen.getByRole('textbox')).toHaveValue('');
  });

  it('sends nothing when there is nothing but whitespace', async () => {
    const user = userEvent.setup();
    const { onSend } = renderComposer();

    await user.type(screen.getByRole('textbox'), '   ');
    await user.keyboard('{Enter}');

    expect(onSend).not.toHaveBeenCalled();
  });
});

describe('the field grows with what is written in it', () => {
  /*
   * §18.4: "The field grows to six lines and then scrolls inside itself." It
   * was `rows={1}` with no growth, so a multi-paragraph prompt scrolled inside
   * a two-line porthole — somebody writing the careful instruction a long run
   * turns on could see two lines of it.
   *
   * jsdom lays nothing out, so `scrollHeight` is stubbed: what is under test
   * is that the field is measured and its height set from the measurement,
   * which is the part that was missing.
   */
  function measured(field: HTMLTextAreaElement, height: number): void {
    Object.defineProperty(field, 'scrollHeight', { configurable: true, get: () => height });
  }

  it('sets its height from what it measures', async () => {
    const user = userEvent.setup();
    renderComposer();
    const field = screen.getByRole('textbox') as HTMLTextAreaElement;
    measured(field, 140);

    await user.type(field, 'a line');

    expect(field.style.height).toBe('140px');
  });

  it('shrinks again when the draft is cut down', async () => {
    // Reset to `auto` before measuring, or `scrollHeight` of an already-tall
    // box never reports anything smaller and the field only ever grows.
    const user = userEvent.setup();
    renderComposer();
    const field = screen.getByRole('textbox') as HTMLTextAreaElement;

    measured(field, 200);
    await user.type(field, 'lots');
    expect(field.style.height).toBe('200px');

    measured(field, 64);
    await user.clear(field);
    expect(field.style.height).toBe('64px');
  });
});

describe('the commands the kernel offers', () => {
  /*
   * §18.4. They used to be printed as a static list at the foot of the message
   * column, below every turn: readable and unusable, so the reader retyped
   * what they had just read.
   */
  const OFFERED = [
    { name: 'compact', description: 'Summarise the conversation so far' },
    { name: 'clear', description: 'Start again with nothing carried over' },
  ];

  it('opens on a slash at the head of an empty draft', async () => {
    const user = userEvent.setup();
    renderComposer({ availableCommands: OFFERED });

    await user.type(screen.getByRole('textbox'), '/');

    expect(screen.getByRole('listbox', { name: /commands/i })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: /compact/ })).toBeInTheDocument();
  });

  it('offers only what the kernel advertised', async () => {
    // §18.2 and §18.10, in identical words: a client that invented a command
    // would be offering something the session cannot honour.
    const user = userEvent.setup();
    renderComposer({ availableCommands: OFFERED });

    await user.type(screen.getByRole('textbox'), '/');
    expect(screen.getAllByRole('option')).toHaveLength(2);
  });

  it('filters as the reader keeps typing', async () => {
    const user = userEvent.setup();
    renderComposer({ availableCommands: OFFERED });

    await user.type(screen.getByRole('textbox'), '/com');

    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option', { name: /compact/ })).toBeInTheDocument();
  });

  it('does not open on a slash inside a sentence', async () => {
    // An agent is asked to do things in prose, and a `/` mid-sentence is a
    // path or a date rather than a command.
    const user = userEvent.setup();
    renderComposer({ availableCommands: OFFERED });

    await user.type(screen.getByRole('textbox'), 'look in src/chat');

    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('opens nothing when the kernel advertised none', async () => {
    // The same reasoning that keeps an ungranted affordance off the surface
    // rather than drawing it empty.
    const user = userEvent.setup();
    renderComposer({ availableCommands: [] });

    await user.type(screen.getByRole('textbox'), '/');

    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('takes a command and leaves the caret after it', async () => {
    const user = userEvent.setup();
    renderComposer({ availableCommands: OFFERED });
    const field = screen.getByRole('textbox');

    await user.type(field, '/com');
    await user.keyboard('{Enter}');

    expect(field).toHaveValue('/compact ');
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('does not send the turn while the menu is choosing', async () => {
    // The field is listening for Enter too. The menu takes it first, or
    // choosing a command would send one instead.
    const user = userEvent.setup();
    const onSend = vi.fn().mockResolvedValue(true);
    renderComposer({ availableCommands: OFFERED, onSend });

    await user.type(screen.getByRole('textbox'), '/com');
    await user.keyboard('{Enter}');

    expect(onSend).not.toHaveBeenCalled();
  });

  it('closes on Escape and stays closed for that draft', async () => {
    const user = userEvent.setup();
    renderComposer({ availableCommands: OFFERED });

    await user.type(screen.getByRole('textbox'), '/');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).toBeNull();

    // Still closed as they keep typing the same command.
    await user.type(screen.getByRole('textbox'), 'com');
    expect(screen.queryByRole('listbox')).toBeNull();
  });
});
