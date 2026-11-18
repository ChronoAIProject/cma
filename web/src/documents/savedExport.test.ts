import { createHash, webcrypto } from 'node:crypto';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DocumentSource, DocumentView } from '../api/documents';
import { downloadSavedDocument, prepareSavedDocumentExport } from './savedExport';
import { savedDocumentExportEnabled } from './savedExportFeature';

const id = `doc_${'b'.repeat(32)}`;
const markdown = '# Saved 文档 📝\r\n\nA café and a combining e\u0301.';
const metadata: DocumentView = {
  id,
  owner_subject: 'owner',
  title: 'Saved document',
  revision: 7,
  source_revision: 3,
  sha256: createHash('sha256').update(markdown).digest('hex'),
  size_bytes: Buffer.byteLength(markdown),
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  origin_agent_id: 'private-origin',
  access: 'owner',
};
const source: DocumentSource = { id, revision: 3, markdown };
const client = () => ({
  read: vi.fn().mockResolvedValue(metadata),
  source: vi.fn().mockResolvedValue(source),
});
const prepare = (api = client(), signal = new AbortController().signal, viewer = 'owner') =>
  prepareSavedDocumentExport(api, id, viewer, signal);
const blobText = (blob: Blob): Promise<string> =>
  new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });

beforeEach(() => {
  vi.stubGlobal('crypto', webcrypto);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.useRealTimers();
});

it('round trips UTF-8 source with a verified, versioned, allowlisted manifest', async () => {
  const api = client();
  api.read.mockResolvedValue({
    ...metadata,
    comments: [{ author_subject: 'other-user', body: 'Private comment' }],
    shares: [{ subject: 'recipient' }],
    share_token: 'private-link-value',
    audit: { payload: 'private-audit' },
  });
  const signal = new AbortController().signal;
  const result = await prepare(api, signal);
  const contents = JSON.parse(await blobText(result.blob));
  expect(contents).toEqual({
    format: 'cma.saved-document',
    version: 1,
    manifest: {
      title: 'Saved document',
      source_revision: 3,
      content_type: 'text/markdown;charset=utf-8',
      size_bytes: Buffer.byteLength(markdown),
      sha256: createHash('sha256').update(markdown).digest('hex'),
    },
    markdown,
  });
  expect(Buffer.byteLength(contents.markdown)).toBe(contents.manifest.size_bytes);
  expect(createHash('sha256').update(contents.markdown).digest('hex')).toBe(
    contents.manifest.sha256,
  );
  expect(result.blob.type).toBe('application/json;charset=utf-8');
  expect(result.filename).toBe('document-Saved-document-r3.json');
  expect(api.read).toHaveBeenCalledExactlyOnceWith(id, { signal });
  expect(api.source).toHaveBeenCalledExactlyOnceWith(id, { revision: 3, signal });
});

it.each([{ access: 'read' }, { access: 'comment' }, { owner_subject: 'another-owner' }] as const)(
  'refuses shared or cross-owner metadata before source retrieval: %j',
  async (change) => {
    const api = client();
    api.read.mockResolvedValue({ ...metadata, ...change });
    await expect(prepare(api)).rejects.toThrow('Only documents you own');
    expect(api.source).not.toHaveBeenCalled();
  },
);

it('refuses an empty viewer even when the returned owner is empty', async () => {
  const api = client();
  api.read.mockResolvedValue({ ...metadata, owner_subject: '' });
  await expect(prepare(api, new AbortController().signal, '')).rejects.toThrow('Only documents');
});

it.each([
  { id: 'different-document' },
  { title: undefined },
  { title: ' ' },
  { title: '文'.repeat(81) },
  { source_revision: undefined },
  { source_revision: 0 },
  { source_revision: 1.5 },
  { source_revision: Number.MAX_SAFE_INTEGER + 1 },
  { size_bytes: undefined },
  { size_bytes: 0 },
  { size_bytes: 1024 * 1024 + 1 },
  { sha256: undefined },
  { sha256: 'invalid' },
])('refuses incomplete or invalid metadata without reading source: %j', async (change) => {
  const api = client();
  api.read.mockResolvedValue({ ...metadata, ...change });
  await expect(prepare(api)).rejects.toThrow('metadata is incomplete or invalid');
  expect(api.source).not.toHaveBeenCalled();
});

