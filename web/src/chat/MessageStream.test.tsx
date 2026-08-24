import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { resizeObserved } from '../test/setup';
import { MessageStream } from './MessageStream';
import { AgentContent } from './AgentContent';
import { blocksOf } from './blocks';
import { summaryOf } from './Thinking';
import { EMPTY, fold, reduce, type Conversation, type Entry } from '@chronoai/cma-sdk';
import { AguiConnection, toEvent as parseEvent, type AguiEvent } from '@chronoai/cma-sdk';
import { MockSession, transcriptNamed } from '@chronoai/cma-sdk/testing';

/**
 * The message stream — `Web_Style.md` §18.1.
 */

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Parse a frame in the session/run envelope shared by the focused fixtures. */
const toEvent = (frame: Record<string, unknown>): AguiEvent =>
  parseEvent({ threadId: 'sesn_1', runId: 'run_1', ...frame });

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

describe('the opening card and what an empty column means', () => {
  it('waits for the log before saying a session has said nothing', () => {
    // An empty column means "this session has said nothing" only once the log
    // has been read. Before that it means "nothing has arrived yet", and
    // announcing readiness over somebody's history — in the moment they are
    // waiting to see it — tells them their work is gone.
    render(<MessageStream entries={[]} linkLive={false} />);
    expect(screen.queryByText(/ready when you are/i)).toBeNull();
  });

  it('says so once the log has been read and there is genuinely nothing', () => {
    render(<MessageStream entries={[]} linkLive />);
    expect(screen.getByText(/ready when you are/i)).toBeInTheDocument();
  });
});

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

  it('rebuilds the complete review fixture identically after a reconnect', async () => {
    const live = await conversationFrom('full-conversation');
    const replayed = await conversationFrom('full-conversation', { dropAfter: 12 });
    expect(replayed.conversation).toEqual(live.conversation);

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
  it('defines the owner bubble and its alignment exactly once', () => {
    const css = readCss();
    expect(css.match(/\.turn\[data-role='user'\]\s*\{/g) ?? []).toHaveLength(1);
    expect(css.match(/\.turn\[data-role='user'\]\s+\.turn__body\s*\{/g) ?? []).toHaveLength(1);
  });

  it('tells the two roles apart by shape alone', () => {
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
    /*
     * §18.1: the owner's turns are bubbles and the agent's run the full
     * measure, which is what tells them apart. The agent's eyebrow is gone —
     * it repeated "CMA · codex" above every turn, a label nobody reads
     * twice that cost a line of the reading column each time. Which kernel is
     * answering is a fact about the session, and the session header says it.
     */
    expect(screen.getByText('You')).toBeInTheDocument();
    expect(screen.queryByText(/CMA ·/)).toBeNull();
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
    //
    // Compared on the message itself rather than the whole container, which is
    // what the rule governs: §18.1 is about a *message* restyling on commit.
    // The turn's copy control is refused while the turn is being written and
    // available once it settles (§18.11), which is an affordance becoming
    // usable rather than the message reporting that it was provisional — a
    // whole-container comparison cannot tell those apart.
    const bodyOf = (html: string) => /<div class="turn__body">.*?<\/div>/s.exec(html)?.[0] ?? html;
    expect(bodyOf(liveHtml).replace(/<b class="turn__caret"[^>]*><\/b>/, '')).toBe(
      bodyOf(doneHtml),
    );
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
    /*
     * `inherit` and `currentcolor` are exempt, and the exemption is the point
     * rather than a loophole: they are how a rule *refuses* a colour. Without
     * it this check forbade the one declaration that keeps a link out of the
     * browser's blue — and a link rendered blue is precisely the exception
     * this rule exists to prevent, arriving because the rule forbade its own
     * enforcement.
     */
    const css = readCss();
    const agentRules = [...css.matchAll(/\.agent[^{]*\{([^}]*)\}/g)].map((match) => match[1] ?? '');
    expect(agentRules.length).toBeGreaterThan(0);
    for (const rule of agentRules) {
      for (const declaration of rule.matchAll(/(?:^|\s)color:\s*([^;]+);/g)) {
        expect(['inherit', 'currentcolor'], `agent content sets ${declaration[1] ?? ''}`).toContain(
          (declaration[1] ?? '').trim().toLowerCase(),
        );
      }
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

describe('the stream stylesheet contract', () => {
  it('uses the reference code frame and distinct failure and warning notices', () => {
    const css = readCss();
    expect(css).toMatch(
      /\.codeblock\s*\{[^}]*background: var\(--dsw-alias-markdown-code-block\)[^}]*border: 1px solid var\(--dsw-alias-border-l1\)[^}]*border-radius: 12px/,
    );
    expect(css).toMatch(
      /\.codeblock__head\s*\{[^}]*position: sticky[^}]*background: var\(--dsw-alias-markdown-code-block-banner\)/,
    );
    expect(css).toMatch(
      /\.failure\s*\{[^}]*background: var\(--dsw-alias-interactive-bg-hover-danger\)[^}]*border-left: 2px solid var\(--dsw-alias-state-error-primary\)/,
    );
    expect(css).toMatch(
      /\.gap\s*\{[^}]*background: var\(--dsw-alias-state-warn-tertiary\)[^}]*border-left: 2px solid var\(--dsw-alias-state-warn-primary\)/,
    );
  });
});

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
      'cma:returns': true,
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

describe('what the agent is working through, §18.8', () => {
  it('collapses the reasoning to one key that says it is not kept', async () => {
    // §18.8: collapsed by default — the previous revision's always-open band
    // put the least durable content at the same weight as the conversation.
    // The words that make it safe to lose stay on the key either way.
    const user = userEvent.setup();
    render(<MessageStream entries={[]} turnInFlight reasoning={{ text: 'checking src/' }} />);

    const key = screen.getByRole('button', { name: /not kept after this turn/i });
    /*
     * The row shows the newest line while it is shut — that is its reason to
     * exist, since "it is doing something" is what a spinner says and this
     * says what. The body is what stays closed.
     */
    expect(key.textContent).toContain('checking src/');
    expect(document.querySelector('.thinking__body')).toBeNull();
    await user.click(key);
    expect(document.querySelector('.thinking__body')?.textContent).toBe('checking src/');
  });

  it('draws nothing at all when there is no reasoning', () => {
    // Not an empty band. §18.4's rule: a thing that can never appear is not
    // one that is currently unavailable, and an empty box would suggest the
    // agent was thinking silently.
    const { container } = render(<MessageStream entries={[]} turnInFlight />);

    expect(container.querySelector('.thinking')).toBeNull();
  });

  it('carries no colour', () => {
    // §2: the semantic inks describe session and system state and describe
    // nothing the agent thought.
    const { container } = render(<MessageStream entries={[]} reasoning={{ text: 'hmm' }} />);
    const classes = [...container.querySelectorAll('.thinking, .thinking *')]
      .flatMap((node) => [...node.classList])
      .join(' ');

    expect(classes).not.toMatch(/live|wait|fail|ended|accent/);
  });

  it('replaces the waiting wave once reasoning starts arriving', () => {
    render(<MessageStream entries={[]} turnInFlight reasoning={{ text: 'hmm' }} />);

    expect(screen.queryByText('Working')).toBeNull();
  });
});

describe('the owner turn label, §18.1', () => {
  it('carries the time the owner sent it', () => {
    // 04:00 UTC is 12:00 in Singapore, the fixed zone §12 uses — so the label
    // means the same moment whoever is looking.
    render(
      <MessageStream
        entries={[
          {
            kind: 'message',
            id: 'm1',
            role: 'user',
            text: 'build me a scraper',
            committed: true,
            seq: 1,
            at: '2026-08-19T04:00:00Z',
          },
        ]}
      />,
    );

    expect(screen.getByText(/You · 12:00/)).toBeInTheDocument();
  });

  it('shows no time when the log carries none', () => {
    // A control plane predating timestamps wrote no time; absence is shown as
    // absence, never as an estimated moment.
    render(
      <MessageStream
        entries={[{ kind: 'message', id: 'm1', role: 'user', text: 'hi', committed: true, seq: 1 }]}
      />,
    );

    expect(screen.getByText('You')).toBeInTheDocument();
    expect(screen.queryByText(/You ·/)).toBeNull();
  });
});

describe('following the turn as it is written', () => {
  /*
   * The defect this guards was the largest felt one on the conversation
   * plane: the view stopped following at exactly the moment text started
   * arriving, so every long answer was read by scrolling by hand.
   *
   * Its cause is why the test drives a resize rather than a rerender. The
   * follow used to depend on `entries`, and a streaming delta lands in `live`
   * and returns a conversation holding the *same* entries array — nothing in
   * a dependency list changed, so nothing ran.
   */
  /*
   * One array, reused across every render in this block, because that is what
   * the real path does: a streaming delta lands in `live` and the conversation
   * it returns holds the *same* entries. Passing a fresh `[]` each time would
   * hand the old code a changed dependency it never gets in production, and
   * these tests would pass against the very defect they exist to catch.
   */
  const settled: readonly Entry[] = fold([
    toEvent({ type: 'RUN_STARTED', seq: 1 }),
    toEvent({ type: 'TEXT_MESSAGE_START', seq: 2, messageId: 'm1', role: 'user' }),
    toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 3, messageId: 'm1', delta: 'do the thing' }),
  ]).entries;

  const stream = (props: Partial<Parameters<typeof MessageStream>[0]> = {}) => (
    <MessageStream entries={settled} turnInFlight={true} {...props} />
  );

  /** A surface with more content than fits, which jsdom does not lay out. */
  function measured(container: HTMLElement): HTMLElement {
    const surface = container.querySelector('.stream__surface');
    if (surface === null) {
      throw new Error('the stream has no scrolling surface');
    }
    Object.defineProperty(surface, 'scrollHeight', { configurable: true, value: 4000 });
    Object.defineProperty(surface, 'clientHeight', { configurable: true, value: 600 });
    return surface as HTMLElement;
  }

  /** Scroll away from the newest turn, the way a reader does. */
  function scrollUp(surface: HTMLElement): void {
    surface.scrollTop = 120;
    fireEvent.scroll(surface);
  }

  it('scrolls to the newest text as the column grows', () => {
    const { container, rerender } = render(stream());
    const surface = measured(container);

    rerender(stream({ live: { text: 'the answer so far' } }));
    resizeObserved();

    expect(surface.scrollTop).toBe(4000);
  });

  it('leaves the reader alone once they have scrolled away', () => {
    // Dragging somebody back to the bottom while they are reading something
    // further up is worse than not following at all.
    const { container, rerender } = render(stream());
    const surface = measured(container);
    scrollUp(surface);

    rerender(stream({ live: { text: 'more' } }));
    resizeObserved();

    expect(surface.scrollTop).toBe(120);
  });

  it('jumps to the newest turn the moment following resumes', () => {
    // Without this, re-pinning does nothing until the agent happens to emit
    // another token, and on a finished turn that is never.
    const { container } = render(stream());
    const surface = measured(container);
    scrollUp(surface);

    surface.scrollTop = 3400;
    fireEvent.scroll(surface);

    expect(surface.scrollTop).toBe(4000);
  });
});

