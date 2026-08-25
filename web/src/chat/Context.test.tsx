import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AguiConnection, type AguiEvent } from '@chronoai/cma-sdk';
import { MockSession, transcriptNamed } from '@chronoai/cma-sdk/testing';
import { Context } from './Context';
import { MessageStream } from './MessageStream';
import { fold } from '@chronoai/cma-sdk';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Drive an authored wire transcript through the same parser the surface uses. */
async function conversationFrom(name: string) {
  const session = new MockSession(transcriptNamed(name));
  const seen: AguiEvent[] = [];
  const connection = new AguiConnection(session.transport, session.id, {
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

describe("the conversation's context", () => {
  it('draws nothing where usage was not granted', () => {
    const { container } = render(
      <Context granted={false} usage={{ contextUsed: 42_100, contextWindow: 200_000 }} />,
    );

    expect(container.textContent).toBe('');
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('names a granted session that has not reported a reading yet', () => {
    const { container } = render(<Context granted />);

    expect(screen.getByText('Context · waiting')).toBeInTheDocument();
    expect(container.textContent).not.toMatch(/\b0\b/);
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('shows a reading without inventing a ceiling or a track', async () => {
    const conversation = await conversationFrom('context-without-window');
    const { container } = render(<Context granted usage={conversation.usage} />);

    expect(screen.getByText('42.1K in context')).toBeInTheDocument();
    expect(container.textContent).not.toMatch(/\b0\b/);
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('shows a stated ceiling with an accessible hairline track', async () => {
    const conversation = await conversationFrom('context-with-window');
    const { container } = render(<Context granted usage={conversation.usage} />);

    expect(screen.getByText('42.1K / 200K context')).toBeInTheDocument();
    expect(container.textContent).not.toMatch(/\b0\b/);
    expect(screen.getByRole('progressbar', { name: 'Conversation context used' })).toHaveAttribute(
      'aria-valuenow',
      '42100',
    );
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuemin', '0');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuemax', '200000');
  });

  it('treats a zero reading as absence rather than drawing zero', () => {
    const { container } = render(
      <Context granted usage={{ contextUsed: 0, contextWindow: 200_000 }} />,
    );

    expect(screen.getByText('Context · waiting')).toBeInTheDocument();
    expect(container.textContent).not.toMatch(/\b0\b/);
    expect(screen.queryByRole('progressbar')).toBeNull();
  });
});

describe('the context summary boundary', () => {
  it('states both readings and the ceiling in plain language', async () => {
    const conversation = await conversationFrom('context-compacts');
    render(<MessageStream entries={conversation.entries} />);

    const boundary = screen.getByRole('separator');
    expect(boundary).toHaveTextContent(
      'The agent summarised the conversation to make room. It was holding 198K; it now holds 42.1K of 200K.',
    );
    expect(boundary).not.toHaveTextContent(/^compaction$/i);
  });

  it('still states both readings where no ceiling was reported', () => {
    render(
      <MessageStream
        entries={[
          {
            kind: 'contextCompaction',
            id: 'context-compaction-8',
            was: 198_000,
            now: 42_100,
            seq: 8,
          },
        ]}
      />,
    );

    expect(screen.getByRole('separator')).toHaveTextContent(
      'The agent summarised the conversation to make room. It was holding 198K; it now holds 42.1K.',
    );
  });

  it('uses no warning, failure, or session-state colour', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/chat/MessageStream.css'), 'utf8');
    const rule = /\.context-compaction\s*\{(?<body>[^}]*)\}/.exec(css)?.groups?.body;

    expect(rule).toBeDefined();
    expect(rule).not.toMatch(/--(?:live|wait|fail|ended|accent)|state-(?:warn|error|success)/);
  });
});
