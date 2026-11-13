import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import { DocumentPage } from './DocumentPage';
import { documentDraft, retainDocumentDraft, retainDocumentDeletionIntent } from './drafts';

vi.mock('./RichDocumentEditor', () => ({
  default: ({
    source,
    editable,
    onChange,
  }: {
    source: string;
    editable: boolean;
    onChange: (value: string) => void;
  }) => (
    <textarea
      aria-label="Document content"
      value={source}
      disabled={!editable}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));
const id = `doc_${'f'.repeat(32)}`;
const document: DocumentView = {
  id,
  owner_subject: 'owner',
  title: 'Recovery document',
  revision: 3,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '2026-11-13T00:00:00Z',
  updated_at: '2026-11-13T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
};
function client(overrides: Partial<DocumentsClient> = {}): DocumentsClient {
  return {
    read: vi.fn().mockResolvedValue(document),
    source: vi.fn().mockResolvedValue({ id, revision: 1, markdown: '# Original' }),
    comments: vi.fn().mockResolvedValue({ items: [], next: null }),
    ...overrides,
  } as unknown as DocumentsClient;
}
const page = (api: DocumentsClient, onNavigate = vi.fn()) =>
  render(<DocumentPage client={api} id={id} viewer="owner" onNavigate={onNavigate} />);
async function editDraft() {
  fireEvent.change(await screen.findByRole('textbox', { name: 'Document content' }), {
    target: { value: '# Retained work' },
  });
}
async function confirmDeletion() {
  fireEvent.click(await screen.findByRole('button', { name: 'Delete document' }));
  const dialog = screen.getByRole('dialog', { name: 'Delete this document?' });
  const confirm = within(dialog).getByRole('button', { name: 'Delete document' });
  expect(confirm).toBeDisabled();
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(confirm);
  return dialog;
}
beforeEach(() => {
  retainDocumentDraft('owner', id);
  retainDocumentDeletionIntent('owner', id);
  window.history.replaceState({}, '', '/');
});

it('reload_after_a_save_refusal_cannot_discard_an_unconfirmed_delete_key', async () => {
  const deleteDocument = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Failed to fetch'))
    .mockResolvedValue({ id, revision: 4, deleted_at: '2026-11-13T01:00:00Z', purge: 'pending' });
  const navigate = vi.fn();
  const api = client({
    deleteDocument,
    update: vi
      .fn()
      .mockRejectedValue(new DocumentError('Capacity', 429, 'document_bytes_limit_reached')),
  });
  page(api, navigate);
  await editDraft();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText(/retained document storage limit/);
  let dialog = await confirmDeletion();
  await within(dialog).findByText(/Deletion could not be confirmed/);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  const reload = screen.getByRole('button', { name: 'Reload latest' });
  expect(reload).toBeDisabled();
  fireEvent.click(reload);
  expect(
    screen.queryByRole('dialog', { name: 'Load the latest document?' }),
  ).not.toBeInTheDocument();
  expect(api.read).toHaveBeenCalledTimes(1);
  expect(documentDraft('owner', id)?.markdown).toBe('# Retained work');
  fireEvent.click(screen.getByRole('button', { name: 'Retry delete' }));
  dialog = screen.getByRole('dialog', { name: 'Delete this document?' });
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: 'Retry delete' }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith({ kind: 'artifacts' }));
  expect(deleteDocument.mock.calls[1]?.[1].idempotencyKey).toBe(
    deleteDocument.mock.calls[0]?.[1].idempotencyKey,
  );
  expect(deleteDocument.mock.calls[1]?.[1].expectedRevision).toBe(3);
});

