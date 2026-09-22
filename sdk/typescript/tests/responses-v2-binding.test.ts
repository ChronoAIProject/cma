import { expect, it } from 'vitest';
import { ProtocolError, ValidationError, type V2EventContext } from '../src/index.js';
import { agentId, completed, harness, request, sse } from './responses-v2-fixtures.js';

const context: V2EventContext = {
  schema_version: 1,
  channel_id: 'channel-independent',
  conversation_id: 'external-conversation',
  event_ref: 'opaque-reference',
  activity: { schema_version: 1, event_id: 'event-1', nested: { full: 'preserved' } },
};
it('accepts mapped conversation aliases and sends the complete immutable context', async () => {
  const t = harness();
  const body = { ...request, conversation: context.conversation_id, event_context: context };
  for (const alias of [undefined, agentId]) {
    t.fetch.mockResolvedValueOnce(sse());
    const options = { idempotencyKey: 'retained-event-key', onEvent: t.onEvent };
    const result =
      alias === undefined
        ? await t.client.responsesV2.create(body, options)
        : await t.client.responsesV2.createForAgent(alias, body, options);
    expect(result).toEqual(completed);
    expect(JSON.parse(String(t.fetch.mock.calls.at(-1)![1]!.body))).toEqual(body);
  }
});
it('reads the full context and rejects mismatched conversation or event identities', async () => {
  const t = harness();
  for (const override of [
    {},
    { conversation_id: 'foreign' },
    { activity: { ...context.activity, event_id: 'foreign' } },
  ]) {
    t.fetch.mockResolvedValueOnce(new Response(JSON.stringify({ ...context, ...override })));
    const result = t.client.responsesV2.readEventContext(
      'binding-id',
      context.conversation_id,
      context.activity.event_id,
    );
    if (!Object.keys(override).length) expect(await result).toEqual(context);
    else await expect(result).rejects.toBeInstanceOf(ProtocolError);
  }
  expect(String(t.fetch.mock.calls[0]![0])).toBe(
    'https://cma.example/api/v1/cmaeg/bindings/binding-id/conversations/external-conversation/events/event-1',
  );
});
it('rejects oversized or malformed context before transport', async () => {
  const t = harness();
  for (const event_context of [
    { ...context, event_ref: '' },
    { ...context, activity: { ...context.activity, huge: 'x'.repeat(1_048_576) } },
  ]) {
    await expect(
      t.client.responsesV2.create({ ...request, event_context }, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ValidationError);
  }
  expect(t.fetch).not.toHaveBeenCalled();
});
