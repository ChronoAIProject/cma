import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createHash, webcrypto } from 'node:crypto';
import { expect, it, vi } from 'vitest';
import { DocumentError, type DocumentSource } from '../api/documents';
import { documentsClient } from '../api/rest/documents';
import { DocumentPage } from './DocumentPage';
import { client, id, page, view } from './DocumentPage.testFixture';
import { documentDraft } from './drafts';

function exportBrowser() {
  vi.stubEnv('VITE_SAVED_DOCUMENT_EXPORT_ENABLED', 'true');
  vi.stubGlobal('crypto', webcrypto);
  const createObjectURL = vi.fn().mockReturnValue('blob:saved-document');
  vi.stubGlobal('URL', { createObjectURL, revokeObjectURL: vi.fn() });
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  vi.spyOn(console, 'info').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  return createObjectURL;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: Error) => void;
  const promise = new Promise<T>((success, failure) => {
    resolve = success;
    reject = failure;
  });
  return { promise, resolve, reject };
}

it('leaves the export action absent by default and makes no extra reads', async () => {
  const api = client();
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  expect(screen.queryByRole('button', { name: /Export/ })).not.toBeInTheDocument();
  expect(api.read).toHaveBeenCalledOnce();
  expect(api.source).toHaveBeenCalledOnce();
});

it.each([{ access: 'read' }, { access: 'comment' }, { owner_subject: 'another-owner' }] as const)(
  'hides saved export for shared or cross-owner selections: %j',
  async (change) => {
    exportBrowser();
    page(client({ read: vi.fn().mockResolvedValue({ ...view, ...change }) }));
    await screen.findByRole('textbox', { name: 'Document content' });
    expect(screen.queryByRole('button', { name: /Export/ })).not.toBeInTheDocument();
  },
);

it('exports freshly authorized saved content while preserving the unsaved draft and save baseline', async () => {
  const createObjectURL = exportBrowser();
  const update = vi.fn();
  const api = client({ update });
  page(api);
  const editor = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(editor, { target: { value: '# Unsaved draft' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Document title' }), {
    target: { value: 'Unsaved title' },
  });
  const saved = '# Newer saved content 文档';
  vi.mocked(api.read).mockResolvedValue({
    ...view,
    title: 'Saved title',
    source_revision: 5,
    revision: 7,
    size_bytes: Buffer.byteLength(saved),
    sha256: createHash('sha256').update(saved).digest('hex'),
  });
  vi.mocked(api.source).mockResolvedValue({ id, revision: 5, markdown: saved });
  const before = documentDraft('owner', id);
  const storageWrite = vi.spyOn(Storage.prototype, 'setItem');
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  await vi.waitFor(() => expect(createObjectURL).toHaveBeenCalledOnce());
  const blob: Blob = createObjectURL.mock.calls[0]![0];
  const text = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });
  expect(JSON.parse(text)).toMatchObject({
    markdown: saved,
    manifest: { title: 'Saved title', source_revision: 5 },
  });
  expect(api.read).toHaveBeenLastCalledWith(id, { signal: expect.any(AbortSignal) });
  expect(api.source).toHaveBeenLastCalledWith(id, {
    revision: 5,
    signal: expect.any(AbortSignal),
  });
  expect(editor).toHaveValue('# Unsaved draft');
  expect(documentDraft('owner', id)).toEqual(before);
  expect(documentDraft('owner', id)?.sourceRevision).toBe(1);
  expect(update).not.toHaveBeenCalled();
  expect(storageWrite).not.toHaveBeenCalled();
  const leaving = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(leaving);
  expect(leaving.defaultPrevented).toBe(true);
  vi.advanceTimersByTime(1000);
  expect(URL.revokeObjectURL).toHaveBeenCalledExactlyOnceWith('blob:saved-document');
});

