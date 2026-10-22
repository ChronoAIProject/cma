import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { DocumentsClient, DocumentView } from '../api/documents';
import { DocumentError } from '../api/documents';
import { DocumentPage } from './DocumentPage';
import { documentDraft, retainDocumentDraft } from './drafts';
vi.mock('./RichDocumentEditor', () => ({
  default: ({
    source,
    editable,
    onChange,
  }: {
    source: string;
    editable: boolean;
    onChange: (v: string) => void;
  }) => (
    <textarea
      aria-label="Document content"
      value={source}
      disabled={!editable}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
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
  retainDocumentDraft('owner', id);
  window.history.replaceState({}, '', '/');
});
it('preserves the same unsaved editor and source baseline through preview without a write or reload', async () => {
  const update = vi.fn();
  const api = client({ update });
  page(api);
  const content = await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.change(content, { target: { value: '# Unfinished draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Preview', exact: true }));
  expect(screen.getByRole('textbox', { name: 'Document content' })).toBe(content);
  expect(content).toBeDisabled();
  expect(content).toHaveValue('# Unfinished draft');
  expect(screen.getByRole('status')).toHaveTextContent('Unsaved changes');
  fireEvent.click(screen.getByRole('button', { name: 'Edit', exact: true }));
  expect(content).toBeEnabled();
  expect(documentDraft('owner', id)?.sourceRevision).toBe(1);
  expect(api.source).toHaveBeenCalledTimes(1);
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
    expect(screen.getByRole('textbox', { name: 'Document title' })).toHaveAttribute('readonly');
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
  // FileReader completion dispatches the editor's onChange even while typing is locked.
  const withImage = '# Initial edit\n\n![Pasted image](data:image/png;base64,aGVsbG8=)';
  fireEvent.change(content, { target: { value: withImage } });
  acknowledge!({ ...view, revision: 4, source_revision: 4 });
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Unsaved changes'));
  expect(content).toHaveValue(withImage);
  expect(documentDraft('owner', id)?.markdown).toBe(withImage);
  expect(documentDraft('owner', id)?.sourceRevision).toBe(4);
});
