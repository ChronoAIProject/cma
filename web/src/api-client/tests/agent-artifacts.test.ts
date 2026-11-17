// @vitest-environment node
import { expect, it } from 'vitest';
import { ApiError, ProtocolError, RequestAborted, ValidationError } from '../index.js';
import { now, transport } from './resource-fixtures.js';

const id = `art_${'a'.repeat(64)}`;
const other = `art_${'b'.repeat(64)}`;
const body = { name: 'Review.txt', media_type: 'text/plain', content_base64: 'cmV2aWV3' } as const;
const metadata = {
  id,
  agent_id: 'agent-1',
  name: body.name,
  media_type: body.media_type,
  size_bytes: 6,
  sha256: 'c'.repeat(64),
  created_at: now,
};

it('publishes explicit content with a retained key, then reads and pages immutable metadata', async () => {
  const test = transport();
  test.answer(metadata, 201);
  expect(
    await test.client.agents.publishArtifact('agent-1', body, { idempotencyKey: 'publish-1' }),
  ).toEqual(metadata);
  expect(test.calls.at(-1)?.init).toMatchObject({ method: 'POST', body: JSON.stringify(body) });
  expect(new Headers(test.calls.at(-1)?.init.headers).get('idempotency-key')).toBe('publish-1');
  test.answer(metadata);
  expect(await test.client.agents.readArtifact('agent-1', id)).toEqual(metadata);
  test.answer({ items: [metadata], next: id });
  expect(await test.client.agents.listArtifacts('agent-1', { limit: 1 })).toEqual({
    items: [metadata],
    next: id,
  });
  test.answer({ items: [{ ...metadata, id: other }] });
  expect(
    (await test.client.agents.listArtifacts('agent-1', { cursor: id, limit: 1 })).items[0]?.id,
  ).toBe(other);
  expect(test.calls.at(-1)?.url).toContain(`/agents/agent-1/artifacts?cursor=${id}&limit=1`);
});

it('refuses changed identities, malformed metadata, repeated pages and mismatched publication receipts', async () => {
  const test = transport();
  for (const patch of [
    { id: other },
    { agent_id: 'foreign' },
    { size_bytes: 0 },
    { size_bytes: 1048577 },
    { sha256: 'no' },
    { name: '../private' },
    { media_type: 'text/html' },
  ]) {
    test.answer({ ...metadata, ...patch });
    await expect(test.client.agents.readArtifact('agent-1', id)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
  for (const page of [
    { items: [metadata, metadata] },
    { items: [metadata], next: other },
    { items: [{ ...metadata, agent_id: 'foreign' }] },
    { items: [], next: id },
  ]) {
    test.answer(page);
    await expect(test.client.agents.listArtifacts('agent-1')).rejects.toBeInstanceOf(ProtocolError);
  }
  test.answer({ ...metadata, name: 'Different.txt' }, 201);
  await expect(
    test.client.agents.publishArtifact('agent-1', body, { idempotencyKey: 'one' }),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('rejects unsafe publication and invalid artifact paging before transport', () => {
  const test = transport();
  for (const patch of [
    { name: '../private' },
    { name: 'Unsafe\nname' },
    { name: 'a'.repeat(161) },
    { media_type: 'image/svg+xml' },
    { content_base64: '' },
    { content_base64: 'YQ' },
    { content_base64: 'YR==' },
    { content_base64: 'YQ==\n' },
  ])
    expect(() =>
      test.client.agents.publishArtifact('agent-1', { ...body, ...patch } as never, {
        idempotencyKey: 'one',
      }),
    ).toThrow(ValidationError);
  for (const options of [{ limit: 0 }, { limit: 51 }, { cursor: 'arbitrary' }])
    expect(() => test.client.agents.listArtifacts('agent-1', options)).toThrow(ValidationError);
  expect(() => test.client.agents.downloadArtifact('agent-1', '../private')).toThrow(
    ValidationError,
  );
  expect(test.calls).toHaveLength(0);
});

it('downloads bytes under exact current authorization and refuses non-byte success or access loss', async () => {
  const test = transport();
  test.fetch.mockImplementation(
    async () => new Response('review', { headers: { 'content-type': 'application/octet-stream' } }),
  );
  expect(await test.client.agents.downloadArtifact('agent-1', id)).toEqual(
    new TextEncoder().encode('review'),
  );
  test.fetch.mockImplementation(
    async () => new Response('<html>Sign in</html>', { headers: { 'content-type': 'text/html' } }),
  );
  await expect(test.client.agents.downloadArtifact('agent-1', id)).rejects.toBeInstanceOf(
    ProtocolError,
  );
  test.fetch.mockImplementation(async () =>
    Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 }),
  );
  await expect(test.client.agents.downloadArtifact('agent-1', id)).rejects.toBeInstanceOf(ApiError);
});

it('cancels a pending artifact byte body without publishing buffered data', async () => {
  const test = transport();
  let cancel = false;
  test.fetch.mockImplementation(
    async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(new Uint8Array([1, 2]));
          },
          cancel() {
            cancel = true;
          },
        }),
        { headers: { 'content-type': 'application/octet-stream' } },
      ),
  );
  const controller = new AbortController();
  const pending = test.client.agents.downloadArtifact('agent-1', id, { signal: controller.signal });
  await new Promise((resolve) => setTimeout(resolve, 0));
  controller.abort();
  await expect(pending).rejects.toBeInstanceOf(RequestAborted);
  expect(cancel).toBe(true);
});
