import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
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
    render(<ApprovalCard interrupt={THREE_ANSWERS} onAnswer={vi.fn()} />);
    expect(screen.getByText('Run `rm -rf build/`?')).toBeInTheDocument();
    expect(screen.getByText('chrono:command_execution')).toBeInTheDocument();
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
    // §3 and §18.3: a plate floats over the page. This sits in the stream
    // where the turn stopped, so it rests — --lift-rest — with no fixed
    // position.
    const css = readFileSync(resolve(process.cwd(), 'src/chat/ApprovalCard.css'), 'utf8');
    const rule = /\.approval\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).toContain('box-shadow: var(--lift-rest)');
    expect(rule).not.toContain('position: fixed');
    expect(rule).not.toContain('z-index');
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
