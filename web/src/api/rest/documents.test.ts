import { afterEach, describe, expect, it, vi } from 'vitest';
import { documentsClient } from './documents';
import { newDocumentShareToken } from '../documents';

const id = `doc_${'a'.repeat(32)}`;
const view = {
  id,
  owner_subject: 'owner',
  title: 'A document',
  revision: 3,
  source_revision: 2,
  sha256: 'b'.repeat(64),
  size_bytes: 8,
  created_at: '2026-10-22T00:00:00Z',
  updated_at: '2026-10-22T01:00:00Z',
  origin_agent_id: null,
  access: 'owner',
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
describe('Personal documents public transport', () => {
  it('keeps share authority in its header and refuses redirect forwarding', async () => {
    const fetcher = response({ ...view, access: 'read' });
    const token = newDocumentShareToken();
    expect(token).toMatch(/^[a-f0-9]{64}$/);
    await documentsClient().read(id, { shareToken: token });
    const [url, options] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(`/api/v1/documents/${id}`);
    expect(url).not.toContain(token);
    expect(options).toMatchObject({
      cache: 'no-store',
      credentials: 'same-origin',
      redirect: 'error',
      referrerPolicy: 'no-referrer',
      headers: { 'X-CMA-Document-Share-Token': token },
    });
  });
  it('preserves the exact mutation key and revision without automatic conflict retries', async () => {
    const fetcher = response(
      { code: 'document_revision_conflict', detail: 'Document revision changed' },
      409,
    );
    await expect(
      documentsClient().update(
        id,
        { expected_revision: 3, markdown: '# Edited' },
        { idempotencyKey: 'stable-save' },
      ),
    ).rejects.toMatchObject({ status: 409, code: 'document_revision_conflict' });
    expect(fetcher).toHaveBeenCalledTimes(1);
    const options = (fetcher.mock.calls[0] as unknown as [string, RequestInit])[1];
    expect(options.headers).toMatchObject({ 'Idempotency-Key': 'stable-save' });
    expect(JSON.parse(String(options.body))).toEqual({
      expected_revision: 3,
      markdown: '# Edited',
    });
  });
  it('keeps document metadata revision separate from source revision', async () => {
    response(view);
    await expect(documentsClient().read(id)).resolves.toMatchObject({
      revision: 3,
      source_revision: 2,
    });
    response({ id, revision: 2, markdown: '# source' });
    await expect(documentsClient().source(id, { revision: 2 })).resolves.toMatchObject({
      revision: 2,
    });
    response({ id, revision: 3, markdown: '# source' });
    await expect(documentsClient().source(id, { revision: 2 })).rejects.toThrow(
      'invalid document source',
    );
  });
  it('refuses mismatched identities, oversized sources and invalid listing receipts', async () => {
    response({ ...view, id: `doc_${'c'.repeat(32)}` });
    await expect(documentsClient().read(id)).rejects.toThrow('identity mismatch');
    response({ id, revision: 2, markdown: '界'.repeat(350000) });
    await expect(documentsClient().source(id)).rejects.toThrow('invalid document source');
    response({ items: [view], next: 4 });
    await expect(documentsClient().list()).rejects.toThrow('invalid document page');
  });
  it('sends source mutations and comments through the same public API', async () => {
    const fetcher = response({
      document: view,
      comment: {
        id: 'comment',
        author_subject: 'owner',
        body: 'Review',
        document_revision: 2,
        created_at: view.created_at,
      },
    });
    await documentsClient().comment(
      id,
      { body: 'Review', document_revision: 2 },
      { idempotencyKey: 'one-comment', shareToken: 'd'.repeat(64) },
    );
    expect(fetcher).toHaveBeenCalledWith(
      `/api/v1/documents/${id}/comments`,
      expect.objectContaining({
        method: 'POST',
        redirect: 'error',
        headers: expect.objectContaining({ 'X-CMA-Document-Share-Token': 'd'.repeat(64) }),
        body: JSON.stringify({ body: 'Review', document_revision: 2 }),
      }),
    );
  });
});