it.each([
  { owner_subject: 'another-owner' },
  { access: 'read' },
  new DocumentError('Denied with private context', 401),
  new DocumentError('Denied with private context', 403),
  new DocumentError('Denied with private context', 404, 'document_not_found'),
] as const)(
  'withdraws the editor and both downloads after metadata confirms loss of access (%#)',
  async (loss) => {
    const createObjectURL = exportBrowser();
    const api = client();
    page(api);
    await screen.findByRole('textbox', { name: 'Document content' });
    if (loss instanceof DocumentError) vi.mocked(api.read).mockRejectedValueOnce(loss);
    else vi.mocked(api.read).mockResolvedValueOnce({ ...view, ...loss });
    fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
    expect(await screen.findByRole('alert')).not.toHaveTextContent('Denied with private context');
    expect(screen.queryByRole('textbox', { name: 'Document content' })).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: 'Document title' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Download draft' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Export/ })).not.toBeInTheDocument();
    expect(createObjectURL).not.toHaveBeenCalled();
    expect(api.source).toHaveBeenCalledOnce();
  },
);

it('allows one export at a time and cancels an in-flight source read on unmount', async () => {
  const createObjectURL = exportBrowser();
  const api = client();
  const onNavigate = vi.fn();
  const mounted = render(
    <DocumentPage client={api} id={id} viewer="owner" onNavigate={onNavigate} />,
  );
  await screen.findByRole('textbox', { name: 'Document content' });
  let finish: ((value: DocumentSource) => void) | undefined;
  vi.mocked(api.source).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  const button = screen.getByRole('button', { name: 'Exporting saved JSON…' });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  await waitFor(() => expect(finish).toBeDefined());
  expect(api.read).toHaveBeenCalledTimes(2);
  const signal = vi.mocked(api.source).mock.calls[1]![1]!.signal!;
  fireEvent.click(screen.getByRole('button', { name: 'Back to artifacts' }));
  expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'artifacts' });
  mounted.unmount();
  expect(signal.aborted).toBe(true);
  await act(async () => finish!({ id, revision: 1, markdown: '# Original' }));
  expect(createObjectURL).not.toHaveBeenCalled();
});

it.each([
  { name: 'checksum failure', reads: 2 },
  { name: 'transient source failure', status: 503, reads: 2 },
  { name: 'missing historical revision', status: 404, reads: 3 },
  { name: 'unconfirmed access after a missing revision', status: 404, failRecheck: true, reads: 3 },
])('keeps draft recovery available after $name', async ({ status, failRecheck, reads }) => {
  const createObjectURL = exportBrowser();
  const api = client();
  page(api);
  const editor = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(editor, { target: { value: '# Recover this draft' } });
  if (status) {
    vi.mocked(api.source).mockRejectedValueOnce(
      new DocumentError(
        'Source unavailable',
        status,
        status === 404 ? 'document_not_found' : undefined,
      ),
    );
  }
  if (failRecheck) {
    vi.mocked(api.read)
      .mockResolvedValueOnce(view)
      .mockRejectedValueOnce(new DocumentError('Metadata unavailable', 503));
  }
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  const download = await screen.findByRole('button', { name: 'Download draft' });
  expect(createObjectURL).not.toHaveBeenCalled();
  expect(api.read).toHaveBeenCalledTimes(reads);
  expect(screen.getByRole('button', { name: 'Export saved JSON' })).toBeEnabled();
  expect(documentDraft('owner', id)?.markdown).toBe('# Recover this draft');
  vi.useFakeTimers();
  fireEvent.click(download);
  expect(createObjectURL).toHaveBeenCalledOnce();
  const blob: Blob = createObjectURL.mock.calls[0]![0];
  expect(blob.type).toBe('text/markdown;charset=utf-8');
  expect(editor).toHaveValue('# Recover this draft');
  vi.runAllTimers();
  expect(URL.revokeObjectURL).toHaveBeenCalledExactlyOnceWith('blob:saved-document');
});

