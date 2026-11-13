import { expect, it } from 'vitest';
import { ApiError, ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const deletion = {
  id: 'doc_fixture',
  revision: 4,
  deleted_at: '2026-11-13T10:00:00Z',
  purge: 'pending',
};
const options = { expectedRevision: 3, idempotencyKey: 'delete-fixture' };

it('deletes with the observed revision and preserves the idempotent request', async () => {
  const test = transport();
  for (const purge of ['pending', 'complete', 'blocked']) {
    test.answer({ ...deletion, purge });
    expect(await test.client.documents.delete(deletion.id, options)).toEqual({
      ...deletion,
      purge,
    });
    const call = test.calls.at(-1)!;
    expect(call.url).toBe(`https://cma.example.test/api/v1/documents/${deletion.id}`);
    expect(call.init.method).toBe('DELETE');
    expect(JSON.parse(String(call.init.body))).toEqual({ expected_revision: 3 });
    expect(new Headers(call.init.headers).get('idempotency-key')).toBe('delete-fixture');
  }
});

it('rejects invalid deletion inputs before transport', () => {
  const test = transport();
  for (const expectedRevision of [0, -1, 1.5, Number.NaN]) {
    expect(() =>
      test.client.documents.delete(deletion.id, { ...options, expectedRevision }),
    ).toThrow(ValidationError);
  }
  expect(() =>
    test.client.documents.delete(deletion.id, { ...options, idempotencyKey: '' }),
  ).toThrow(ValidationError);
  expect(test.calls).toHaveLength(0);
});

it('rejects malformed deletion responses and preserves named failures', async () => {
  const test = transport();
  test.answer({ ...deletion, purge: 'unknown' });
  await expect(test.client.documents.delete(deletion.id, options)).rejects.toBeInstanceOf(
    ProtocolError,
  );
  for (const [status, code] of [
    [404, 'document_not_found'],
    [409, 'document_revision_conflict'],
    [409, 'idempotency_key_reused'],
    [503, 'document_deletion_unavailable'],
    [503, 'document_unavailable'],
  ] as const) {
    test.answer({ type: 'about:blank', title: 'Deletion refused', status, code }, status);
    await expect(test.client.documents.delete(deletion.id, options)).rejects.toBeInstanceOf(
      ApiError,
    );
  }
});

it('reports pending deletions independently of quota enforcement', async () => {
  const test = transport();
  for (const pending_deletions of [null, 0, 2]) {
    const usage = {
      enforcement: 'not_enforced',
      documents: null,
      bytes: null,
      objects: null,
      pending_deletions,
    };
    test.answer(usage);
    expect(await test.client.usage.documents()).toEqual(usage);
  }
  test.answer({
    enforcement: 'not_enforced',
    documents: null,
    bytes: null,
    objects: null,
    pending_deletions: -1,
  });
  await expect(test.client.usage.documents()).rejects.toBeInstanceOf(ProtocolError);
});