describe('what a screen reader hears', () => {
  /*
   * A conversation is a log, and a log announces what arrives. The failure
   * this guards is that the default live region also announces text changing
   * inside nodes it already holds — and the agent's message grows one delta at
   * a time, so a reader was re-reading the whole message on every token. For a
   * long answer that never finishes and cannot be interrupted.
   */
  it('announces additions rather than every character typed', () => {
    const { container } = render(<MessageStream entries={[]} />);
    const log = container.querySelector('[role="log"]');
    expect(log).toHaveAttribute('aria-live', 'polite');
    expect(log).toHaveAttribute('aria-relevant', 'additions');
  });

  it('keeps the message being typed out of the announcements', () => {
    // It is announced when it commits, once and whole, because the committed
    // message is an addition.
    const { container } = render(<MessageStream entries={[]} live={{ text: 'half a sen' }} />);
    expect(container.querySelector('.turn[data-role="assistant"]')).toHaveAttribute(
      'aria-live',
      'off',
    );
  });
});

describe('what a reader can take away', () => {
  /*
   * §18.11. An answer that cannot leave the surface has not finished being
   * useful, and the two things somebody does with what an agent produced are
   * run it and quote it — both of which mean getting the characters out.
   */
  const settledTurn = () =>
    fold([
      toEvent({ type: 'RUN_STARTED', seq: 1 }),
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 2, messageId: 'm1', role: 'assistant' }),
      toEvent({
        type: 'TEXT_MESSAGE_CONTENT',
        seq: 3,
        messageId: 'm1',
        delta: '# Steps\n\n1. Run `npm ci`\n',
      }),
      toEvent({ type: 'TEXT_MESSAGE_END', seq: 4, messageId: 'm1' }),
    ]).entries;

  it('copies the markdown the agent wrote, not the tree it was drawn as', () => {
    // The whole reason this exists. Selection-copy already works and already
    // yields degraded text: the renderer strips heading hashes, list markers
    // and backticks before drawing, so pasting into an issue or a shell gives
    // something the agent did not write.
    // `fireEvent` rather than `userEvent`, which installs a clipboard stub of
    // its own and would be the thing under test instead of the component.
    const written = vi.fn().mockResolvedValue(undefined);
    const real = navigator.clipboard;
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: written },
    });

    render(<MessageStream entries={settledTurn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));

    expect(written).toHaveBeenCalledWith('# Steps\n\n1. Run `npm ci`\n');
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: real });
  });

  it('refuses while the turn is still being written, rather than vanishing', () => {
    // §7 and §19: copying half an answer is the thing to prevent, and removing
    // the control is the wrong instrument for it.
    const arriving = fold([
      toEvent({ type: 'RUN_STARTED', seq: 1 }),
      toEvent({ type: 'TEXT_MESSAGE_START', seq: 2, messageId: 'm1', role: 'assistant' }),
      toEvent({ type: 'TEXT_MESSAGE_CONTENT', seq: 3, messageId: 'm1', delta: 'half' }),
    ]).entries;

    const { container } = render(<MessageStream entries={arriving} turnInFlight={true} />);
    const key = container.querySelector('.turn__actions .copykey');
    expect(key).toBeDisabled();
    expect(key).toHaveAttribute('title');
  });

  it('gives a fenced block its language and a way to take it', () => {
    const { container } = render(<AgentContent text={'```python\nprint(1)\n```'} />);
    // Verbatim, uppercased by type rather than mapped onto a name this client
    // invented — §18.2 and §18.10 forbid that in identical words.
    expect(container.querySelector('.codeblock__lang')?.textContent).toBe('python');
    expect(container.querySelector('.codeblock .copykey')).not.toBeNull();
  });

  it('draws no label where the agent named no language', () => {
    const { container } = render(<AgentContent text={'```\nplain\n```'} />);
    expect(container.querySelector('.codeblock__lang')?.textContent).toBe('');
  });
});

