import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MessageStream } from './MessageStream';
import { AgentContent, blocksOf } from './AgentContent';
import { EMPTY, fold, reduce, type Conversation } from './conversation';
import { AguiConnection, MockSession, toEvent, transcriptNamed, type AguiEvent } from '../agui';

/**
 * The message stream — `Web_Style.md` §18.1.
 */

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Drive a transcript through the real client and fold what comes out. */
async function conversationFrom(name: string, options: { dropAfter?: number } = {}) {
  const session = new MockSession(transcriptNamed(name), options);
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
  return { events: seen, conversation: fold(seen) };
}

describe('the equivalence rule', () => {
  it('produces identical markup replayed and watched', async () => {
    // §18.1, and the assertion this component exists to hold: "the surface must
    // produce an identical result whether it watched a turn stream or drew it
    // from history on reconnect." The one a person reloads into is the one they
    // will believe, so a disagreement is a defect rather than a style.
    const live = await conversationFrom('plain-turn');
    const replayed = await conversationFrom('plain-turn', { dropAfter: 2 });

    const watching = render(<MessageStream entries={live.conversation.entries} />);
    const watchedHtml = watching.container.innerHTML;
    watching.unmount();

    const reading = render(<MessageStream entries={replayed.conversation.entries} />);
    expect(reading.container.innerHTML).toBe(watchedHtml);
  });

  it('folds one function for both, so the two cannot disagree', async () => {
    // Not a coincidence of two code paths agreeing today: there is one path.
    const { events } = await conversationFrom('two-tool-calls');
    const atOnce = fold(events);
    const oneAtATime = events.reduce<Conversation>(reduce, EMPTY);
    expect(atOnce).toEqual(oneAtATime);
  });

  it('reaches the same state however many times an event is folded', async () => {
    // The connection deduplicates on `seq`; this makes a redelivery harmless
    // even if it ever did not.
    const { events } = await conversationFrom('plain-turn');
    expect(fold([...events, ...events])).toEqual(fold(events));
  });
});

describe('a turn', () => {
  it('is a band with a label, not a bubble', () => {
    const state = fold([
      toEvent({ type: 'RUN_STARTED', seq: 1 }),
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 2, messageId: 'm1', role: 'user' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 3, messageId: 'm1', delta: 'do the thing' }),
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 4, messageId: 'm2', role: 'assistant' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 5, messageId: 'm2', delta: 'done' }),
    ]);
    const { container } = render(<MessageStream entries={state.entries} />);

    expect(container.querySelectorAll('.turn')).toHaveLength(2);
    expect(container.querySelector('[data-role="user"]')).not.toBeNull();
    expect(container.querySelector('[data-role="assistant"]')).not.toBeNull();
    expect(screen.getByText('You')).toBeInTheDocument();
    expect(screen.getByText('Agent')).toBeInTheDocument();
  });

  it('changes nothing about a message when it commits, except the caret', () => {
    // §18.1: "a message that restyles itself on commit tells a reader the
    // earlier state was provisional when it was not."
    const arriving = fold([
      toEvent({ type: 'RUN_STARTED', seq: 1 }),
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 2, messageId: 'm1', role: 'assistant' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 3, messageId: 'm1', delta: 'half' }),
    ]);
    const settled = reduce(
      arriving,
      toEvent({ type: 'TEXT_MESSAGE_END', seq: 4, messageId: 'm1' }),
    );

    const live = render(<MessageStream entries={arriving.entries} turnInFlight />);
    const liveHtml = live.container.innerHTML;
    live.unmount();

    const done = render(<MessageStream entries={settled.entries} turnInFlight />);
    const doneHtml = done.container.innerHTML;

    // The only difference is the caret.
    expect(liveHtml.replace(/<b class="turn__caret"[^>]*><\/b>/, '')).toBe(doneHtml);
    expect(liveHtml).toContain('turn__caret');
    expect(doneHtml).not.toContain('turn__caret');
  });

  it('draws a rotation as a boundary in the stream', async () => {
    // §18.6: not a status chip. An owner who can see the boundary recognises a
    // gap rather than puzzling over one.
    const { conversation } = await conversationFrom('rotates');
    render(<MessageStream entries={conversation.entries} />);

    expect(screen.getByRole('separator')).toBeInTheDocument();
    expect(screen.getByText(/carried forward from/i)).toBeInTheDocument();
    expect(screen.getByText(/did not travel/i)).toBeInTheDocument();
  });

  it('shows an event kind it does not know rather than hiding it', async () => {
    const { conversation } = await conversationFrom('unknown-kind');
    render(<MessageStream entries={conversation.entries} />);
    expect(screen.getByText('THINKING_START')).toBeInTheDocument();
    expect(screen.getByText(/was not lost/i)).toBeInTheDocument();
  });
});

