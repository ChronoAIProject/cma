import { expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import {
  completed,
  events,
  harness,
  item,
  request,
  responseId,
  sse,
  wire,
} from './responses-v2-fixtures.js';

function activityEvents(status = 'completed', recorded?: { text: string; truncated: boolean }) {
  const activity = {
    type: 'mcp_call',
    id: 'mcp_call_1',
    server_label: 'cma',
    name: 'Check proof',
    arguments: '',
    output: null,
    error: null,
    status: 'in_progress',
  };
  const output = { ...activity, status, ...(recorded ? { 'cma:output': recorded } : {}) };
  const base = events();
  return [
    ...base.slice(0, 2),
    { type: 'response.output_item.added', output_index: 0, item: activity },
    { type: 'response.mcp_call.in_progress', output_index: 0, item_id: activity.id },
    ...base.slice(2, 8).map((event) => ({ ...event, output_index: 1 })),
    { type: `response.mcp_call.${status}`, output_index: 0, item_id: activity.id },
    { type: 'response.output_item.done', output_index: 0, item: output },
    { type: 'response.completed', response: { ...completed, output: [output, item] } },
  ].map((event, sequence_number) => ({ ...event, sequence_number }));
}
it('retains bounded recorded output without changing the activity lifecycle', async () => {
  const recorded = { text: 'native-computer-proof\n', truncated: false };
  const frames = activityEvents('completed', recorded);
  const fixture = harness();
  fixture.fetch.mockResolvedValueOnce(sse(wire(frames)));
  const result = await fixture.client.responsesV2.create(request, { onEvent: fixture.onEvent });
  expect(result.output[0]).toMatchObject({
    status: 'completed',
    output: null,
    'cma:output': recorded,
  });
});
it.each(['completed', 'failed'])(
  'accepts %s activity interleaved with text and every replay cursor',
  async (status) => {
    const frames = activityEvents(status);
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse(wire(frames)));
    const result = await t.client.responsesV2.create(request, { onEvent: t.onEvent });
    expect(result.output[0]).toMatchObject({
      type: 'mcp_call',
      status,
      arguments: '',
      output: null,
      error: null,
    });
    for (let cursor = 0; cursor < frames.length; cursor++) {
      t.onEvent.mockClear();
      t.fetch.mockResolvedValueOnce(sse(wire(frames.slice(cursor + 1))));
      await t.client.responsesV2.replay(responseId, { startingAfter: cursor, onEvent: t.onEvent });
      expect(t.onEvent.mock.calls.map(([event]) => event)).toEqual(frames.slice(cursor + 1));
    }
  },
);
it.each([
  [3, { item_id: 'foreign' }],
  [3, { type: 'response.mcp_call.completed' }],
  [10, { type: 'response.mcp_call.in_progress' }],
  [
    11,
    {
      item: {
        type: 'message',
        id: 'mcp_call_1',
        role: 'assistant',
        status: 'completed',
        content: item.content,
      },
    },
  ],
  [12, { response: { ...completed, output: [item] } }],
])('rejects inconsistent activity at event %s', async (index, override) => {
  const frames: Record<string, unknown>[] = activityEvents();
  frames[index as number] = { ...frames[index as number], ...(override as object) };
  const t = harness();
  t.fetch.mockResolvedValueOnce(sse(wire(frames)));
  await expect(t.client.responsesV2.create(request, { onEvent: t.onEvent })).rejects.toBeInstanceOf(
    ProtocolError,
  );
});
