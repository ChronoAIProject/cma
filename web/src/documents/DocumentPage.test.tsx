import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createHash, webcrypto } from 'node:crypto';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DocumentsClient, DocumentSource, DocumentView } from '../api/documents';
import { DocumentError } from '../api/documents';
import { DocumentPage } from './DocumentPage';
import { documentDraft, retainDocumentDraft } from './drafts';
const editorCallbacks = vi.hoisted(() => ({ save: () => {} }));
vi.mock('./RichDocumentEditor', () => ({
  default: ({
    source,
    editable,
    onChange,
    onSave,
  }: {
    source: string;
    editable: boolean;
    onChange: (v: string) => void;
    onSave: () => void;
  }) => {
    editorCallbacks.save = onSave;
    return (
      <textarea
        aria-label="Document content"
        value={source}
        disabled={!editable}
        onChange={(event) => onChange(event.target.value)}
      />
    );
  },
}));
const id = `doc_${'a'.repeat(32)}`;
const view: DocumentView = {
  id,
  owner_subject: 'owner',
  title: 'Original',
  revision: 3,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '2026-10-22T00:00:00Z',
  updated_at: '2026-10-22T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
};
function client(overrides: Partial<DocumentsClient> = {}): DocumentsClient {
  return {
    read: vi.fn().mockResolvedValue(view),
    source: vi.fn().mockResolvedValue({ id, revision: 1, markdown: '# Original' }),
    comments: vi.fn().mockResolvedValue({ items: [], next: null }),
    ...overrides,
  } as unknown as DocumentsClient;
}
const page = (api: DocumentsClient) =>
  render(<DocumentPage client={api} id={id} viewer="owner" onNavigate={vi.fn()} />);