it.each([401, 403, 404])(
  'handles a %s during the source read when access is lost, then allows a fresh reload',
  async (status) => {
    const createObjectURL = exportBrowser();
    const api = client();
    page(api);
    const editor = await screen.findByRole('textbox', { name: 'Document content' });
    fireEvent.change(editor, { target: { value: '# Private draft' } });
    expect(documentDraft('owner', id)?.markdown).toBe('# Private draft');
    let revoke: ((cause: Error) => void) | undefined;
    vi.mocked(api.source).mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          revoke = reject;
        }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
    await waitFor(() => expect(revoke).toBeDefined());
    if (status === 404) {
      vi.mocked(api.read).mockRejectedValueOnce(
        new DocumentError('Document no longer available', 404, 'document_not_found'),
      );
    }
    await act(async () =>
      revoke!(new DocumentError('Source unavailable', status, 'document_not_found')),
    );
    expect(screen.queryByRole('textbox', { name: 'Document content' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Download draft' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Export/ })).not.toBeInTheDocument();
    expect(documentDraft('owner', id)).toBeUndefined();
    expect(createObjectURL).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
    expect(await screen.findByRole('textbox', { name: 'Document content' })).toHaveValue(
      '# Original',
    );
    expect(screen.getByRole('button', { name: 'Export saved JSON' })).toBeEnabled();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(api.read).toHaveBeenCalledTimes(status === 404 ? 4 : 3);
  },
);

it.each(['metadata', 'source'] as const)(
  'ignores a reload waiting on %s when export confirms access loss',
  async (pending) => {
    const createObjectURL = exportBrowser();
    const api = client({
      update: vi.fn().mockRejectedValue(new DocumentError('Revision conflict', 409)),
    });
    page(api);
    const editor = await screen.findByRole('textbox', { name: 'Document content' });
    fireEvent.change(editor, { target: { value: '# Private draft' } });

    const exportSource = deferred<DocumentSource>();
    vi.mocked(api.source).mockReturnValueOnce(exportSource.promise);
    fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
    await waitFor(() => expect(api.source).toHaveBeenCalledTimes(2));

    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByText('Revision conflict');
    fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
    const metadata = deferred<typeof view>();
    const source = deferred<DocumentSource>();
    if (pending === 'metadata') vi.mocked(api.read).mockReturnValueOnce(metadata.promise);
    else vi.mocked(api.source).mockReturnValueOnce(source.promise);
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft and reload' }));
    await waitFor(() => expect(api.read).toHaveBeenCalledTimes(3));
    if (pending === 'source') await waitFor(() => expect(api.source).toHaveBeenCalledTimes(3));
    const reloadSignal = vi.mocked(api.read).mock.calls[2]?.[1]?.signal;

    await act(async () => exportSource.reject(new DocumentError('Access denied', 403)));
    const stale = { id, revision: 1, markdown: '# Earlier response' };
    await act(async () => {
      if (pending === 'metadata') metadata.resolve({ ...view, title: 'Earlier response' });
      else source.resolve(stale);
    });

    expect(screen.queryByRole('textbox', { name: 'Document content' })).not.toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('This document is no longer available');
    expect(reloadSignal?.aborted).toBe(true);
    expect(api.source).toHaveBeenCalledTimes(pending === 'metadata' ? 2 : 3);
    expect(documentDraft('owner', id)).toBeUndefined();
    expect(createObjectURL).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
    expect(await screen.findByRole('textbox', { name: 'Document content' })).toHaveValue(
      '# Original',
    );
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  },
);

it.each([
  {
    name: 'array-valued access on the export metadata read',
    authority: { access: ['owner'] },
    recheck: false,
  },
  {
    name: 'empty owner identity on the metadata recheck',
    authority: { owner_subject: '' },
    recheck: true,
  },
])('keeps draft recovery after $name through the REST decoder', async ({ authority, recheck }) => {
  const createObjectURL = exportBrowser();
  const fetcher = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(Response.json(view))
    .mockResolvedValueOnce(Response.json({ id, revision: 1, markdown: '# Original' }));
  if (recheck) {
    fetcher
      .mockResolvedValueOnce(Response.json(view))
      .mockResolvedValueOnce(Response.json({ code: 'document_not_found' }, { status: 404 }));
  }
  fetcher.mockResolvedValueOnce(Response.json({ ...view, ...authority }));
  vi.stubGlobal('fetch', fetcher);
  page(documentsClient({ shareRecipients: vi.fn() }));

  const editor = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(editor, { target: { value: '# Recoverable draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'The saved document could not be exported',
  );
  expect(editor).toHaveValue('# Recoverable draft');
  expect(screen.getByRole('button', { name: 'Download draft' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Export saved JSON' })).toBeEnabled();
  expect(documentDraft('owner', id)?.markdown).toBe('# Recoverable draft');
  expect(createObjectURL).not.toHaveBeenCalled();
  expect(fetcher).toHaveBeenCalledTimes(recheck ? 5 : 3);
});
