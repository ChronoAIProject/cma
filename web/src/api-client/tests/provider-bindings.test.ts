// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError, ValidationError, type ResetBindingConversation } from '../index.js';
import { transport } from './resource-fixtures.js';

const target = ['binding-one', 'conversation-one'] as const;
const body: ResetBindingConversation = {
  expected_agent_id: 'agt_previous',
  owner: 'owner-one',
  organization: false,
  idempotency_binding_id: target[0],
  request_digest: 'a'.repeat(64),
};
const options = { idempotencyKey: 'b'.repeat(64), requestId: 'reset-one' };
const receipt = {
  binding_id: target[0],
  generation: 1,
  conversation_id: target[1],
  incarnation: 1,
  previous_agent_id: body.expected_agent_id,
  previous_conversation_id: target[1],
  successor_agent_id: 'agt_successor',
  state: 'reserved',
};
const inspection = {
  binding_id: target[0],
  generation: 1,
  conversation_id: target[1],
  incarnation: 0,
  agent_id: body.expected_agent_id,
  state: 'ready',
  agent_status: null,
};

it.each([0, 1, Number.MAX_SAFE_INTEGER])(
  'preserves inspection incarnation %s',
  async (incarnation) => {
    const t = transport();
    const response = { ...inspection, incarnation };
    t.answer(response);
    expect(await t.client.providerBindings.inspectConversation(...target)).toEqual(response);
  },
);

it.each([undefined, null, -1, 0.5, '1', Number.MAX_SAFE_INTEGER + 1])(
  'rejects malformed inspection incarnation %s',
  async (incarnation) => {
    const t = transport();
    t.answer({ ...inspection, incarnation });
    await expect(t.client.providerBindings.inspectConversation(...target)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  },
);

it('sends the exact reset body and retained key on an explicit retry', async () => {
  const t = transport();
  t.answer(receipt);
  for (let attempt = 0; attempt < 2; attempt++)
    expect(await t.client.providerBindings.resetConversation(...target, body, options)).toEqual(
      receipt,
    );
  expect(t.calls).toHaveLength(2);
  for (const { url, init } of t.calls) {
    expect(url).toBe(
      `https://cma.example.test/api/v1/cmaeg/bindings/${target[0]}/conversations/${target[1]}/resets`,
    );
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual(body);
    const headers = new Headers(init.headers);
    expect(headers.get('idempotency-key')).toBe(options.idempotencyKey);
    expect(headers.get('x-request-id')).toBe(options.requestId);
  }
});

it('surfaces disabled reset admission without retrying', async () => {
  const t = transport();
  t.answer({ title: 'Reset disabled', status: 503, code: 'partition_reset_disabled' }, 503);
  await expect(
    t.client.providerBindings.resetConversation(...target, body, options),
  ).rejects.toMatchObject({ status: 503, code: 'partition_reset_disabled' });
  expect(t.calls).toHaveLength(1);
});

it('rejects malformed reset inputs and absent command keys before HTTP', () => {
  const t = transport();
  for (const patch of [
    { expected_agent_id: undefined },
    { owner: null },
    { organization: 'false' },
    { idempotency_binding_id: 1 },
    { request_digest: undefined },
  ])
    expect(() =>
      t.client.providerBindings.resetConversation(
        ...target,
        { ...body, ...patch } as unknown as ResetBindingConversation,
        options,
      ),
    ).toThrow(ValidationError);
  for (const idempotencyKey of ['', 'contains spaces'])
    expect(() =>
      t.client.providerBindings.resetConversation(...target, body, { idempotencyKey }),
    ).toThrow(ValidationError);
  expect(t.calls).toHaveLength(0);
});

it.each([
  { generation: 0 },
  { incarnation: 0 },
  { incarnation: -1 },
  { incarnation: 0.5 },
  { incarnation: Number.MAX_SAFE_INTEGER + 1 },
  { previous_agent_id: undefined },
  { successor_agent_id: null },
  { state: 'ready' },
])('rejects an invalid reset receipt %j', async (patch) => {
  const t = transport();
  t.answer({ ...receipt, ...patch });
  await expect(
    t.client.providerBindings.resetConversation(...target, body, options),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('preserves assigned profile drift alongside conversation identity', async () => {
  const t = transport();
  const response = {
    ...inspection,
    profile_drift: {
      assigned_profile_digest: 'sha256:profile',
      prompt: true,
      skills: false,
      model: true,
    },
  };
  t.answer(response);
  expect(await t.client.providerBindings.inspectConversation(...target)).toEqual(response);
});

it('preserves desired profile drift including the Sandbox service scope', async () => {
  const t = transport();
  const body = { profile: { schema_version: 1 }, profile_digest: 'sha256:profile' };
  const response = {
    profile_digest: body.profile_digest,
    prompt: false,
    skills: true,
    model: false,
    service_scope: true,
  };
  t.answer(response);
  expect(await t.client.providerBindings.inspectDesiredProfile(...target, body)).toEqual(response);
  t.answer({ ...response, service_scope: 'true' });
  await expect(
    t.client.providerBindings.inspectDesiredProfile(...target, body),
  ).rejects.toBeInstanceOf(ProtocolError);
});
