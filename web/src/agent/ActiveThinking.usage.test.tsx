import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { V2AgentCursor, V2AgentEventKind } from '@chronoai/cma-sdk';
import { MessageStream } from './MessageStream';
import { UsageFooter } from './UsageFooter';
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
    <UsageFooter usage={conversation.usage} />
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
  '%s usage events display real turn counts, keep cumulative totals, and reset only the next turn',
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
    expect(screen.getByLabelText('Total input tokens: 27,801')).toHaveTextContent('27,801 input');
    const input = screen.getByLabelText('Turn input tokens: 13,963');
    await act(() => vi.advanceTimersByTimeAsync(200));
    const midway = Number(input.textContent?.replaceAll(',', '').split(' ')[0]);
    expect(midway).toBeGreaterThan(0);
    expect(midway).toBeLessThan(13963);
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(input).toHaveTextContent('13,963 input');
    expect(screen.getByLabelText('Turn output tokens: 20')).toHaveTextContent('20 output');
    view.rerender(draw(start(observed, 32)));
    expect(screen.queryByLabelText('Turn input tokens: 13,963')).not.toBeInTheDocument();
    expect(
      screen.getByRole('region', { name: 'What the agent is working through' }),
    ).toHaveTextContent('Turn input unavailable · output unavailable');
    expect(screen.getByLabelText('Total output tokens: 107')).toHaveTextContent('107 output');
  },
);

it('keeps legacy context-only reports distinct from real token counts and animates actual totals', async () => {
  vi.useFakeTimers();
  const started = start(empty(), 24);
  const legacy = foldAgentEvent(started, {
    seq: 25,
    appendedAt: at,
    kind: { type: 'usage', input_tokens: 0, output_tokens: 0, context_used: 23657 },
  });
  const view = render(draw(legacy));
  expect(
    screen.getByRole('region', { name: 'What the agent is working through' }),
  ).toHaveTextContent('Turn input unavailable · output unavailable');
  const observed = foldAgentEvent(legacy, {
    seq: 26,
    appendedAt: at,
    kind: { type: 'usage', input_tokens: 81, output_tokens: 24, context_used: 0 },
  });
  view.rerender(draw(observed));
  expect(screen.queryByLabelText('Turn input tokens: 81')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(700));
  expect(screen.getByLabelText('Total input tokens: 81')).toHaveTextContent('81 input');
  expect(screen.getByLabelText('Total output tokens: 24')).toHaveTextContent('24 output');
});