describe('agent-emitted content', () => {
  it('renders the structure the agent wrote', () => {
    const text = [
      '# Heading',
      '',
      'A paragraph with `inline` code.',
      '',
      '- one',
      '- two',
      '',
      '```rust',
      'fn main() {}',
      '```',
    ].join('\n');

    const { container } = render(<AgentContent text={text} />);

    expect(container.querySelector('.agent__heading')?.textContent).toBe('Heading');
    expect(container.querySelectorAll('.agent__list li')).toHaveLength(2);
    expect(container.querySelector('.agent__block')?.textContent).toBe('fn main() {}');
    expect(container.querySelector('.agent__code')?.textContent).toBe('inline');
  });

  it('applies no colour to anything the agent wrote', () => {
    // §18.1: "No colour is applied to agent content, ever. Not syntax
    // highlighting, not a tint per role, not a semantic colour on a word the
    // agent happened to write." §2 reserves those tokens for session and
    // system state, and they describe nothing an agent said.
    const css = readCss();
    const agentRules = [...css.matchAll(/\.agent[^{]*\{([^}]*)\}/g)].map((match) => match[1] ?? '');
    expect(agentRules.length).toBeGreaterThan(0);
    for (const rule of agentRules) {
      expect(rule).not.toMatch(/(^|\s)color:/);
      expect(rule).not.toContain('--live');
      expect(rule).not.toContain('--wait');
      expect(rule).not.toContain('--fail');
      expect(rule).not.toContain('--accent');
    }
  });

  it('never turns a string into markup', () => {
    // This content came from a coding agent executing the user's own untrusted
    // code (§9). React escapes what it renders; nothing here opts out.
    const { container } = render(<AgentContent text={'<img src=x onerror=alert(1)>'} />);
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).toContain('<img src=x onerror=alert(1)>');
  });

  it('shows the words of something it cannot parse rather than swallowing them', () => {
    const text = '| a table | it does not render |\n|---|---|\n| but | shows |';
    const { container } = render(<AgentContent text={text} />);
    expect(container.textContent).toContain('a table');
    expect(container.textContent).toContain('shows');
  });

  it('takes the rest of the message when a fence never closes', () => {
    const blocks = blocksOf('before\n```\nunclosed forever');
    expect(blocks.at(-1)).toEqual({ kind: 'code', language: '', body: 'unclosed forever' });
  });
});

/** The stylesheet itself is the assertion: no rule may colour agent content. */
function readCss(): string {
  return readFileSync(resolve(process.cwd(), 'src/chat/MessageStream.css'), 'utf8');
}

describe('the plane-change boundary', () => {
  it('names where the session went and when history stopped being continuous', () => {
    // §18.6: a rule across the stream, labelled with the time in the display
    // zone. An owner who switched planes and finds their conversation
    // apparently intact — but silently missing the last two minutes — has been
    // misled by the interface rather than by the platform.
    render(
      <MessageStream
        entries={[
          {
            kind: 'gap',
            id: 'plane-9',
            rotatedAt: '2026-08-18T09:14:22Z',
            movedTo: 'conversation',
            seq: 9,
          },
        ]}
      />,
    );

    const boundary = screen.getByRole('separator');
    expect(boundary).toHaveTextContent(/moved to conversation/i);
    expect(boundary).toHaveTextContent(/did not travel/i);
  });

  it('still draws a boundary nobody asked for', () => {
    // A stretch the runtime had to drop is the same kind of fact and is drawn
    // the same way, minus a destination there is none of.
    render(
      <MessageStream entries={[{ kind: 'gap', id: 'gap-4', rotatedAt: undefined, seq: 4 }]} />,
    );

    const boundary = screen.getByRole('separator');
    expect(boundary).toHaveTextContent(/carried forward/i);
    expect(boundary).not.toHaveTextContent(/moved to/i);
  });
});

