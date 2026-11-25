// @vitest-environment node
import { expect, it } from 'vitest';
import {
  ApiError,
  ProtocolError,
  ValidationError,
  type ResetBindingConversation,
} from '../index.js';
import { transport } from './resource-fixtures.js';

const body: ResetBindingConversation = {
  expected_agent_id: 'agt_previous',
  owner: 'owner',
  organization: false,
  idempotency_binding_id: 'binding/one',
  request_digest: 'a'.repeat(64),
};
const options = { idempotencyKey: 'b'.repeat(64), requestId: 'reset-request' };
const receipt = {
  binding_id: 'binding/one',
  generation: 2,
  conversation_id: 'conversation/one',
  incarnation: 1,
  previous_agent_id: body.expected_agent_id,
  previous_conversation_id: 'conversation/one',
  successor_agent_id: 'agt_successor',
  state: 'reserved',
};

it('retains the exact reset intent and key across caller retries with encoded resource paths', async () => {
  const t = transport();
  t.answer(receipt);
  for (let attempt = 0; attempt < 2; attempt++) {
    expect(
      await t.client.providerBindings.resetConversation(
        receipt.binding_id,
        receipt.conversation_id,
        body,
        options,
      ),
    ).toEqual(receipt);
    expect(t.calls).toHaveLength(attempt + 1);
  }
  for (const call of t.calls) {
    expect(call.url).toBe(
      'https://cma.example.test/api/v1/cmaeg/bindings/binding%2Fone/conversations/conversation%2Fone/resets',
    );
    expect(call.init.method).toBe('POST');
    expect(JSON.parse(call.init.body as string)).toEqual(body);
    expect(new Headers(call.init.headers).get('idempotency-key')).toBe(options.idempotencyKey);
  }
});

it.each([409, 503])(
  'preserves a typed HTTP %i refusal without replaying the reset',
  async (status) => {
    const t = transport();
    const code = status === 409 ? 'partition_reset_key_conflict' : 'provider_unavailable';
    t.answer({ code, status, title: 'reset refused' }, status);
    const request = t.client.providerBindings.resetConversation(
      'binding',
      'conversation',
      body,
      options,
    );
    await expect(request).rejects.toBeInstanceOf(ApiError);
    await expect(request).rejects.toMatchObject({ status, code });
    expect(t.calls).toHaveLength(1);
  },
);

it('rejects malformed input and missing command keys before any reset request', () => {
  const t = transport();
  expect(() =>
    t.client.providerBindings.resetConversation('binding', 'conversation', body, {
      idempotencyKey: '',
    }),
  ).toThrow(ValidationError);
  expect(() =>
    t.client.providerBindings.resetConversation(
      'binding',
      'conversation',
      { ...body, organization: 'false' } as unknown as ResetBindingConversation,
      options,
    ),
  ).toThrow(ValidationError);
  expect(t.calls).toHaveLength(0);
});

it.each([
  { incarnation: 0 },
  { incarnation: -1 },
  { incarnation: Number.MAX_SAFE_INTEGER + 1 },
  { generation: 0 },
  { successor_agent_id: undefined },
  { state: 'ready' },
])('refuses malformed reset receipts %j', async (patch) => {
  const t = transport();
  t.answer({ ...receipt, ...patch });
  await expect(
    t.client.providerBindings.resetConversation('binding', 'conversation', body, options),
  ).rejects.toBeInstanceOf(ProtocolError);
  expect(t.calls).toHaveLength(1);
});

const inspection = {
  binding_id: 'binding',
  generation: 1,
  conversation_id: 'conversation',
  agent_id: 'agt_current',
  state: 'ready',
  agent_status: 'ready',
};

it.each([0, 1, Number.MAX_SAFE_INTEGER])(
  'preserves inspection incarnation %i',
  async (incarnation) => {
    const t = transport();
    t.answer({ ...inspection, incarnation });
    expect(await t.client.providerBindings.inspectConversation('binding', 'conversation')).toEqual({
      ...inspection,
      incarnation,
    });
  },
);

it.each([undefined, null, -1, 0.5, '0', Number.MAX_SAFE_INTEGER + 1])(
  'refuses malformed inspection incarnation %j',
  async (incarnation) => {
    const t = transport();
    t.answer({ ...inspection, incarnation });
    await expect(
      t.client.providerBindings.inspectConversation('binding', 'conversation'),
    ).rejects.toBeInstanceOf(ProtocolError);
  },
);
