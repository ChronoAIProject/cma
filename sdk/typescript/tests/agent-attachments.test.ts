import { expect, it } from 'vitest';
import { ProtocolError, RequestAborted, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';
const id = `att_${'a'.repeat(64)}`;
const wire = {
  id,
  agent_id: 'agent-1',
  name: 'review_file.png',
  path: `/home/attachments/agent-1/${id}/review_file.png`,
  size_bytes: 3,
  media_type: 'image/png',
  sha256: 'c'.repeat(64),
};
it('sends original image bytes and MIME under caller key, returns verified persisted path', async () => {
  const t = transport();
  const file = new File([new Uint8Array([0, 255, 10])], 'review file.png', { type: 'image/png' });
  t.answer(wire, 201);
  expect(
    await t.client.agentAttachments.upload('agent-1', file, { idempotencyKey: 'upload-1' }),
  ).toEqual({
    id,
    agentId: 'agent-1',
    name: wire.name,
    path: wire.path,
    size: 3,
    mediaType: 'image/png',
    sha256: wire.sha256,
  });
  const call = t.calls.at(-1)!;
  expect(call.init.body).toBe(file);
  expect(call.url).toContain('/agents/agent-1/attachments?name=review+file.png');
  expect(new Headers(call.init.headers).get('content-type')).toBe('image/png');
  expect(new Headers(call.init.headers).get('idempotency-key')).toBe('upload-1');
});
it('refuses invalid paths, oversized files and missing Blob names before transport', async () => {
  const t = transport();
  for (const name of ['../a', 'a/b', 'a\\b', '..', '']) {
    await expect(
      t.client.agentAttachments.upload('agent-1', new Blob([]), { name, idempotencyKey: 'one' }),
    ).rejects.toBeInstanceOf(ValidationError);
  }
  await expect(
    t.client.agentAttachments.upload('agent-1', new Blob([]), { idempotencyKey: 'one' }),
  ).rejects.toBeInstanceOf(ValidationError);
  await expect(
    t.client.agentAttachments.upload('agent-1', new Blob([new Uint8Array(20 * 1024 * 1024 + 1)]), {
      name: 'large',
      idempotencyKey: 'one',
    }),
  ).rejects.toBeInstanceOf(ValidationError);
  expect(t.calls).toHaveLength(0);
});
it('refuses changed Agent, directory, filename, length, MIME and malformed receipt identities', async () => {
  const t = transport();
  for (const patch of [
    { agent_id: 'other' },
    { path: '/tmp/fake' },
    { name: 'other' },
    { size_bytes: 2 },
    { media_type: 'text/plain' },
    { id: 'att_wrong' },
    { sha256: 'wrong' },
  ]) {
    t.answer({ ...wire, ...patch }, 201);
    await expect(
      t.client.agentAttachments.upload(
        'agent-1',
        new File(['abc'], 'review file.png', { type: 'image/png' }),
        { idempotencyKey: 'one' },
      ),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});
it('supports empty binary files and aborts before transport', async () => {
  const t = transport();
  t.answer(
    {
      ...wire,
      name: 'empty',
      path: `/home/attachments/agent-1/${id}/empty`,
      size_bytes: 0,
      media_type: 'application/octet-stream',
    },
    201,
  );
  expect(
    (
      await t.client.agentAttachments.upload('agent-1', new Blob([]), {
        name: 'empty',
        idempotencyKey: 'one',
      })
    ).size,
  ).toBe(0);
  const abort = new AbortController();
  abort.abort();
  await expect(
    t.client.agentAttachments.upload('agent-1', new File([], 'empty'), {
      idempotencyKey: 'two',
      signal: abort.signal,
    }),
  ).rejects.toBeInstanceOf(RequestAborted);
  expect(t.calls).toHaveLength(1);
});