it.each([
  { id: 'different-document' },
  { revision: undefined },
  { revision: 4 },
  { markdown: undefined },
  { markdown: 'x'.repeat(1024 * 1024 + 1) },
])('refuses a missing or mismatched source revision (case %#)', async (change) => {
  const api = client();
  api.source.mockResolvedValue({ ...source, ...change });
  await expect(prepare(api)).rejects.toThrow('source does not match');
});

it('refuses a truncated source before hashing', async () => {
  const api = client();
  api.source.mockResolvedValue({ ...source, markdown: '# Partial' });
  const digest = vi.spyOn(crypto.subtle, 'digest');
  await expect(prepare(api)).rejects.toThrow('size check');
  expect(digest).not.toHaveBeenCalled();
});

it('refuses altered content with the same UTF-8 byte count', async () => {
  const api = client();
  api.source.mockResolvedValue({ ...source, markdown: markdown.replace('Saved', 'Other') });
  await expect(prepare(api)).rejects.toThrow('SHA-256 check');
});

it.each(['read', 'source'] as const)('propagates %s failure without a package', async (step) => {
  const api = client();
  const failure = new Error('Authorized read refused');
  api[step].mockRejectedValue(failure);
  await expect(prepare(api)).rejects.toBe(failure);
});

it('propagates unavailable browser hashing without a package', async () => {
  vi.spyOn(crypto.subtle, 'digest').mockRejectedValue(new Error('Hash unavailable'));
  await expect(prepare()).rejects.toThrow('Hash unavailable');
});

it.each(['before', 'read', 'source', 'hash'] as const)(
  'refuses cancellation at %s even when the transport finishes successfully',
  async (step) => {
    const api = client();
    const request = new AbortController();
    if (step === 'before') request.abort();
    if (step === 'read' || step === 'source') {
      api[step].mockImplementation(async () => {
        request.abort();
        return step === 'read' ? metadata : source;
      });
    }
    if (step === 'hash') {
      vi.spyOn(crypto.subtle, 'digest').mockImplementation(async () => {
        request.abort();
        return new ArrayBuffer(32);
      });
    }
    await expect(prepare(api, request.signal)).rejects.toMatchObject({ name: 'AbortError' });
    if (step === 'before') expect(api.read).not.toHaveBeenCalled();
    if (step === 'read') expect(api.source).not.toHaveBeenCalled();
  },
);

it.each(['../../CON:report?*\u0000\u202e', '文档', 'CON', 'x'.repeat(240)])(
  'uses a bounded portable JSON filename for title %s',
  async (title) => {
    const api = client();
    api.read.mockResolvedValue({ ...metadata, title });
    const result = await prepare(api);
    expect(result.filename).toMatch(/^document-[a-zA-Z0-9_-]+-r3\.json$/);
    expect(result.filename.length).toBeLessThanOrEqual(97);
  },
);

it.each([false, true])('releases object URLs and anchors when click fails = %s', (fail) => {
  vi.useFakeTimers();
  const createObjectURL = vi.fn().mockReturnValue('blob:synthetic-export');
  const revokeObjectURL = vi.fn();
  vi.stubGlobal('URL', { createObjectURL, revokeObjectURL });
  const blob = new Blob(['{}']);
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
    this: HTMLAnchorElement,
  ) {
    expect(this.isConnected).toBe(true);
    expect(this.download).toBe('document-example-r3.json');
    expect(this.href).toBe('blob:synthetic-export');
    if (fail) throw new Error('Download failed');
  });
  const run = () => downloadSavedDocument({ blob, filename: 'document-example-r3.json' });
  if (fail) expect(run).toThrow('Download failed');
  else run();
  expect(createObjectURL).toHaveBeenCalledExactlyOnceWith(blob);
  expect(click).toHaveBeenCalledOnce();
  expect(document.querySelector('a[download]')).toBeNull();
  expect(revokeObjectURL).not.toHaveBeenCalled();
  vi.runAllTimers();
  expect(revokeObjectURL).toHaveBeenCalledExactlyOnceWith('blob:synthetic-export');
});

it.each([undefined, '', 'false', '1', 'TRUE', 'true'])(
  'enables saved export only with the explicit true flag (%s)',
  (value) => {
    vi.stubEnv('VITE_SAVED_DOCUMENT_EXPORT_ENABLED', value);
    expect(savedDocumentExportEnabled()).toBe(value === 'true');
  },
);
