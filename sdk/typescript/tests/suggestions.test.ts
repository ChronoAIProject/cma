import { expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

it('reads saved scoped suggestions without sending input and rejects a different answer', async () => {
  const test = transport();
  const scope = { turn_sequence: 3, answer_sequence: 5 };
  const result = {
    status: 'ready',
    set: {
      id: 'saved',
      viewerId: 'owner',
      agentId: 'agent',
      answerId: 'event:5',
      scope,
      source: { kind: 'generated', version: 'contextual-v1' },
      items: [{ id: 'saved:0', text: 'Explain SQLite backup options.', rank: 0 }],
    },
  };
  test.answer(result);
  expect(await test.client.agents.suggestions('agent', scope)).toEqual(result);
  expect(test.calls[0]?.init.method).toBe('GET');
  expect(test.calls[0]?.url).toContain(
    '/agents/agent/suggestions?turn_sequence=3&answer_sequence=5',
  );
  expect(test.calls[0]?.init.body).toBeUndefined();
  test.answer({ ...result, set: { ...result.set, answerId: 'event:6' } });
  await expect(test.client.agents.suggestions('agent', scope)).rejects.toBeInstanceOf(
    ProtocolError,
  );
});