describe('what the agent wrote reaches the reader as written', () => {
  /*
   * §18.1's Fidelity rule: "Lists, headings and emphasis take the scale §4
   * defines." Emphasis was required from the day §18 was written and never
   * rendered, so `**do not**` arrived as four asterisks around a word — an
   * agent emphasising something is an agent saying it matters, and printing
   * its punctuation is the surface refusing to hear that.
   */
  const drawn = (markdown: string) => {
    const { container } = render(<AgentContent text={markdown} />);
    return container;
  };

  it('renders strong and emphatic runs rather than their punctuation', () => {
    const c = drawn('Do **not** delete _anything_ yet');
    expect(c.querySelector('strong')?.textContent).toBe('not');
    expect(c.querySelector('em')?.textContent).toBe('anything');
    expect(c.textContent).toBe('Do not delete anything yet');
  });

  it('leaves the inside of inline code alone', () => {
    // `a ** b` is an expression, not emphasis. A pass that ran the other way
    // round would quietly rewrite what the agent wrote.
    const c = drawn('Use `a ** b` for that');
    expect(c.querySelector('code')?.textContent).toBe('a ** b');
    expect(c.querySelector('strong')).toBeNull();
  });

  it('does not italicise the middle of a snake_case name', () => {
    // Agents write these constantly, and turning their middles italic corrupts
    // an identifier the reader may be about to copy.
    const c = drawn('Set environment_profile_id on the request');
    expect(c.querySelector('em')).toBeNull();
    expect(c.textContent).toBe('Set environment_profile_id on the request');
  });

  it('gives a long answer a real outline', () => {
    // §18.1 says headings take §4's scale; they were emitting <p>, so a screen
    // reader could not move between the sections of an agent's plan.
    const c = drawn('# Plan\n\n## First step');
    expect(c.querySelector('h2')?.textContent).toBe('Plan');
    expect(c.querySelector('h3')?.textContent).toBe('First step');
    // The page owns h1; an agent starting at `#` must not compete for it.
    expect(c.querySelector('h1')).toBeNull();
  });
});

