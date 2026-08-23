import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ApprovalCard } from './ApprovalCard';
import { MessageStream } from './MessageStream';
import { fold } from './conversation';
import {
  AguiConnection,
  AguiOperations,
  MockSession,
  transcriptNamed,
  type AguiEvent,
} from '../agui';

/**
 * The approval card — `Web_Style.md` §18.3.
 */

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/**
 * Drive a transcript, optionally cutting the stream, and fold the result.
 *
 * `upTo` stops the transcript at a frame, which is how a test looks at a
 * session that is *parked* on an approval rather than one whose transcript
 * runs past the answer.
 */
async function conversationFrom(name: string, options: { dropAfter?: number; upTo?: number } = {}) {
  const full = transcriptNamed(name);
  const transcript =
    options.upTo === undefined ? full : { ...full, frames: full.frames.slice(0, options.upTo) };
  const session = new MockSession(transcript, { dropAfter: options.dropAfter });
  const seen: AguiEvent[] = [];
  const connection = new AguiConnection(session.transport, 'sesn_1', {
    backoffMs: 0,
    sleep: () => Promise.resolve(),
  });
  const stop = connection.subscribe((event) => seen.push(event));
  for (let i = 0; i < 30; i += 1) {
    await tick();
  }
  stop();
  return { session, conversation: fold(seen) };
}

const THREE_ANSWERS = {
  id: 'apr_7Q2',
  reason: 'chrono:command_execution',
  message: 'Run `rm -rf build/`?',
  options: [
    { decision: 'allow', label: 'Allow', affirmative: true },
    { decision: 'deny', label: 'Deny' },
    { decision: 'abort', label: 'Stop the turn' },
  ],
};

describe('the card', () => {
  it('renders what arrived, not a hardcoded pair', async () => {
    // §18.3: "The options come from the kernel and are not enumerated here.
    // Render what arrives, never a hardcoded allow/deny pair." A kernel
    // offering three renders three.
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />);

    expect(screen.getByRole('button', { name: 'Allow' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Deny' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop the turn' })).toBeInTheDocument();
  });

  it('carries the kernel wording verbatim', () => {
    // Rewording it into a category loses the one detail the decision turns on.
    // Every word survives; only the backticks become a frame around the
    // command, which is §18.3's "a command inside it takes inline code".
    const { container } = render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />);
    const question = container.querySelector('.approval__question');
    expect(question?.textContent).toBe('Run rm -rf build/?');
    // The reason rides the eyebrow — approval · <the kernel's own reason>.
    expect(screen.getByText(/chrono:command_execution/)).toBeInTheDocument();
  });

  it('draws the command as code rather than as punctuation', () => {
    // §18.3 requires it, and this is the one sentence in the product where the
    // exact command is the whole decision. It was rendering its backticks.
    const { container } = render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />);
    const code = container.querySelector('.approval__question code');
    expect(code?.textContent).toBe('rm -rf build/');
  });

  it('says when the question was asked, and stays quiet when it cannot', () => {
    // §18.3 requires the time in the eyebrow. On a session picked up later it
    // is the difference between "the agent is waiting on me" and "the agent
    // has been waiting since this morning".
    const asked = { ...THREE_ANSWERS, askedAt: '2026-08-19T09:14:00Z' };
    const { container } = render(<ApprovalCard interrupt={asked} onAnswer={vi.fn()} />);
    expect(container.querySelector('.approval__asked')?.textContent).toMatch(/\d{1,2}[:.]\d{2}/);

    // A log written before timestamps draws no time rather than a wrong one.
    const { container: without } = render(
      <ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />,
    );
    expect(without.querySelector('.approval__asked')).toBeNull();
  });

  it('spends the surface primary key on the affirmative answer', () => {
    const { container } = render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />);
    const primary = container.querySelectorAll('.button--primary');
    expect(primary).toHaveLength(1);
    expect(primary[0]?.textContent).toContain('Allow');
  });

  it('falls back to the protocol answers, never to a set it invented', () => {
    // §4: a decision this service does not recognise is read as a refusal, so
    // inventing one would be inventing a refusal.
    render(
      <ApprovalCard
        interrupt={{ id: 'a', reason: 'confirmation', message: 'May I?' }}
        onAnswer={vi.fn()}
      />,
    );
    expect(screen.getAllByRole('button').map((b) => b.textContent)).toEqual(['Allow', 'Deny']);
  });

  it('answers by the decision the kernel named', async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={onAnswer} />);

    await user.click(screen.getByRole('button', { name: 'Stop the turn' }));
    expect(onAnswer).toHaveBeenCalledWith('abort');
  });
});

describe('it is not a modal', () => {
  it('opens no scrim and traps no focus', () => {
    // §18.3, binding: a session can be parked on an approval while its owner
    // reads something else, and a modal makes every other session in the rail
    // unreachable until they answer.
    const { container } = render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />);

    expect(container.querySelector('.dialog__scrim')).toBeNull();
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(container.querySelector('[aria-modal]')).toBeNull();
  });

  it('leaves the rest of the interface reachable', async () => {
    // Something else on the page is still clickable while one is pending.
    const user = userEvent.setup();
    const elsewhere = vi.fn();
    render(
      <div>
        <ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />
        <button type="button" onClick={elsewhere}>
          Another session
        </button>
      </div>,
    );

    await user.click(screen.getByRole('button', { name: 'Another session' }));
    expect(elsewhere).toHaveBeenCalled();
  });

  it('is a card rather than a plate', () => {
    /*
     * §18.3's binding constraint: never a modal. A plate floats over the
     * page and makes every other session unreachable while one waits; this
     * takes the composer's seat inside one session's own surface, so it
     * neither floats nor stacks.
     */
    const css = readFileSync(resolve(process.cwd(), 'src/chat/ApprovalCard.css'), 'utf8');
    const rule = /\.approval\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).not.toContain('position: fixed');
    expect(rule).not.toContain('position: absolute');
    expect(rule).not.toContain('z-index');
  });

  it('warns rather than informs', () => {
    // Blue reads as information. This is a decision with a cost.
    const css = readFileSync(resolve(process.cwd(), 'src/chat/ApprovalCard.css'), 'utf8');
    const rule = /\.approval\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).toMatch(/border: 1px solid var\(--dsw-alias-state-warn/);
  });
});

