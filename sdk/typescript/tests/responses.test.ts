import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError, ValidationError } from '../src/index.js';
import { responseEvent } from '../src/responses/events.js';
import { transport } from './resource-fixtures.js';
import {
  agentId,
  responseId,
  response,
  completed,
  frame,
  opening,
  sse,
  harness,
  request,
} from './responses-fixtures.js';

describe('Responses public SDK', () => {
  it('creates and reads the same Agent conversation, with exact metadata and no invented command key', async () => {
    const t = transport();
    const conversation = {
      id: agentId,
      object: 'conversation',
      created_at: 1,
      metadata: { agent_profile: 'agp_default', workspace: 'wks_example', sandbox: 'sbx_example' },
    };
    t.answer(conversation);
    expect(
      await t.client.responses.createConversation({ metadata: { workspace: 'wks_example' } }),
    ).toEqual(conversation);
    expect(t.calls[0]!.init.method).toBe('POST');
    expect(JSON.parse(String(t.calls[0]!.init.body))).toEqual({
      metadata: { workspace: 'wks_example' },
    });
    expect(new Headers(t.calls[0]!.init.headers).has('idempotency-key')).toBe(false);
    expect(await t.client.responses.readConversation(agentId)).toEqual(conversation);
    await expect(t.client.responses.readConversation('different')).rejects.toBeInstanceOf(
      ProtocolError,
    );
  });

  it('retrieves durable responses and validates identity and output state', async () => {
    const t = transport();
    t.answer(completed);
    expect(await t.client.responses.read(responseId)).toEqual(completed);
    await expect(t.client.responses.read('different')).rejects.toBeInstanceOf(ProtocolError);
    t.answer({ ...completed, status: 'invented' });
    await expect(t.client.responses.read(responseId)).rejects.toBeInstanceOf(ProtocolError);
    t.answer({ ...completed, temperature: 'wrong' });
    await expect(t.client.responses.read(responseId)).rejects.toBeInstanceOf(ProtocolError);
  });

  it('reads newest-first conversation items with nullable bounds and provider tool activity', async () => {
    const t = transport();
    const message = {
      id: `msg_${agentId}_1`,
      type: 'message',
      status: 'completed',
      role: 'user',
      content: [{ type: 'input_text', text: 'Hello' }],
    };
    const tool = {
      id: `tool_${agentId}_2`,
      type: 'cma_chat.tool_activity',
      call_id: 'call-1',
      kind: 'read',
      target: null,
      status: 'completed',
      duration_ms: null,
    };
    const page = {
      object: 'list',
      data: [tool, message],
      first_id: tool.id,
      last_id: message.id,
      has_more: true,
    };
    t.answer(page);
    expect(
      await t.client.responses.listConversationItems(agentId, {
        after: 'previous/item',
        limit: 100,
      }),
    ).toEqual(page);
    expect(new URL(t.calls[0]!.url).searchParams.get('after')).toBe('previous/item');
    t.answer({ object: 'list', data: [], first_id: null, last_id: null, has_more: false });
    expect((await t.client.responses.listConversationItems(agentId)).data).toEqual([]);
    t.answer({ ...page, first_id: 'wrong' });
    await expect(t.client.responses.listConversationItems(agentId)).rejects.toBeInstanceOf(
      ProtocolError,
    );
    for (const limit of [0, 101, 1.5])
      await expect(
        t.client.responses.listConversationItems(agentId, { limit }),
      ).rejects.toBeInstanceOf(ValidationError);
  });

  it('submits one stream and acknowledges each event before returning the terminal response', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse());
    expect(await t.client.responses.create(request, { onEvent: t.onEvent })).toEqual(completed);
    expect(t.onEvent.mock.calls.map(([event]) => event.type)).toEqual([
      'response.created',
      'response.completed',
    ]);
    expect(t.fetch).toHaveBeenCalledTimes(1);
    const [url, init] = t.fetch.mock.calls[0]!;
    expect(url).toBe('https://cma.example/api/v1/responses');
    expect(init!.method).toBe('POST');
    expect(JSON.parse(String(init!.body))).toEqual(request);
    expect(new Headers(init!.headers).has('idempotency-key')).toBe(false);
  });

  it.each(['failed', 'incomplete'] as const)(
    'returns the authoritative %s outcome, not a transport error',
    async (status) => {
      const t = harness();
      const outcome = {
        ...response,
        status,
        error: status === 'failed' ? { code: 'runtime_failure', message: 'Turn failed.' } : null,
        incomplete_details: status === 'incomplete' ? { reason: 'interrupted' } : null,
      };
      t.fetch.mockResolvedValueOnce(
        sse(opening + frame(`response.${status}`, 1, { response: outcome })),
      );
      expect(await t.client.responses.create(request, { onEvent: t.onEvent })).toEqual(outcome);
    },
  );

  it('accepts a standalone provisioning failure at sequence zero', async () => {
    const t = harness();
    const outcome = {
      ...response,
      id: `resp_${agentId}_0`,
      status: 'failed',
      error: { code: 'provisioning_timeout', message: 'Agent not ready.' },
    };
    t.fetch.mockResolvedValueOnce(sse(frame('response.failed', 0, { response: outcome })));
    expect((await t.client.responses.create(request, { onEvent: t.onEvent })).error?.code).toBe(
      'provisioning_timeout',
    );
  });

  it.each([
    'hello',
    [{ role: 'user', content: 'hello' }],
    [{ type: 'message', role: 'user', content: [{ type: 'input_text', text: 'hello' }] }],
  ])('supports the server text input form %j', async (input) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse());
    await t.client.responses.create({ ...request, input } as never, { onEvent: t.onEvent });
    expect(t.fetch).toHaveBeenCalledTimes(1);
  });

  it.each([
    { stream: false },
    { store: false },
    { conversation: '' },
    { tools: [] },
    { model: 1 },
    { input: '' },
    { input: [] },
    { input: [{ role: 'assistant', content: 'hello' }] },
    { input: [{ role: 'user', content: [], unexpected: true }] },
    { input: [{ role: 'user', type: 'unknown', content: 'hello' }] },
    { input: [{ role: 'user', content: [{ type: 'input_image', text: 'hello' }] }] },
    { input: [{ role: 'user', content: [{ type: 'input_text', text: '' }] }] },
    { metadata: Object.fromEntries(Array.from({ length: 17 }, (_, i) => [String(i), 'value'])) },
    { metadata: { ['x'.repeat(65)]: 'value' } },
    { metadata: { key: 'x'.repeat(513) } },
  ])('refuses unsupported or malformed input without sending: %j', async (override) => {
    const t = harness();
    await expect(
      t.client.responses.create({ ...request, ...override } as never, { onEvent: t.onEvent }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });

  it('preserves typed unsupported-operation refusals and never retries them', async () => {
    const t = transport();
    t.answer(
      {
        status: 422,
        code: 'responses_field_unsupported',
        field: 'delete',
        detail: 'Durable response deletion is unsupported.',
      },
      422,
    );
    await expect(t.client.responses.delete(responseId)).rejects.toMatchObject({
      code: 'responses_field_unsupported',
      field: 'delete',
    });
    await expect(t.client.responses.cancel(responseId)).rejects.toBeInstanceOf(ApiError);
    expect(t.calls.map((call) => call.init.method)).toEqual(['DELETE', 'POST']);
    t.fetch.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await expect(t.client.responses.delete(responseId)).rejects.toBeInstanceOf(ProtocolError);
    t.fetch.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await expect(t.client.responses.cancel(responseId)).rejects.toBeInstanceOf(ProtocolError);
  });

  it('decodes typed live reasoning and tool extensions without treating them as durable output', () => {
    const item = {
      id: 'rs_example',
      type: 'reasoning',
      status: 'completed',
      summary: [{ type: 'summary_text', text: 'Thought' }],
      'cma:live': true,
    };
    expect(
      responseEvent({
        type: 'response.output_item.done',
        sequence_number: 2,
        output_index: 0,
        item,
      }),
    ).toHaveProperty('item', item);
    expect(
      responseEvent({
        type: 'response.reasoning_summary_text.done',
        sequence_number: 3,
        output_index: 0,
        item_id: item.id,
        summary_index: 0,
        text: 'Thought',
        'cma:truncated': true,
      }),
    ).toHaveProperty('cma:truncated', true);
  });
});