describe('when the surface cannot hear the session', () => {
  /*
   * §18.13. The failure mode is asserting liveness that is false, not failing
   * to narrate the transport: nothing but the run ending clears the in-flight
   * state, so a dead stream kept the pulse going and the caret blinking on a
   * message that had stopped growing — the one affordance built to tell a
   * working agent from a dead stream was the one that lied.
   */
  it('stops claiming the agent is working the moment the link is not live', () => {
    const { container, rerender } = render(
      <MessageStream entries={[]} turnInFlight={true} linkLive={true} />,
    );
    expect(container.querySelector('.stream__waiting')).not.toBeNull();

    rerender(<MessageStream entries={[]} turnInFlight={true} linkLive={false} />);
    expect(container.querySelector('.stream__waiting')).toBeNull();
  });

  it('stops the caret too, on the message that has stopped growing', () => {
    const { container } = render(
      <MessageStream
        entries={[]}
        turnInFlight={true}
        linkLive={false}
        live={{ text: 'half a sen' }}
      />,
    );
    expect(container.querySelector('.turn__caret')).toBeNull();
  });

  it('says nothing at all during an ordinary reconnect', () => {
    // A blip announced as a fault teaches a reader to distrust the next
    // notice, and §11 forbids drawing a recovering session as a failed one.
    const { container } = render(
      <MessageStream entries={[]} turnInFlight={true} linkLive={false} />,
    );
    expect(container.querySelector('.stream__unheard')).toBeNull();
  });

  it('names it once the link has stayed down, and not as a failure', () => {
    const { container } = render(
      <MessageStream entries={[]} turnInFlight={true} linkLive={false} linkLost={true} />,
    );
    const notice = container.querySelector('.stream__unheard');
    expect(notice).not.toBeNull();
    // §18.9 draws a run the agent could not finish; this draws a link that
    // stopped. Conflating them tells an owner their work broke when their
    // network did.
    expect(container.querySelector('.failure')).toBeNull();
    expect(notice?.textContent).toMatch(/nothing has been lost/i);
  });
});

