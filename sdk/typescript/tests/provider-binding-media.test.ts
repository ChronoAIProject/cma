import { expect, it } from 'vitest';
import {
  ApiError,
  ProtocolError,
  ValidationError,
  type BindingMediaSupplement,
} from '../src/index.js';
import { errorFromBody } from '../src/http.js';
import { transport } from './resource-fixtures.js';

const id = `att_${'a'.repeat(64)}`;
const params = { name: 'voice.ogg', sha256: 'b'.repeat(64), event_expires_at: 1_800_000_000 };
const receipt = {
  id,
  agent_id: 'agt_one',
  name: params.name,
  path: `/home/attachments/agt_one/${id}/${params.name}`,
  size_bytes: 3,
  media_type: 'audio/ogg',
  sha256: params.sha256,
};
const target = ['bnd_one', 'conv_one', 'event_one', 0] as const;
const body: BindingMediaSupplement = {
  event_expires_at: params.event_expires_at,
  artifact: {
    attachment_index: 0,
    kind: 'transcript',
    content_sha256: 'c'.repeat(64),
    source_sha256: params.sha256,
    name: params.name,
    media_type: 'audio/ogg',
    size_bytes: 3,
  },
  derived_text: '  exact transcript\n中文\n',
};
const supplementary = {
  command_id: 'cmd_supplement',
  response_id: 'resp_supplement',
  response_url: '/api/v2/responses/resp_supplement',
  status: 'queued',
  queue_deadline: '2026-10-30T00:00:00Z',
  metadata: { cma_parent_command_id: 'cmd_original' },
};

it('retries original bytes at the same slot and reads the durable receipt without a new command key', async () => {
  const t = transport();
  const file = new Blob([new Uint8Array([0, 255, 10])], { type: 'audio/ogg' });
  t.answer({ error: 'media_continuation_unavailable' }, 503);
  await expect(
    t.client.providerBindings.media.upload(...target, file, params),
  ).rejects.toMatchObject({ status: 503, code: 'media_continuation_unavailable' });
  expect(t.calls).toHaveLength(1);
  t.answer(receipt, 201);
  expect(await t.client.providerBindings.media.upload(...target, file, params)).toEqual(receipt);
  expect(t.calls[0]!.url).toBe(t.calls[1]!.url);
  for (const call of t.calls) {
    expect(call.init.body).toBe(file);
    const headers = new Headers(call.init.headers);
    expect(headers.get('idempotency-key')).toBeNull();
    expect(headers.get('content-length')).toBe('3');
    expect(headers.get('content-type')).toBe('audio/ogg');
    expect(call.url).not.toContain('gateway_binding_generation');
  }
  t.answer(receipt);
  expect(await t.client.providerBindings.media.get(...target)).toEqual(receipt);
  expect(t.calls[2]!.url).not.toContain('?');
  expect(t.calls[2]!.init.body).toBeUndefined();
});

it('preserves exact supplement text and omitted or null Gateway generation on repeat admission', async () => {
  const t = transport();
  t.answer(supplementary, 202);
  for (const request of [
    body,
    { ...body, gateway_binding_generation: null },
    { ...body, gateway_binding_generation: 7 },
  ]) {
    expect(await t.client.providerBindings.media.supplement(...target, request)).toEqual(
      supplementary,
    );
    const call = t.calls.at(-1)!;
    expect(JSON.parse(call.init.body as string)).toEqual(request);
    expect(new Headers(call.init.headers).get('idempotency-key')).toBeNull();
  }
});

it('refuses forged attachment paths and changed upload identity', async () => {
  const t = transport();
  for (const patch of [
    { path: '/tmp/file' },
    { id: 'wrong' },
    { sha256: 'c'.repeat(64) },
    { name: 'other' },
    { size_bytes: 4 },
    { media_type: 'image/png' },
    { agent_id: '../escape', path: `/home/attachments/../escape/${id}/${params.name}` },
  ]) {
    t.answer({ ...receipt, ...patch }, 201);
    await expect(
      t.client.providerBindings.media.upload(
        ...target,
        new Blob(['abc'], { type: 'audio/ogg' }),
        params,
      ),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('refuses malformed upload authority and out-of-slot or oversized text before HTTP', async () => {
  const t = transport();
  for (const patch of [
    { name: '../escape' },
    { sha256: 'wrong' },
    { event_expires_at: 0 },
    { gateway_binding_generation: 0 },
  ]) {
    await expect(
      t.client.providerBindings.media.upload(...target, new Blob([]), { ...params, ...patch }),
    ).rejects.toBeInstanceOf(ValidationError);
  }
  expect(() =>
    t.client.providerBindings.media.supplement(...target, {
      ...body,
      artifact: { ...body.artifact, attachment_index: 1 },
    }),
  ).toThrow(ValidationError);
  expect(() =>
    t.client.providerBindings.media.supplement(...target, {
      ...body,
      derived_text: '字'.repeat(22_000),
    }),
  ).toThrow(ValidationError);
  expect(t.calls).toHaveLength(0);
});

it('recognizes only the documented string refusals and preserves retry metadata', () => {
  const response = new Response(null, {
    status: 503,
    headers: { 'retry-after': '2', 'x-request-id': 'request-one' },
  });
  const error = errorFromBody(response, { error: 'media_continuation_unavailable' });
  expect(error).toBeInstanceOf(ApiError);
  expect(error).toMatchObject({
    code: 'media_continuation_unavailable',
    requestId: 'request-one',
    retryAfterMs: 2000,
  });
  for (const value of [
    { error: 'unknown' },
    { error: 'media_continuation_unavailable', extra: true },
    { error: null },
  ])
    expect(errorFromBody(response, value)).toBeInstanceOf(ProtocolError);
});