describe('a run that failed, §18.9', () => {
  /** Fold a failure as the projection sends one, through the real parser. */
  const failure = (body: Record<string, unknown>) =>
    fold([toEvent({ type: 'RUN_ERROR', seq: 9, message: 'the run failed', ...body })]);

  it('says what failed, in the stream rather than in place of it', () => {
    const state = fold([
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 1, messageId: 'm1', role: 'assistant' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 2, messageId: 'm1', delta: 'reading a file' }),
      toEvent({ type: 'RUN_ERROR', seq: 3, message: 'the model endpoint refused' }),
    ]);
    render(<MessageStream entries={state.entries} />);

    expect(screen.getByText('the model endpoint refused')).toBeTruthy();
    expect(
      screen.getByText('reading a file'),
      'what the agent already did stays readable',
    ).toBeTruthy();
  });

  it('says the word as well as wearing the ink', () => {
    // §2: colour is never the only carrier. A reader who cannot distinguish
    // the ink must still be able to read what happened.
    render(<MessageStream entries={failure({}).entries} />);

    expect(screen.getByText('Run failed')).toBeTruthy();
  });

  it('names the ability when the failure was a refusal', () => {
    render(<MessageStream entries={failure({ capability: 'conversation' }).entries} />);

    const named = screen.getByText(/This session cannot/);
    expect(named.textContent).toContain('conversation');
  });

  it('names nothing when there was nothing to name', () => {
    render(<MessageStream entries={failure({}).entries} />);

    expect(screen.queryByText(/This session cannot/)).toBeNull();
  });

  it('draws nothing at all for a session being carried onto another runtime', () => {
    // §11: a rotating session never takes the failure ink. It arrives on the
    // same frame as a failure, so this is the only thing keeping them apart.
    const state = failure({
      message: 'moving this session to the other plane',
      'chrono:returns': true,
    });
    render(<MessageStream entries={state.entries} />);

    expect(screen.queryByText('Run failed')).toBeNull();
  });
});

describe('a turn in flight, §18.1', () => {
  it('says something is happening before anything has arrived', () => {
    // A turn that has started and produced nothing is otherwise
    // indistinguishable from a dead stream, and a turn takes as long as the
    // agent takes.
    render(<MessageStream entries={[]} turnInFlight />);

    expect(screen.getByText('Working')).toBeTruthy();
  });

  it('stops saying it the moment there is something to read', () => {
    render(<MessageStream entries={[]} turnInFlight live={{ text: 'Hel' }} />);

    expect(screen.queryByText('Working')).toBeNull();
    expect(screen.getByText('Hel')).toBeTruthy();
  });

  it('draws the message being typed after the last entry, not as one', () => {
    const state = fold([
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 1, messageId: 'm1', role: 'user' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 1, messageId: 'm1', delta: 'do the thing' }),
    ]);
    const { container } = render(
      <MessageStream entries={state.entries} turnInFlight live={{ text: 'on it' }} />,
    );

    const turns = [...container.querySelectorAll('.turn')];
    expect(turns).toHaveLength(2);
    expect(turns[1]?.textContent).toContain('on it');
  });

  it('gives the caret to the message being typed and to nothing else', () => {
    const settled = fold([
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 1, messageId: 'm1', role: 'assistant' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 1, messageId: 'm1', delta: 'committed' }),
      toEvent({ type: 'RUN_FINISHED', seq: 2 }),
    ]);
    const { container } = render(
      <MessageStream entries={settled.entries} turnInFlight live={{ text: 'still going' }} />,
    );

    const carets = [...container.querySelectorAll('.turn__caret')];
    expect(carets).toHaveLength(1);
    expect(carets[0]?.closest('.turn')?.textContent).toContain('still going');
  });

  it('draws nothing extra when a turn is not in flight', () => {
    render(<MessageStream entries={[]} />);

    expect(screen.queryByText('Working')).toBeNull();
  });
});
