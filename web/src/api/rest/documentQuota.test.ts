import { afterEach, describe, expect, it, vi } from 'vitest';
import { documentsClient } from './documents';
import { DocumentError } from '../documents';

const id = `doc_${'a'.repeat(32)}`;
const deletion = {
  id,
  revision: 4,
  deleted_at: '2026-11-13T00:00:00Z',
  purge: 'pending',
};
const usage = {
  enforcement: 'enforced',
  documents: { used: 37, limit: 100 },
  bytes: { used: 210 * 1024 * 1024, limit: 1024 * 1024 * 1024 },
  objects: { used: 49, limit: 32768 },
  pending_deletions: 2,
};

afterEach(() => vi.unstubAllGlobals());

function response(body: unknown, status = 200) {
  const fetcher = vi.fn(
    async () =>
      new Response(JSON.stringify(body), {
        status,
        headers: { 'Content-Type': 'application/json' },
      }),
  );
  vi.stubGlobal('fetch', fetcher);
  return fetcher;
}

describe('Document usage public transport', () => {
  it('reads all enforced counters and pending deletions through public REST', async () => {
    const fetcher = response(usage);
    const controller = new AbortController();
    await expect(documentsClient().documentUsage({ signal: controller.signal })).resolves.toEqual(
      usage,
    );
    expect(fetcher).toHaveBeenCalledWith(
      '/api/v1/usage/documents',
      expect.objectContaining({
        method: 'GET',
        signal: controller.signal,
        cache: 'no-store',
        credentials: 'same-origin',
        redirect: 'error',
        referrerPolicy: 'no-referrer',
      }),
    );
  });

  it('preserves null counters and pending deletions without inventing zero usage', async () => {
    const notEnforced = {
      enforcement: 'not_enforced',
      documents: null,
      bytes: null,
      objects: null,
      pending_deletions: null,
    };
    response(notEnforced);
    await expect(documentsClient().documentUsage()).resolves.toEqual(notEnforced);
    response({ ...notEnforced, enforcement: 'enforced', pending_deletions: 4 });
    await expect(documentsClient().documentUsage()).resolves.toMatchObject({
      documents: null,
      pending_deletions: 4,
    });
  });

  it('accepts zero and existing usage above the limit', async () => {
    response({ ...usage, documents: { used: 101, limit: 100 }, pending_deletions: 0 });
    await expect(documentsClient().documentUsage()).resolves.toMatchObject({
      documents: { used: 101, limit: 100 },
      pending_deletions: 0,
    });
  });

  it.each(
    [
      null,
      [],
      { ...usage, enforcement: 'unknown' },
      { ...usage, enforcement: ['enforced'] },
      { ...usage, documents: undefined },
      { ...usage, documents: { used: -1, limit: 100 } },
      { ...usage, bytes: { used: 0.5, limit: 100 } },
      { ...usage, objects: { used: 1, limit: '100' } },
      { ...usage, bytes: { used: Number.MAX_SAFE_INTEGER + 1, limit: 100 } },
      { ...usage, pending_deletions: undefined },
      { ...usage, pending_deletions: -1 },
      { ...usage, pending_deletions: '2' },
    ].map((body) => ({ body })),
  )('refuses malformed usage rather than reporting available capacity: %j', async ({ body }) => {
    response(body);
    await expect(documentsClient().documentUsage()).rejects.toThrow('invalid document usage');
  });

  it.each([
    [401, 'not_authenticated'],
    [404, 'document_not_found'],
    [503, 'document_unavailable'],
  ])('preserves a typed usage refusal with status %i and code %s', async (status, code) => {
    const fetcher = response({ code, detail: 'Usage unavailable' }, status as number);
    await expect(documentsClient().documentUsage()).rejects.toMatchObject({
      status,
      code,
    });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
});

describe('Document deletion public transport', () => {
  it.each(['pending', 'complete', 'blocked'])(
    'validates the %s deletion receipt',
    async (purge) => {
      const fetcher = response({ ...deletion, purge });
      const controller = new AbortController();
      await expect(
        documentsClient().deleteDocument(id, {
          expectedRevision: 3,
          idempotencyKey: 'stable-delete',
          signal: controller.signal,
        }),
      ).resolves.toEqual({ ...deletion, purge });
      expect(fetcher).toHaveBeenCalledWith(
        `/api/v1/documents/${id}`,
        expect.objectContaining({
          method: 'DELETE',
          signal: controller.signal,
          headers: {
            Accept: 'application/json',
            'Content-Type': 'application/json',
            'Idempotency-Key': 'stable-delete',
          },
          body: JSON.stringify({ expected_revision: 3 }),
          credentials: 'same-origin',
          redirect: 'error',
          cache: 'no-store',
          referrerPolicy: 'no-referrer',
        }),
      );
    },
  );

  it.each([
    [400, 'invalid_document_request'],
    [401, 'not_authenticated'],
    [403, 'forbidden'],
    [404, 'document_not_found'],
    [409, 'document_revision_conflict'],
    [409, 'idempotency_key_reused'],
    [503, 'document_deletion_unavailable'],
    [503, 'document_unavailable'],
  ])('preserves deletion refusal %i %s without retrying', async (status, code) => {
    const fetcher = response({ code, detail: 'Deletion refused' }, status as number);
    await expect(
      documentsClient().deleteDocument(id, {
        expectedRevision: 3,
        idempotencyKey: 'stable-delete',
      }),
    ).rejects.toMatchObject({ status, code, message: 'Deletion refused' });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it('lets the caller replay an uncertain deletion with the original key and revision', async () => {
    const fetcher = vi
      .fn()
      .mockRejectedValueOnce(new TypeError('Connection lost'))
      .mockResolvedValueOnce(new Response(JSON.stringify(deletion), { status: 200 }));
    vi.stubGlobal('fetch', fetcher);
    const client = documentsClient();
    const options = { expectedRevision: 3, idempotencyKey: 'stable-delete' };
    await expect(client.deleteDocument(id, options)).rejects.toThrow('Connection lost');
    await expect(client.deleteDocument(id, options)).resolves.toEqual(deletion);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(fetcher.mock.calls[0]).toEqual(fetcher.mock.calls[1]);
  });

  it.each([
    null,
    { ...deletion, id: `doc_${'b'.repeat(32)}` },
    { ...deletion, revision: 3 },
    { ...deletion, revision: 0 },
    { ...deletion, deleted_at: 1 },
    { ...deletion, deleted_at: '' },
    { ...deletion, purge: 'unknown' },
    { ...deletion, purge: ['pending'] },
    { ...deletion, purge: undefined },
  ])('refuses an invalid deletion receipt: %j', async (body) => {
    response(body);
    await expect(
      documentsClient().deleteDocument(id, {
        expectedRevision: 3,
        idempotencyKey: 'stable-delete',
      }),
    ).rejects.toThrow('confirm this document deletion');
  });
});

describe('Document quota refusals', () => {
  it.each([
    'document_limit_reached',
    'document_bytes_limit_reached',
    'document_objects_limit_reached',
    'document_write_busy',
  ])('preserves %s and server quota context on a source write', async (code) => {
    response({ code, detail: 'Write refused', limit: 100, live: 101 }, 429);
    const failure = documentsClient().update(
      id,
      { expected_revision: 3, markdown: '# Draft' },
      { idempotencyKey: 'stable-save' },
    );
    await expect(failure).rejects.toBeInstanceOf(DocumentError);
    await expect(failure).rejects.toMatchObject({ status: 429, code, limit: 100, live: 101 });
  });

  it('does not expose malformed quota context as usable capacity', async () => {
    response({ code: 'document_limit_reached', limit: '100', live: -1 }, 429);
    await expect(
      documentsClient().update(
        id,
        { expected_revision: 3, markdown: '# Draft' },
        { idempotencyKey: 'stable-save' },
      ),
    ).rejects.toMatchObject({ limit: undefined, live: undefined });
  });
});
