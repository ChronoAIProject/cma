import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { V2AgentCursor, V2AgentEventKind } from '@chronoai/cma-sdk';
import { MessageStream } from './MessageStream';
import { ContextRing } from './ContextRing';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { foldV2Item } from './transport/projection';
import type { ConversationState } from './model';

const at = '2026-10-15T08:00:00Z';
const empty = () => conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
const start = (state: ConversationState, seq: number) =>
  foldAgentEvent(state, {
    seq,
    appendedAt: at,
    kind: { type: 'turn_started', turn_sequence: seq },
  });
const usage: V2AgentEventKind = {
  type: 'usage_updated',
  input_tokens: 27801,
  output_tokens: 107,
  context_used: 13983,
  context_window: 258400,
  turn_sequence: 24,
  turn_input_tokens: 13963,
  turn_output_tokens: 20,
};
const draw = (conversation: ConversationState) => (
  <>
    <MessageStream conversation={conversation} onApproval={vi.fn()} />
    <ContextRing usage={conversation.usage} />
  </>
);

beforeEach(() => {
  vi.stubGlobal(
    'Worker',
    class {
      postMessage() {}
      terminate() {}
    },
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each(['responses', 'agui'] as const)(
  '%s usage events retain real counts outside Thinking and reset only the next turn',
  async (transport) => {
    vi.useFakeTimers();
    const started = start(empty(), 24);
    const observed =
      transport === 'responses'
        ? foldV2Item(started, {
            sequence: 29,
            cursor: `ag1.${'a'.repeat(64)}.29` as V2AgentCursor,
            appended_at: at,
            response_binding: null,
            kind: usage,
          })
        : foldLiveEvent(
            started,
            {
              type: 'STATE_DELTA',
              threadId: 'agent',
              runId: '24',
              delta: [
                {
                  op: 'replace',
                  path: '/usage',
                  value: {
                    inputTokens: 27801,
                    outputTokens: 107,
                    contextUsed: 13983,
                    contextWindow: 258400,
                    currentTurn: { turnSequence: 24, inputTokens: 13963, outputTokens: 20 },
                  },
                },
              ],
            },
            'agent',
          );
    const view = render(draw(observed));
    expect(screen.queryByLabelText('Turn input tokens: 13,963')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'View activity' }));
    expect(screen.getByRole('button', { name: 'Context usage' })).toHaveAccessibleDescription(
      /27,801 input · 107 output/,
    );
    expect(observed.turnUsage?.['24']).toMatchObject({ inputTokens: 13963, outputTokens: 20 });
    await act(() => vi.advanceTimersByTimeAsync(700));
    expect(screen.queryByLabelText('Turn input tokens: 13,963')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Turn output tokens: 20')).not.toBeInTheDocument();
    view.rerender(draw(start(observed, 32)));
    fireEvent.click(screen.getByRole('button', { name: 'View activity' }));
    expect(screen.queryByLabelText('Turn input tokens: 13,963')).not.toBeInTheDocument();
    expect(
      screen.getByRole('region', { name: 'What the agent is working through' }),
    ).not.toHaveTextContent(/input|output|tokens/);
    expect(screen.getByRole('button', { name: 'Context usage' })).toHaveAccessibleDescription(
      /27,801 input · 107 output/,
    );
  },
);

it('keeps legacy context-only reports distinct from real token counts and reports actual totals', async () => {
  vi.useFakeTimers();
  const started = start(empty(), 24);
  const legacy = foldAgentEvent(started, {
    seq: 25,
    appendedAt: at,
    kind: { type: 'usage', input_tokens: 0, output_tokens: 0, context_used: 23657 },
  });
  const view = render(draw(legacy));
  expect(screen.queryByText('Turn input unavailable · output unavailable')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'View activity' }));
  expect(
    screen.getByRole('region', { name: 'What the agent is working through' }),
  ).not.toHaveTextContent(/input|output|tokens/);
  const observed = foldAgentEvent(legacy, {
    seq: 26,
    appendedAt: at,
    kind: { type: 'usage', input_tokens: 81, output_tokens: 24, context_used: 0 },
  });
  view.rerender(draw(observed));
  expect(screen.queryByLabelText('Turn input tokens: 81')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(700));
  expect(screen.getByRole('button', { name: 'Context usage' })).toHaveAccessibleDescription(
    /81 input · 24 output/,
  );
});