describe('what an agent wrote, beyond code and lists', () => {
  /*
   * §18.12. The renderer drew none of this: a link arrived as `[label](url)`,
   * a table as a wall of pipes, every nesting level flattened to one, and a
   * wrapped list of steps ran together into a paragraph.
   */
  const drawn = (markdown: string) => render(<AgentContent text={markdown} />).container;

  it('draws a link, and decides nothing about where it points', () => {
    const c = drawn('See [the release notes](https://example.com/r) first');
    const link = c.querySelector('a.agent__link');
    expect(link?.textContent).toBe('the release notes');
    expect(link).toHaveAttribute('href', 'https://example.com/r');
    // Untrusted by construction: this came from an agent running the owner's
    // own code.
    expect(link).toHaveAttribute('rel', expect.stringContaining('noopener'));
  });

  it('leaves a bare bracket alone', () => {
    // Not everything in square brackets is a link, and rewriting prose that
    // merely looks like one would corrupt what the agent wrote.
    const c = drawn('The array is [1, 2, 3] here');
    expect(c.querySelector('a')).toBeNull();
    expect(c.textContent).toContain('[1, 2, 3]');
  });

  it('keeps a list nested where the agent nested it', () => {
    // A plan with sub-steps flattened to one level is a different plan.
    const c = drawn('- build\n  - install\n  - compile\n- ship');
    const items = [...c.querySelectorAll('li')];
    expect(items.map((li) => li.getAttribute('data-depth'))).toEqual([null, '1', '1', null]);
  });

  it('draws a table as a table', () => {
    const c = drawn('| step | who |\n| --- | --- |\n| build | ci |\n| ship | you |');
    expect(c.querySelectorAll('th')).toHaveLength(2);
    expect(c.querySelectorAll('tbody tr')).toHaveLength(2);
    // Scrolls inside its own frame rather than widening the reading column.
    expect(c.querySelector('.agent__tablewrap')).not.toBeNull();
  });

  it('does not turn a sentence with pipes into a table', () => {
    // Recognised only where a head is followed by its rule; without that
    // check, prose containing pipes becomes a table.
    const c = drawn('Run `a | b | c` to see it');
    expect(c.querySelector('table')).toBeNull();
  });

  it('draws a blockquote with a rule and no ink of its own', () => {
    // §18.1 forbids ink on agent content without exception. Drawing a quote
    // fainter would be this surface deciding it matters less than the agent
    // did.
    const c = drawn('> the agent quoted this');
    expect(c.querySelector('blockquote')?.textContent).toBe('the agent quoted this');
  });

  it('keeps a soft line break as a line break', () => {
    // An agent that wrote its steps on separate lines meant them on separate
    // lines; joining them rewrites the shape of the answer.
    const c = drawn('first line\nsecond line');
    expect(c.querySelector('.agent__paragraph')?.textContent).toBe('first line\nsecond line');
  });
});

