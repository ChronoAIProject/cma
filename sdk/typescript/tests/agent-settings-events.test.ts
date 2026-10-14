import { expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { agent, now, transport } from './resource-fixtures.js';

it.each(['pre_write_failed', 'write_confirmed'])(
  'reads settings delivery events from a real wire page after %s',
  async (state) => {
    const test = transport();
    const items = ['dispatch_fenced', state].map((delivery, index) => ({
      seq: 26 + index,
      appended_at: now,
      kind: {
        type: 'operation_delivery_changed',
        operation_sequence: 26,
        operation: 'update_settings',
        state: delivery,
      },
    }));
    test.answer({ items, next: 27 });
    expect(await test.client.agents.events(agent.id, { after: 25 })).toEqual({ items, next: 27 });
    expect(test.calls[0]?.init.method).toBe('GET');
  },
);

it.each([
  { operation: 'future_settings_operation', state: 'write_confirmed' },
  { operation: 'update_settings', state: 'future_delivery_state' },
])('continues rejecting unknown operation or delivery values: %j', async (kind) => {
  const test = transport();
  test.answer({
    items: [
      {
        seq: 26,
        appended_at: now,
        kind: { type: 'operation_delivery_changed', operation_sequence: 26, ...kind },
      },
    ],
  });
  await expect(test.client.agents.events(agent.id)).rejects.toBeInstanceOf(ProtocolError);
});