beforeEach(() => {
  vi.stubEnv('VITE_SAVED_DOCUMENT_EXPORT_ENABLED', undefined);
  retainDocumentDraft('owner', id);
  window.history.replaceState({}, '', '/');
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
it('preserves the same unsaved editor and source baseline through preview without a write or reload', async () => {
  const update = vi
    .fn()
    .mockResolvedValue({ ...view, title: 'Draft', revision: 4, source_revision: 4 });
  const api = client({ update });
  page(api);
  const content = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(content, { target: { value: '# Unfinished draft' } });
  const title = screen.getByRole('textbox', { name: 'Document title' });
  fireEvent.change(title, { target: { value: 'Draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
  expect(screen.getByRole('textbox', { name: 'Document content' })).toBe(content);
  expect(content).toBeDisabled();
  expect(content).toHaveValue('# Unfinished draft');
  expect(screen.getByRole('status')).toHaveTextContent('Unsaved changes');
  expect(screen.getByText('Switch to Edit to save')).toBeVisible();
  expect(title).toBeDisabled();
  expect(title).toHaveValue('Draft');
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  act(() => editorCallbacks.save());
  expect(update).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  expect(content).toBeEnabled();
  expect(title).toBeEnabled();
  expect(documentDraft('owner', id)?.sourceRevision).toBe(1);
  expect(api.source).toHaveBeenCalledTimes(1);
  expect(update).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Saved'));
  expect(update).toHaveBeenCalledExactlyOnceWith(
    id,
    {
      title: 'Draft',
      markdown: '# Unfinished draft',
      expected_revision: 3,
    },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
});
it('keeps a clean preview out of title focus and ignores typing and save callbacks', async () => {
  const user = userEvent.setup();
  const update = vi.fn();
  page(client({ update }));
  const content = await screen.findByRole('textbox', { name: 'Document content' });
  const title = screen.getByRole('textbox', { name: 'Document title' });
  await user.click(screen.getByRole('button', { name: 'Preview' }));
  expect(title).toBeDisabled();
  title.focus();
  expect(title).not.toHaveFocus();
  await user.type(title, 'Accidental edit');
  await user.type(content, 'Accidental edit');
  await user.click(screen.getByRole('button', { name: 'Back to artifacts' }));
  await user.tab();
  expect(screen.getByRole('button', { name: 'Edit' })).toHaveFocus();
  act(() => editorCallbacks.save());
  expect(title).toHaveValue('Original');
  expect(content).toHaveValue('# Original');
  expect(screen.getByRole('status')).toHaveTextContent('Saved');
  expect(documentDraft('owner', id)).toBeUndefined();
  expect(update).not.toHaveBeenCalled();
});
it('pins source reads to the source revision, and retains the exact save key across an uncertain retry', async () => {
  const update = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Network lost'))
    .mockResolvedValueOnce({ ...view, title: 'Edited', revision: 4, source_revision: 4 });
  const api = client({ update });
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  expect(api.source).toHaveBeenCalledWith(id, expect.objectContaining({ revision: 1 }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Document title' }), {
    target: { value: 'Edited' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByRole('button', { name: 'Retry save' });
  expect(screen.getByRole('textbox', { name: 'Document content' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
  expect(screen.getByRole('button', { name: 'Retry save' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry save' }));
  act(() => editorCallbacks.save());
  expect(update).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  expect(screen.getByRole('textbox', { name: 'Document title' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry save' }));
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Saved'));
  expect(update.mock.calls[1]?.[1]).toEqual(update.mock.calls[0]?.[1]);
  expect(update.mock.calls[1]?.[2].idempotencyKey).toBe(update.mock.calls[0]?.[2].idempotencyKey);
});
it('preserves the original draft baseline across navigation when a newer source exists', async () => {
  retainDocumentDraft('owner', id, { title: 'Draft', markdown: '# My work', sourceRevision: 1 });
  const api = client({
    read: vi.fn().mockResolvedValue({ ...view, revision: 5, source_revision: 5 }),
    source: vi.fn().mockResolvedValue({ id, revision: 5, markdown: '# Other work' }),
  });
  const first = page(api);
  expect(await screen.findByRole('textbox', { name: 'Document content' })).toHaveValue('# My work');
  await waitFor(() => expect(documentDraft('owner', id)?.sourceRevision).toBe(1));
  first.unmount();
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(screen.getByRole('status')).toHaveTextContent('Version changed');
  expect(documentDraft('owner', id)?.sourceRevision).toBe(1);
});
it('retains an uncertain save across navigation and reconciles it using its original key', async () => {
  const update = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Network lost'))
    .mockResolvedValueOnce({ ...view, title: 'Edited', revision: 4, source_revision: 4 });
  const api = client({ update });
  const first = page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(screen.getByRole('textbox', { name: 'Document title' }), {
    target: { value: 'Edited' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByRole('button', { name: 'Retry save' });
  first.unmount();
  vi.mocked(api.read).mockResolvedValue({
    ...view,
    title: 'Edited',
    revision: 4,
    source_revision: 4,
  });
  page(api);
  fireEvent.click(await screen.findByRole('button', { name: 'Retry save' }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(2));
  expect(update.mock.calls[1]?.[2].idempotencyKey).toBe(update.mock.calls[0]?.[2].idempotencyKey);
});
it('does not permit a comment receipt from a newer source to authorize an overwrite', async () => {
  const api = client({
    comment: vi.fn().mockResolvedValue({ document: { ...view, revision: 6, source_revision: 5 } }),
  });
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(screen.getByRole('textbox', { name: 'Document content' }), {
    target: { value: '# Local work' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Comments' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Add a comment' }), {
    target: { value: 'A note' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Comment' }));
  await screen.findByText(
    'This document changed. Your draft is retained; reload the latest version before saving.',
  );
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(documentDraft('owner', id)?.sourceRevision).toBe(1);
  expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue('# Local work');
});
it('preserves content after a conflict and loads the latest only after explicit discard', async () => {
  const api = client({
    update: vi.fn().mockRejectedValue(new DocumentError('Revision conflict', 409)),
  });
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(screen.getByRole('textbox', { name: 'Document content' }), {
    target: { value: '# Local work' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Revision conflict');
  for (const mode of ['Preview', 'Edit']) {
    fireEvent.click(screen.getByRole('button', { name: mode }));
    expect(screen.getByRole('textbox', { name: 'Document title' })).toBeDisabled();
    expect(screen.getByRole('textbox', { name: 'Document content' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    act(() => editorCallbacks.save());
  }
  expect(api.update).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue('# Local work');
  fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
  expect(screen.getByRole('dialog', { name: 'Load the latest document?' })).toBeVisible();
  vi.mocked(api.source).mockResolvedValue({ id, revision: 1, markdown: '# Fresh source' });
  fireEvent.click(screen.getByRole('button', { name: 'Discard draft and reload' }));
  await waitFor(() =>
    expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue('# Fresh source'),
  );
});
it.each(['read', 'comment'] as const)(
  'keeps %s access read-only without edit or sharing actions',
  async (access) => {
    page(client({ read: vi.fn().mockResolvedValue({ ...view, access }) }));
    expect(await screen.findByRole('textbox', { name: 'Document content' })).toBeDisabled();
    expect(screen.getByRole('textbox', { name: 'Document title' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Share' })).not.toBeInTheDocument();
  },
);

it('keeps image source that arrives asynchronously while an earlier save is being acknowledged', async () => {
  let acknowledge: ((document: DocumentView) => void) | undefined;
  const api = client({
    update: vi.fn(
      () =>
        new Promise<DocumentView>((resolve) => {
          acknowledge = resolve;
        }),
    ),
  });
  page(api);
  const content = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(content, { target: { value: '# Initial edit' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(acknowledge).toBeDefined());
  fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
  // FileReader completion dispatches the editor's onChange even while typing is locked.
  const withImage = '# Initial edit\n\n![Pasted image](data:image/png;base64,aGVsbG8=)';
  fireEvent.change(content, { target: { value: withImage } });
  acknowledge!({ ...view, revision: 4, source_revision: 4 });
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Unsaved changes'));
  expect(content).toHaveValue(withImage);
  await waitFor(() => {
    expect(documentDraft('owner', id)?.markdown).toBe(withImage);
    expect(documentDraft('owner', id)?.sourceRevision).toBe(4);
  });
});

it('reconciles a save admitted in Edit after switching to Preview', async () => {
  let acknowledge!: (document: DocumentView) => void;
  const update = vi.fn(
    () =>
      new Promise<DocumentView>((resolve) => {
        acknowledge = resolve;
      }),
  );
  page(client({ update }));
  const content = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(content, { target: { value: '# Saved draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
  expect(screen.getByRole('status')).toHaveTextContent('Saving…');
  act(() => editorCallbacks.save());
  await act(async () => acknowledge({ ...view, revision: 4, source_revision: 4 }));
  expect(screen.getByRole('status')).toHaveTextContent('Saved');
  expect(content).toHaveValue('# Saved draft');
  expect(content).toBeDisabled();
  expect(documentDraft('owner', id)).toBeUndefined();
  expect(update).toHaveBeenCalledTimes(1);
});

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
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  await waitFor(() => expect(createObjectURL).toHaveBeenCalledOnce());
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
});

it.each(['owner changed', 'metadata denied', 'source revoked', 'source missing', 'corrupt'])(
  'creates no download when %s after the page was loaded',
  async (failure) => {
    const createObjectURL = exportBrowser();
    const api = client();
    page(api);
    await screen.findByRole('textbox', { name: 'Document content' });
    if (failure === 'owner changed') {
      vi.mocked(api.read).mockResolvedValue({ ...view, owner_subject: 'another-owner' });
    } else if (failure === 'metadata denied') {
      vi.mocked(api.read).mockRejectedValue(new DocumentError('Denied with private context', 403));
    } else if (failure === 'source revoked' || failure === 'source missing') {
      vi.mocked(api.source).mockRejectedValue(
        new DocumentError('No longer available', failure === 'source revoked' ? 403 : 404),
      );
    }
    // The original fixture has a deliberately mismatched digest for the corrupt case.
    fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
    await screen.findByRole('alert');
    expect(createObjectURL).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Export saved JSON' })).toBeEnabled();
    expect(screen.queryByText('Denied with private context')).not.toBeInTheDocument();
    if (failure === 'owner changed' || failure === 'metadata denied') {
      expect(api.source).toHaveBeenCalledOnce();
    }
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

it('keeps draft recovery downloads available when saved export fails', async () => {
  const createObjectURL = exportBrowser();
  page(client());
  const editor = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(editor, { target: { value: '# Recover this draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  const download = await screen.findByRole('button', { name: 'Download draft' });
  vi.useFakeTimers();
  fireEvent.click(download);
  expect(createObjectURL).toHaveBeenCalledOnce();
  const blob: Blob = createObjectURL.mock.calls[0]![0];
  expect(blob.type).toBe('text/markdown;charset=utf-8');
  expect(editor).toHaveValue('# Recover this draft');
  vi.runAllTimers();
  expect(URL.revokeObjectURL).toHaveBeenCalledExactlyOnceWith('blob:saved-document');
});

it('handles revocation during the source read without downloading, then allows a fresh reload', async () => {
  const createObjectURL = exportBrowser();
  const api = client();
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  let revoke: ((cause: Error) => void) | undefined;
  vi.mocked(api.source).mockImplementationOnce(
    () =>
      new Promise((_resolve, reject) => {
        revoke = reject;
      }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Export saved JSON' }));
  await waitFor(() => expect(revoke).toBeDefined());
  await act(async () => revoke!(new DocumentError('Access revoked during source retrieval', 403)));
  expect(createObjectURL).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
  await screen.findByRole('textbox', { name: 'Document content' });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(api.read).toHaveBeenCalledTimes(3);
});