describe('following the newest turn', () => {
  /** A message entry, which is all this behaviour cares about. */
  function turn(id: string): Entry {
    return { kind: 'message', id, role: 'assistant', text: id, seq: Number(id), committed: true };
  }

  /** Put the surface at a scroll position, the way a person would. */
  function scrollTo(distanceFromBottom: number) {
    const surface = screen.getByRole('log', { name: 'Conversation' });
    Object.defineProperty(surface, 'scrollHeight', { value: 1000, configurable: true });
    Object.defineProperty(surface, 'clientHeight', { value: 400, configurable: true });
    surface.scrollTop = 600 - distanceFromBottom;
    fireEvent.scroll(surface);
  }

  it('says nothing while somebody is following along', () => {
    // Pinned is the ordinary state, and a control counting entries somebody
    // is already reading would be noise on every turn.
    render(<MessageStream entries={[turn('1')]} />);
    expect(screen.queryByRole('button', { name: /newer/i })).toBeNull();
  });

  it('does not chase somebody who has scrolled away', () => {
    /*
     * §18.1's rule. A person reading something the agent said four turns ago
     * has said what they want, so what arrives while they read is announced
     * rather than scrolled to.
     */
    const view = render(<MessageStream entries={[turn('1')]} />);
    scrollTo(300);

    view.rerender(<MessageStream entries={[turn('1'), turn('2'), turn('3')]} />);

    expect(screen.getByRole('button', { name: /2 newer entries/i })).toBeInTheDocument();
  });

  it('counts one entry as one, not as a plural', () => {
    const view = render(<MessageStream entries={[turn('1')]} />);
    scrollTo(300);

    view.rerender(<MessageStream entries={[turn('1'), turn('2')]} />);

    expect(screen.getByRole('button', { name: /1 newer entry\./i })).toBeInTheDocument();
  });

  it('follows again the moment somebody scrolls back to the bottom', () => {
    const view = render(<MessageStream entries={[turn('1')]} />);
    scrollTo(300);
    view.rerender(<MessageStream entries={[turn('1'), turn('2')]} />);
    expect(screen.getByRole('button', { name: /newer/i })).toBeInTheDocument();

    scrollTo(0);
    expect(screen.queryByRole('button', { name: /newer/i })).toBeNull();
  });

  it('treats near-enough to the bottom as the bottom', () => {
    /*
     * A few pixels of overscroll, a fractional layout, or a scrollbar that
     * rounds should not read as having deliberately scrolled away — the
     * effect of getting this wrong is the surface stopping following while
     * somebody watches it, which looks like the stream dying.
     *
     * Checked by what arrives next: a scroll alone resets the count, so the
     * threshold is only visible in whether the following entry is chased.
     */
    const view = render(<MessageStream entries={[turn('1')]} />);
    scrollTo(10);

    view.rerender(<MessageStream entries={[turn('1'), turn('2')]} />);

    expect(screen.queryByRole('button', { name: /newer/i })).toBeNull();
  });

  it('names what the control does, since its glyph says it to nobody', () => {
    // A circle with a chevron in it says "go down" to somebody who can see
    // it, and says nothing at all otherwise.
    const view = render(<MessageStream entries={[turn('1')]} />);
    scrollTo(300);
    view.rerender(<MessageStream entries={[turn('1'), turn('2')]} />);

    const control = screen.getByRole('button', { name: /newer/i });
    expect(control.getAttribute('aria-label')).toMatch(/go to the newest/i);
    expect(control.textContent).toBe('');
  });
});