describe('it survives a reconnect', () => {
  it('is still there, and still answerable, after the stream died', async () => {
    // §18.3: "approvals ride the event stream, so they inherit replay — a
    // client that disconnects mid-approval finds the request waiting on
    // reconnect, and this component must prove that rather than assume it."
    // Parked on the approval — the run ended to ask and nothing has answered
    // it — and the stream was cut before the client saw the whole of it.
    const dropped = await conversationFrom('approval-granted', { upTo: 2, dropAfter: 1 });

    const pending = dropped.conversation.pending;
    expect(pending, 'the approval is still pending after a reconnect').toBeDefined();
    expect(pending?.id).toBe('apr_7Q2');

    const onAnswer = vi.fn();
    render(<ApprovalCard interrupt={pending!} onAnswer={onAnswer} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Allow' }));
    expect(onAnswer).toHaveBeenCalledWith('allow');
  });

  it('is drawn from history rather than from having held a connection', async () => {
    const live = await conversationFrom('approval-granted', { upTo: 2 });
    const replayed = await conversationFrom('approval-granted', { upTo: 2, dropAfter: 1 });
    expect(live.conversation.pending).toEqual(replayed.conversation.pending);
  });

  it('clears when the answer resumes the session', async () => {
    const { conversation } = await conversationFrom('approval-granted');
    // The transcript runs past the answer, so by the end nothing is pending.
    expect(conversation.pending).toBeUndefined();
  });
});

describe('in the stream', () => {
  it('sits where the turn stopped', async () => {
    const { conversation } = await conversationFrom('approval-granted', { upTo: 2 });
    const { container } = render(
      <MessageStream
        entries={conversation.entries}
        footer={
          conversation.pending === undefined ? null : (
            <ApprovalCard interrupt={conversation.pending} onAnswer={vi.fn()} />
          )
        }
      />,
    );
    expect(container.querySelector('.stream__surface .approval')).not.toBeNull();
  });

  it('sends the decision through the client unchanged', async () => {
    const { session, conversation } = await conversationFrom('approval-granted', { upTo: 2 });
    const operations = new AguiOperations('sesn_1', session.operations);

    await operations.resolveApproval(conversation.pending?.id ?? '', 'abort', 'run_9');

    expect(session.sent[0]?.body).toMatchObject({
      resume: [{ interruptId: 'apr_7Q2', payload: { decision: 'abort' } }],
    });
  });
});

/*
 * Answering, before the log catches up — #373.
 *
 * The card is cleared by the durable event, which is a whole lap of the stream
 * after the request returns. Until it does, what the reader sees is this
 * card's own doing, and it used to be nothing: the buttons greyed, ungreyed
 * when the request resolved, and the card sat unchanged. That reads as a click
 * that did not register, on the one interaction where somebody is definitely
 * watching.
 */
describe('answering', () => {
  it('shows the answer at once, without waiting for the service', async () => {
    const user = userEvent.setup();
    // Never settles: the point is what is drawn before it does.
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={() => new Promise(() => {})} />);

    await user.click(screen.getByRole('button', { name: 'Allow' }));

    expect(screen.getByRole('button', { name: 'Allow' })).toBeInTheDocument();
    expect(screen.getByText(/sending your answer/i)).toBeInTheDocument();
  });

  it('withdraws the answers not given', async () => {
    // Not disabled: they are not unavailable, they are no longer the question,
    // and a row of greyed alternatives beside the answer invites another press.
    const user = userEvent.setup();
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={() => new Promise(() => {})} />);

    await user.click(screen.getByRole('button', { name: 'Deny' }));

    expect(screen.queryByRole('button', { name: 'Allow' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Stop the turn' })).toBeNull();
  });

  it('sends once however many times it is pressed', async () => {
    /*
     * The window this closes was real: `busy` went false when the request
     * resolved, while the card stayed up until the event arrived — so the
     * answers were live again on a question that had already been answered.
     */
    const user = userEvent.setup();
    const onAnswer = vi.fn(() => Promise.resolve(true));
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={onAnswer} />);

    const allow = screen.getByRole('button', { name: 'Allow' });
    await user.click(allow);
    await user.click(allow);
    await user.click(allow);

    expect(onAnswer).toHaveBeenCalledOnce();
  });

  it('puts the question back when the service refuses it', async () => {
    // Otherwise the reader is left looking at a decision nothing received.
    const user = userEvent.setup();
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={() => Promise.resolve(false)} />);

    await user.click(screen.getByRole('button', { name: 'Allow' }));

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Deny' })).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: 'Stop the turn' })).toBeInTheDocument();
    expect(screen.queryByText(/sending your answer/i)).toBeNull();
  });

  it('does not carry an answer onto the next question', async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <ApprovalCard interrupt={THREE_ANSWERS} onAnswer={() => new Promise(() => {})} />,
    );
    await user.click(screen.getByRole('button', { name: 'Allow' }));

    rerender(
      <ApprovalCard
        interrupt={{ ...THREE_ANSWERS, id: 'apr_8Z3', message: 'Something else?' }}
        onAnswer={() => new Promise(() => {})}
      />,
    );

    expect(screen.getByRole('button', { name: 'Allow' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Deny' })).toBeInTheDocument();
  });
});
