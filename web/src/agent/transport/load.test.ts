import { waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AGENT_TRANSCRIPT } from '../../test/agent';
import { item, json, page } from './controller.fixture';
import { deferred, setup } from './load.test.fixture';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('shows the last reply immediately without replaying historical Thinking as event pages hydrate', async () => {
  const h = await setup();
  const remaining = deferred<Response>();
  vi.mocked(h.client.readAgentTranscript).mockResolvedValue(AGENT_TRANSCRIPT);
  h.snapshot
    .mockResolvedValueOnce(json(page([item(1, { type: 'turn_started', turn_sequence: 1 })], 4, 1)))
    .mockReturnValueOnce(remaining.promise);
  const result = h.start();
  await waitFor(() => expect(h.progress).toHaveBeenCalledOnce());
  expect(h.progress.mock.calls[0]![0]).toMatchObject({
    historyLoading: true,
    v2Items: [],
    conversation: {
      activeRunId: undefined,
      messages: [{ content: 'Inspect this repository.' }, { content: 'The repository is ready.' }],
      toolCalls: [],
      approvals: [],
    },
  });
  remaining.resolve(
    json(
      page(
        [
          item(2, { type: 'message', role: 'user', content: 'Inspect this repository.' }),
          item(3, { type: 'message', role: 'assistant', content: 'The repository is ready.' }),
          item(4, { type: 'turn_completed', turn_sequence: 1 }),
        ],
        4,
      ),
    ),
  );
  expect(await result).toMatchObject({
    value: {
      historyLoading: false,
      conversation: { cursor: 4, activeRunId: undefined },
      v2Items: expect.arrayContaining([expect.objectContaining({ sequence: 4 })]),
    },
  });
  expect(h.progress).toHaveBeenCalledOnce();
  expect(
    new URL(h.snapshot.mock.calls[0]![0], 'https://fixture.invalid').searchParams.get('limit'),
  ).toBe('200');
  expect(
    new URL(h.snapshot.mock.calls[1]![0], 'https://fixture.invalid').searchParams.get('limit'),
  ).toBe('200');
  expect(h.snapshot.mock.calls[1]![0]).toContain('page_cursor=ags1.');
});

it('uses the bounded 200-row page for a history that crosses one continuation', async () => {
  const h = await setup();
  const first = Array.from({ length: 200 }, (_, index) => item(index + 1, null));
  h.snapshot.mockImplementation(async (url) => {
    const request = new URL(url, 'https://fixture.invalid');
    return request.searchParams.has('page_cursor')
      ? json(page([item(201, null)], 201))
      : json(page(first, 201, 200));
  });

  const result = await h.start();

  expect(result.error).toBeUndefined();
  expect(result.value?.v2Items).toHaveLength(201);
  expect(h.snapshot).toHaveBeenCalledTimes(2);
  for (const [url] of h.snapshot.mock.calls)
    expect(new URL(url, 'https://fixture.invalid').searchParams.get('limit')).toBe('200');
});

it('uses the current transcript active turn rather than a historical turn start', async () => {
  const h = await setup();
  vi.mocked(h.client.readAgentTranscript).mockResolvedValue({
    ...AGENT_TRANSCRIPT,
    throughSeq: 5,
    turns: [
      ...AGENT_TRANSCRIPT.turns,
      {
        turnSequence: 5,
        startedAt: '2026-10-22T00:00:00Z',
        ownerMessage: '',
        agentMessages: [],
        messages: [],
        outcome: { type: 'active' },
      },
    ],
  });
  h.snapshot
    .mockResolvedValueOnce(json(page([item(1, { type: 'turn_started', turn_sequence: 1 })], 5, 1)))
    .mockResolvedValueOnce(json(page([item(2, null)], 5, 2)))
    .mockResolvedValueOnce(
      json(
        page(
          [
            item(3, null),
            item(4, { type: 'turn_completed', turn_sequence: 1 }),
            item(5, { type: 'turn_started', turn_sequence: 5 }),
          ],
          5,
        ),
      ),
    );
  const result = await h.start();
  expect(result.error).toBeUndefined();
  expect(h.progress).toHaveBeenCalledOnce();
  expect(h.progress.mock.calls[0]![0].conversation.activeRunId).toBe('5');
  expect(result.value?.conversation.activeRunId).toBe('5');
  expect(result.value?.v2Items).toHaveLength(5);
});

it('refreshes a fixed event prefix if the parallel transcript read observed newer history', async () => {
  const h = await setup();
  vi.mocked(h.client.readAgentTranscript).mockResolvedValue(AGENT_TRANSCRIPT);
  h.snapshot
    .mockResolvedValueOnce(json(page([], 0)))
    .mockResolvedValueOnce(
      json(
        page(
          [
            item(1, { type: 'turn_started', turn_sequence: 1 }),
            item(2, { type: 'message', role: 'user', content: 'Inspect this repository.' }),
            item(3, { type: 'message', role: 'assistant', content: 'The repository is ready.' }),
            item(4, { type: 'turn_completed', turn_sequence: 1 }),
          ],
          4,
        ),
      ),
    );
  const result = await h.start();
  expect(result.error).toBeUndefined();
  expect(h.snapshot).toHaveBeenCalledTimes(2);
  expect(result.value?.conversation.cursor).toBe(4);
  expect(result.value?.conversation.activeRunId).toBeUndefined();
});

it('retains the latest transcript without starting replay if event history has not caught up', async () => {
  const h = await setup();
  vi.mocked(h.client.readAgentTranscript).mockResolvedValue(AGENT_TRANSCRIPT);
  h.snapshot.mockImplementation(async () => json(page([], 0)));
  const result = await h.start(true);
  expect(result.error).toBeUndefined();
  expect(result.value).toMatchObject({
    historyUnavailable: true,
    historyLoading: false,
    conversation: { cursor: 4, activeRunId: undefined },
  });
  expect(result.value?.conversation.messages.at(-1)?.content).toBe('The repository is ready.');
  expect(result.value?.conversation.agentCursor).toBeUndefined();
});
