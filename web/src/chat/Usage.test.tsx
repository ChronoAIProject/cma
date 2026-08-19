import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Usage } from './Usage';
import { fold } from './conversation';
import { AguiConnection, MockSession, transcriptNamed, type AguiEvent } from '../agui';

/**
 * Usage, and the honest absence of it — `Web_Style.md` §18.5.
 */

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

async function conversationFrom(name: string) {
  const session = new MockSession(transcriptNamed(name));
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
  return fold(seen);
}

describe('a session that reports usage', () => {
  it('shows accumulating cost', async () => {
    const conversation = await conversationFrom('reports-usage');
    render(<Usage granted usage={conversation.usage} />);

    // The last figure the session reported, not the first.
    expect(screen.getByText('2,310 tokens')).toBeInTheDocument();
    expect(screen.getByText('0.019')).toBeInTheDocument();
  });

  it('estimates nothing', async () => {
    // A number this interface computed would be a number nobody can reconcile
    // with a bill.
    const conversation = await conversationFrom('reports-usage');
    expect(conversation.usage?.tokens).toBe(2310);
    expect(conversation.usage?.cost).toBe('0.019');
  });
});

describe('a session that cannot report it', () => {
  it('says so, and nothing reads as zero', () => {
    // §18.5, and the assertion this component exists to hold: "a session that
    // cannot report usage must not read as a session that has used nothing."
    const { container } = render(<Usage granted={false} />);

    expect(screen.getByText(/not reported for this session/i)).toBeInTheDocument();
    expect(container.textContent).not.toContain('0');
    expect(container.textContent).not.toMatch(/\btokens\b/);
  });

  it('is not drawn as an empty space either', () => {
    // An absent figure and an absent mechanism are different facts. Drawing
    // nothing at all would be the second dressed as neither.
    const { container } = render(<Usage granted={false} />);
    expect(container.textContent?.trim()).not.toBe('');
  });

  it('tells "not granted" apart from "granted and nothing yet"', async () => {
    const absent = render(<Usage granted={false} />);
    expect(absent.getByText(/not reported for this session/i)).toBeInTheDocument();
    absent.unmount();

    const waiting = render(<Usage granted usage={undefined} />);
    expect(waiting.getByText(/waiting/i)).toBeInTheDocument();
    expect(waiting.queryByText(/not reported for this session/i)).toBeNull();
    // And still nothing reading as zero.
    expect(waiting.container.textContent).not.toContain('0');
  });

  it('reports nothing at all for a session whose transcript carries none', async () => {
    const conversation = await conversationFrom('no-usage');
    expect(conversation.usage).toBeUndefined();

    const { container } = render(<Usage granted={false} usage={conversation.usage} />);
    expect(container.textContent).not.toContain('0');
  });
});

describe('leaving the plane', () => {
  it('drops the figure rather than leaving a stale one on screen', () => {
    // §18.5: "switching a session to the terminal plane does not leave a stale
    // figure on screen." The terminal plane is never granted `usage`, so the
    // grant is the switch, and there is no path by which a number survives it.
    const { rerender, container } = render(
      <Usage granted usage={{ tokens: 4820, cost: '0.041' }} />,
    );
    expect(container.textContent).toContain('4,820');

    rerender(<Usage granted={false} usage={{ tokens: 4820, cost: '0.041' }} />);
    expect(container.textContent).not.toContain('4,820');
    expect(screen.getByText(/not reported for this session/i)).toBeInTheDocument();
  });
});