it('a_newer_comment_receipt_cannot_block_replaying_an_unconfirmed_delete', async () => {
  const deleteDocument = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Failed to fetch'))
    .mockRejectedValueOnce(
      new DocumentError('Revision changed', 409, 'document_revision_conflict'),
    );
  const api = client({
    deleteDocument,
    comment: vi.fn().mockResolvedValue({
      document: { ...document, revision: 6, source_revision: 5 },
    }),
  });
  page(api);
  await editDraft();
  let dialog = await confirmDeletion();
  await within(dialog).findByText(/Deletion could not be confirmed/);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  fireEvent.click(screen.getByRole('button', { name: 'Comments' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Add a comment' }), {
    target: { value: 'Review this version' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Comment' }));
  await screen.findByText(
    'This document changed. Your draft is retained; reload the latest version before saving.',
  );
  const retry = screen.getByRole('button', { name: 'Retry delete' });
  expect(retry).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Reload latest' })).toBeDisabled();
  fireEvent.click(retry);
  dialog = screen.getByRole('dialog', { name: 'Delete this document?' });
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: 'Retry delete' }));
  await screen.findByText(/The document changed before it could be deleted/);
  expect(deleteDocument.mock.calls[1]?.[1].idempotencyKey).toBe(
    deleteDocument.mock.calls[0]?.[1].idempotencyKey,
  );
  expect(deleteDocument.mock.calls[1]?.[1].expectedRevision).toBe(3);
  expect(screen.getByRole('button', { name: 'Reload latest' })).toBeEnabled();
  expect(documentDraft('owner', id)?.markdown).toBe('# Retained work');
});

it('uncertain_delete_survives_spa_navigation_with_its_original_key_and_revision', async () => {
  const deleteDocument = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Failed to fetch'))
    .mockRejectedValueOnce(
      new DocumentError('Revision changed', 409, 'document_revision_conflict'),
    );
  const api = client({ deleteDocument });
  const first = page(api);
  const dialog = await confirmDeletion();
  await within(dialog).findByText(/Deletion could not be confirmed/);
  first.unmount();
  vi.mocked(api.read).mockResolvedValue({ ...document, revision: 5, source_revision: 5 });
  vi.mocked(api.source).mockResolvedValue({ id, revision: 5, markdown: '# New saved version' });
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  fireEvent.click(screen.getByRole('button', { name: 'Retry delete' }));
  const retry = screen.getByRole('dialog', { name: 'Delete this document?' });
  fireEvent.click(within(retry).getByRole('checkbox'));
  fireEvent.click(within(retry).getByRole('button', { name: 'Retry delete' }));
  await screen.findByText(/The document changed before it could be deleted/);
  expect(deleteDocument.mock.calls[1]?.[1].idempotencyKey).toBe(
    deleteDocument.mock.calls[0]?.[1].idempotencyKey,
  );
  expect(deleteDocument.mock.calls[1]?.[1].expectedRevision).toBe(3);
  expect(screen.getByRole('button', { name: 'Reload latest' })).toBeEnabled();
});

it('uncertain_delete_can_replay_after_navigation_when_the_document_read_is_not_found', async () => {
  const deleteDocument = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Failed to fetch'))
    .mockResolvedValue({ id, revision: 4, deleted_at: '2026-11-13T01:00:00Z', purge: 'pending' });
  const api = client({ deleteDocument });
  const first = page(api);
  const dialog = await confirmDeletion();
  await within(dialog).findByText(/Deletion could not be confirmed/);
  first.unmount();
  vi.mocked(api.read).mockRejectedValue(
    new DocumentError('Document not found', 404, 'document_not_found'),
  );
  const navigate = vi.fn();
  page(api, navigate);
  fireEvent.click(await screen.findByRole('button', { name: 'Retry delete' }));
  const retry = screen.getByRole('dialog', { name: 'Delete this document?' });
  fireEvent.click(within(retry).getByRole('checkbox'));
  fireEvent.click(within(retry).getByRole('button', { name: 'Retry delete' }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith({ kind: 'artifacts' }));
  expect(deleteDocument.mock.calls[1]?.[1].idempotencyKey).toBe(
    deleteDocument.mock.calls[0]?.[1].idempotencyKey,
  );
  expect(deleteDocument.mock.calls[1]?.[1].expectedRevision).toBe(3);
});

it('a_feature_off_refusal_cannot_clear_an_earlier_unconfirmed_delete', async () => {
  const deleteDocument = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Failed to fetch'))
    .mockRejectedValueOnce(new DocumentError('Disabled', 503, 'document_deletion_unavailable'))
    .mockResolvedValue({ id, revision: 4, deleted_at: '2026-11-13T01:00:00Z', purge: 'pending' });
  const navigate = vi.fn();
  page(client({ deleteDocument }), navigate);
  const dialog = await confirmDeletion();
  await within(dialog).findByText(/Deletion could not be confirmed/);
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: 'Retry delete' }));
  await within(dialog).findByText(/Document deletion is unavailable/);
  expect(dialog).not.toHaveTextContent('This document has not been deleted');
  expect(dialog).toHaveTextContent('still unconfirmed');
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: 'Retry delete' }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith({ kind: 'artifacts' }));
  for (const call of deleteDocument.mock.calls) {
    expect(call[1].idempotencyKey).toBe(deleteDocument.mock.calls[0]?.[1].idempotencyKey);
    expect(call[1].expectedRevision).toBe(3);
  }
});

it('an_unconfirmed_delete_warns_before_leaving_the_tab', async () => {
  page(client({ deleteDocument: vi.fn().mockRejectedValue(new TypeError('Failed to fetch')) }));
  const dialog = await confirmDeletion();
  await within(dialog).findByText(/Deletion could not be confirmed/);
  const leaving = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(leaving);
  expect(leaving.defaultPrevented).toBe(true);
});