describe('the reasoning summary', () => {
  it('shows the newest line, which is what the agent is on now', () => {
    // A row that showed the first line would say what the agent set out to
    // do and keep saying it for the whole turn, which is a spinner with
    // words on it.
    expect(summaryOf('first thought\nsecond thought\nthird thought')).toBe('third thought');
  });

  it('ignores the blank lines a stream leaves behind', () => {
    // Reasoning arrives a delta at a time and routinely ends mid-newline, so
    // the last line is empty about as often as not.
    expect(summaryOf('reading the config\n\n')).toBe('reading the config');
  });

  it('says nothing rather than something wrong when there is nothing yet', () => {
    expect(summaryOf('')).toBe('');
    expect(summaryOf('\n  \n')).toBe('');
  });
});

describe('what a real parser reads that regexes did not', () => {
  const drawn = (markdown: string) => render(<AgentContent text={markdown} />).container;

  it('draws an image as an image, not as a link with a stray bang', () => {
    /*
     * The hand-rolled inline pass matched `[alt](url)` inside `![alt](url)`,
     * so every screenshot an agent posted arrived as a link with an
     * exclamation mark in front of it.
     */
    const c = drawn('Here it is: ![the failing run](https://example.com/a.png)');
    const image = c.querySelector('img.agent__image');
    expect(image).toHaveAttribute('src', 'https://example.com/a.png');
    expect(image).toHaveAttribute('alt', 'the failing run');
    expect(c.textContent).not.toContain('!');
  });

  it('keeps a task list a task list', () => {
    // Agents write these constantly. The boxes are the whole point of the
    // notation, and the old renderer dropped them to literal brackets.
    const c = drawn('- [x] built\n- [ ] shipped');
    const boxes = [...c.querySelectorAll('input[type="checkbox"]')];
    expect(boxes).toHaveLength(2);
    expect((boxes[0] as HTMLInputElement).checked).toBe(true);
    expect((boxes[1] as HTMLInputElement).checked).toBe(false);
  });

  it('draws a strikethrough the agent meant', () => {
    const c = drawn('~~the first approach~~ did not work');
    expect(c.querySelector('del')?.textContent).toBe('the first approach');
  });

  it('leaves a snake_case identifier alone', () => {
    /*
     * The property the old renderer went out of its way to hold and this one
     * gets from the specification: `_` inside a word is not emphasis. An
     * identifier with an italic middle is one somebody is about to copy
     * wrong.
     */
    const c = drawn('call granted_services_for(session) next');
    expect(c.querySelector('em')).toBeNull();
    expect(c.textContent).toContain('granted_services_for');
  });

  it('draws a link in the prose ink, never in the browser blue', () => {
    /*
     * §18.1 forbids colour on agent content without exception. Without an
     * explicit `color: inherit` the browser's own blue wins — that exception
     * arriving by habit rather than by decision, and invisible to every test
     * that reads text rather than computed style.
     */
    const css = readCss();
    expect(/\.agent__link\s*\{([^}]*)\}/.exec(css)?.[1] ?? '').toContain('color: inherit');
  });

  it('refuses to open a scheme a reader cannot act on', () => {
    // §9: this is the least trusted text on the surface. The words survive;
    // only the navigation is withheld.
    const c = drawn('[click me](javascript:alert(1))');
    expect(c.querySelector('a')).toBeNull();
    expect(c.textContent).toContain('click me');
  });

  it('keeps a quoted list a list', () => {
    // A quote holds blocks. Flattening it to one paragraph would be this
    // surface deciding what the agent quoted.
    const c = drawn('> - first\n> - second');
    expect(c.querySelectorAll('blockquote li')).toHaveLength(2);
  });
});
