import { expect, it, vi } from 'vitest';
import { countTokens } from 'gpt-tokenizer/encoding/o200k_base';
import {
  newlyVisibleText,
  visibleTokenCounter,
  visibleTurnText,
  VISIBLE_TEXT_LIMIT,
} from './tokenEstimate';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';

const count = (text: string) => countTokens(text, { disallowedSpecial: new Set() });

it('uses real BPE counts for English, Chinese, emoji, code and literal special-token text', () => {
  const estimate = visibleTokenCounter(count);
  for (const text of ['Hello world!', '你好，世界！', '👩‍💻🚀', 'const x = 42;', '<|endoftext|>']) {
    expect(estimate([{ id: 'reply', text }])).toEqual({ tokens: count(text), partial: false });
  }
  expect(estimate([{ id: 'reply', text: 'Hello world!' }]).tokens).toBe(3);
});

it('encodes only changed chunks, replaces reconciled text and drops old cached parts', () => {
  const encode = vi.fn(count);
  const estimate = visibleTokenCounter(encode);
  const prefix = 'x'.repeat(2048);
  estimate([{ id: 'reply', text: prefix + ' first' }]);
  expect(encode).toHaveBeenCalledTimes(2);
  encode.mockClear();
  const next = estimate([{ id: 'reply', text: prefix + ' first second' }]);
  expect(encode).toHaveBeenCalledExactlyOnceWith(' first second');
  expect(next.tokens).toBe(count(prefix) + count(' first second'));
  encode.mockClear();
  estimate([{ id: 'reply', text: prefix + ' first second' }]);
  expect(encode).not.toHaveBeenCalled();
  expect(estimate([{ id: 'durable', text: 'Hello world!' }]).tokens).toBe(3);
  expect(estimate([])).toEqual({ tokens: 0, partial: false });
});

it('bounds work to 65,536 visible code units and marks incomplete estimates', () => {
  const encode = vi.fn(count);
  const estimate = visibleTokenCounter(encode);
  expect(estimate([{ id: 'large', text: 'a'.repeat(VISIBLE_TEXT_LIMIT + 1) }]).partial).toBe(true);
  expect(encode.mock.calls.reduce((size, [text]) => size + text.length, 0)).toBe(
    VISIBLE_TEXT_LIMIT,
  );
  expect(encode.mock.calls.every(([text]) => text.length <= 2048)).toBe(true);
});

it('marks text past an exact boundary in a later reply as partial', () => {
  const empty = conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
  const parts = visibleTurnText({
    ...empty,
    activeRunId: '1',
    messages: [
      {
        id: 'first',
        runId: '1',
        role: 'assistant',
        status: 'complete',
        content: 'a'.repeat(VISIBLE_TEXT_LIMIT),
      },
      { id: 'next', runId: '1', role: 'assistant', status: 'streaming', content: 'later output' },
    ],
  });
  expect(parts.reduce((length, part) => length + part.text.length, 0)).toBe(VISIBLE_TEXT_LIMIT + 1);
  expect(visibleTokenCounter(count)(parts).partial).toBe(true);
});

it('excludes user input, old turns and tool results from the visible output estimate', () => {
  const empty = conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
  expect(
    visibleTurnText({
      ...empty,
      activeRunId: '2',
      messages: [
        { id: 'old', runId: '1', role: 'assistant', status: 'complete', content: 'old output' },
        { id: 'input', runId: '2', role: 'user', status: 'complete', content: 'private input' },
        { id: 'reply', runId: '2', role: 'assistant', status: 'streaming', content: 'Hello' },
      ],
      reasoning: [{ id: 'thought', runId: '2', content: 'Visible reasoning', status: 'streaming' }],
      toolCalls: [
        { id: 'tool', runId: '2', title: 'Tool', status: 'complete', detail: 'tool output' },
      ],
    }),
  ).toEqual([
    { id: 'reply', text: 'Hello' },
    { id: 'reasoning', text: 'Visible reasoning' },
  ]);
  expect(visibleTurnText(empty)).toEqual([]);
});

it('anchors reports before later model text, survives durable reconciliation and ignores same-count replay', () => {
  const empty = conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
  const event = (
    state: typeof empty,
    seq: number,
    kind: { type: string } & Record<string, unknown>,
  ) => foldAgentEvent(state, { seq, appendedAt: '2026-10-15T08:00:00Z', kind });
  const live = (state: typeof empty, text: string) =>
    foldLiveEvent(
      state,
      {
        type: 'TEXT_MESSAGE_CONTENT',
        threadId: 'agent',
        runId: '1',
        messageId: 'live',
        delta: text,
        'cma:live': true,
      },
      'agent',
    );
  const report = {
    type: 'usage_updated',
    input_tokens: 100,
    output_tokens: 8,
    context_used: 108,
    turn_sequence: 1,
    turn_input_tokens: 100,
    turn_output_tokens: 8,
  };
  const started = event(empty, 1, { type: 'turn_started', turn_sequence: 1 });
  const first = event(live(started, 'First reply.'), 3, report);
  const anchor = first.visibleUsageAnchor!;
  expect(newlyVisibleText(visibleTurnText(first), anchor.parts).every((p) => p.text === '')).toBe(
    true,
  );
  const reconciled = event(first, 4, { type: 'agent_message', content: 'First reply.' });
  expect(
    newlyVisibleText(visibleTurnText(reconciled), anchor.parts).every((p) => p.text === ''),
  ).toBe(true);
  const second = live(reconciled, 'Next reply.');
  const replayed = event(second, 5, report);
  expect(replayed.visibleUsageAnchor).toBe(anchor);
  expect(newlyVisibleText(visibleTurnText(replayed), anchor.parts)[0]?.text).toBe('Next reply.');
  const observed = event(replayed, 6, { ...report, output_tokens: 12, turn_output_tokens: 12 });
  expect(
    newlyVisibleText(visibleTurnText(observed), observed.visibleUsageAnchor!.parts).every(
      (p) => p.text === '',
    ),
  ).toBe(true);
  const nextTurn = event(observed, 7, { type: 'turn_started', turn_sequence: 7 });
  expect(visibleTurnText(nextTurn).every((p) => p.text === '')).toBe(true);
  expect(nextTurn.usage?.outputTokens).toBe(12);
});
