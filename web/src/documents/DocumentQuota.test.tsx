import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import type { AppRoute } from '../routing/routes';
import { DocumentPage } from './DocumentPage';
import { DocumentRegistry } from './DocumentRegistry';
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

const id = `doc_${'e'.repeat(32)}`;
const document: DocumentView = {
  id,
  owner_subject: 'owner',
  title: 'Quota document',
  revision: 3,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '2026-11-13T00:00:00Z',
  updated_at: '2026-11-13T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
};
const usage = {
  enforcement: 'enforced',
  documents: { used: 37, limit: 100 },
  bytes: { used: 210 * 2 ** 20, limit: 2 ** 30 },
  objects: { used: 40, limit: 32768 },
  pending_deletions: 0,
};
function client(overrides: Partial<DocumentsClient> = {}): DocumentsClient {
  return {
    list: vi.fn().mockResolvedValue({ items: [document], next: null }),
    documentUsage: vi.fn().mockResolvedValue(usage),
    read: vi.fn().mockResolvedValue(document),
    source: vi.fn().mockResolvedValue({ id, revision: 1, markdown: '# Original' }),
    comments: vi.fn().mockResolvedValue({ items: [], next: null }),
    deleteDocument: vi.fn().mockResolvedValue({
      id,
      revision: 4,
      deleted_at: '2026-11-13T01:00:00Z',
      purge: 'pending',
    }),
    ...overrides,
  } as unknown as DocumentsClient;
}
const registry = (api: DocumentsClient) =>
  render(<DocumentRegistry client={api} canCreate onNavigate={vi.fn()} />);
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

it('registry_shows_document_and_storage_usage_when_enforced', async () => {
  registry(client());
  expect(await screen.findByText('37 / 100 documents · 210 MiB / 1 GiB')).toBeVisible();
});
it('registry_hides_numbers_when_not_enforced', async () => {
  registry(
    client({ documentUsage: vi.fn().mockResolvedValue({ ...usage, enforcement: 'not_enforced' }) }),
  );
  await screen.findByText('Document quotas are not enforced.');
  expect(screen.queryByText(/37 \/ 100|210 MiB|32,768/)).not.toBeInTheDocument();
});
it('registry_explains_pending_release_when_deletions_pending', async () => {
  registry(
    client({ documentUsage: vi.fn().mockResolvedValue({ ...usage, pending_deletions: 2 }) }),
  );
  expect(await screen.findByText(/Storage is being released/)).toHaveTextContent(
    'Deleted documents still count toward your limits until cleanup finishes.',
  );
  expect(screen.getByText('37 / 100 documents · 210 MiB / 1 GiB')).toBeVisible();
});
it('registry_explains_pending_release_even_when_quotas_not_enforced', async () => {
  registry(
    client({
      documentUsage: vi.fn().mockResolvedValue({
        enforcement: 'not_enforced',
        documents: null,
        bytes: null,
        objects: null,
        pending_deletions: 2,
      }),
    }),
  );
  await screen.findByText('Document quotas are not enforced.');
  expect(screen.getByText(/Storage is being released/)).toBeVisible();
  expect(screen.queryByText(/37 \/ 100|210 MiB|32,768/)).not.toBeInTheDocument();
});
it('usage_reflects_pending_release_until_purge_completes', async () => {
  const documentUsage = vi.fn().mockResolvedValue({
    ...usage,
    documents: { used: 101, limit: 100 },
    pending_deletions: 1,
  });
  const api = client({ documentUsage });
  const first = registry(api);
  await screen.findByText('101 / 100 documents · 210 MiB / 1 GiB');
  expect(screen.getByText(/Storage is being released/)).toBeVisible();
  first.unmount();
  documentUsage.mockResolvedValue({
    ...usage,
    documents: { used: 100, limit: 100 },
    bytes: { used: 200 * 2 ** 20, limit: 2 ** 30 },
    pending_deletions: 0,
  });
  registry(api);
  await screen.findByText('100 / 100 documents · 200 MiB / 1 GiB');
  expect(screen.queryByText(/Storage is being released/)).not.toBeInTheDocument();
});
it('registry_never_substitutes_zero_for_unknown_counters', async () => {
  registry(
    client({
      documentUsage: vi.fn().mockResolvedValue({ ...usage, documents: null, bytes: null }),
    }),
  );
  expect(await screen.findByText('Document usage is unavailable.')).toBeVisible();
  expect(screen.queryByText(/0 \/ 100|0 B/)).not.toBeInTheDocument();
});
it('registry_keeps_the_library_available_when_usage_fails_and_can_retry', async () => {
  const documentUsage = vi
    .fn()
    .mockRejectedValueOnce(new Error('Unavailable'))
    .mockResolvedValue(usage);
  registry(client({ documentUsage }));
  await screen.findByRole('link', { name: /Quota document/ });
  const warning = await screen.findByText('Document usage could not be loaded.');
  fireEvent.click(
    within(warning.closest('[role="alert"]')!).getByRole('button', { name: 'Retry usage' }),
  );
  expect(await screen.findByText('37 / 100 documents · 210 MiB / 1 GiB')).toBeVisible();
});

it.each([
  ['document_limit_reached', 'document count', 100, 100, '0 documents available'],
  ['document_bytes_limit_reached', 'retained document storage', 2 ** 30, 2 ** 30, '0 B available'],
  ['document_objects_limit_reached', 'stored document object', 32768, 32768, '0 objects available'],
])(
  'quota_refusal_keeps_draft_and_names_dimension: %s',
  async (code, dimension, limit, live, available) => {
    page(
      client({
        update: vi.fn().mockRejectedValue(
          new DocumentError('Technical refusal', 429, String(code), {
            limit: Number(limit),
            live: Number(live),
          }),
        ),
      }),
    );
    await editDraft();
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    const alert = await screen.findByText(new RegExp(String(dimension)));
    expect(alert).toHaveTextContent('Delete old documents');
    expect(alert).toHaveTextContent(String(available));
    expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue(
      '# Retained work',
    );
    expect(documentDraft('owner', id)?.markdown).toBe('# Retained work');
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
  },
);
it('write_busy_is_retryable_and_keeps_draft', async () => {
  const update = vi
    .fn()
    .mockRejectedValueOnce(new DocumentError('Busy', 429, 'document_write_busy'))
    .mockResolvedValue({ ...document, revision: 4, source_revision: 4 });
  page(client({ update }));
  await editDraft();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(await screen.findByText(/Document writing is busy/)).toHaveTextContent(
    'Try again in a moment.',
  );
  expect(documentDraft('owner', id)?.markdown).toBe('# Retained work');
  expect(screen.getByRole('textbox', { name: 'Document content' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Saved'));
});
it('creation_quota_refusal_keeps_the_title_and_names_the_limit', async () => {
  registry(
    client({
      create: vi
        .fn()
        .mockRejectedValue(new DocumentError('Capacity', 429, 'document_limit_reached')),
    }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'New document' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
    target: { value: 'Keep this title' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Create document' }));
  await screen.findByText(/document count limit/);
  expect(screen.getByRole('textbox', { name: 'Title' })).toHaveValue('Keep this title');
});

it('delete_confirms_then_navigates_back_and_registry_omits_document', async () => {
  const api = client();
  vi.mocked(api.deleteDocument).mockImplementation(async () => {
    vi.mocked(api.list).mockResolvedValue({ items: [], next: null });
    return { id, revision: 4, deleted_at: '2026-11-13T01:00:00Z', purge: 'pending' };
  });
  function Journey() {
    const [route, navigate] = useState<AppRoute>({ kind: 'document', documentId: id });
    return route.kind === 'document' ? (
      <DocumentPage client={api} id={id} viewer="owner" onNavigate={navigate} />
    ) : (
      <DocumentRegistry client={api} canCreate onNavigate={navigate} />
    );
  }
  render(<Journey />);
  await editDraft();
  const trigger = screen.getByRole('button', { name: 'Delete document' });
  fireEvent.click(trigger);
  expect(api.deleteDocument).not.toHaveBeenCalled();
  let dialog = screen.getByRole('dialog', { name: 'Delete this document?' });
  expect(within(dialog).getByText(/cannot be undone/)).toBeVisible();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  fireEvent.click(trigger);
  dialog = screen.getByRole('dialog', { name: 'Delete this document?' });
  expect(within(dialog).getByRole('checkbox')).not.toBeChecked();
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: 'Delete document' }));
  await screen.findByRole('heading', { name: 'Artifacts' });
  await screen.findByText('A place for your next idea');
  expect(screen.queryByRole('link', { name: /Quota document/ })).not.toBeInTheDocument();
  expect(api.deleteDocument).toHaveBeenCalledWith(
    id,
    expect.objectContaining({
      expectedRevision: 3,
      idempotencyKey: expect.any(String),
    }),
  );
  expect(documentDraft('owner', id)).toBeUndefined();
});
it('delete_conflict_offers_reload_and_keeps_draft', async () => {
  const navigate = vi.fn();
  page(
    client({
      deleteDocument: vi
        .fn()
        .mockRejectedValue(
          new DocumentError('Revision changed', 409, 'document_revision_conflict'),
        ),
    }),
    navigate,
  );
  await editDraft();
  await confirmDeletion();
  expect(await screen.findByText(/The document changed before it could be deleted/)).toBeVisible();
  expect(navigate).not.toHaveBeenCalled();
  expect(documentDraft('owner', id)?.markdown).toBe('# Retained work');
  expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue('# Retained work');
  fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
  expect(screen.getByRole('dialog', { name: 'Load the latest document?' })).toBeVisible();
  expect(documentDraft('owner', id)?.markdown).toBe('# Retained work');
});
it('delete_unavailable_shows_typed_message', async () => {
  const navigate = vi.fn();
  page(
    client({
      deleteDocument: vi
        .fn()
        .mockRejectedValue(new DocumentError('Disabled', 503, 'document_deletion_unavailable')),
    }),
    navigate,
  );
  await confirmDeletion();
  expect(await screen.findByText(/Document deletion is unavailable/)).toHaveTextContent(
    'Try again later.',
  );
  expect(navigate).not.toHaveBeenCalled();
});
it.each(['read', 'comment'] as const)('non_owner_cannot_see_delete: %s', async (access) => {
  const api = client({ read: vi.fn().mockResolvedValue({ ...document, access }) });
  page(api);
  await screen.findByRole('textbox', { name: 'Document content' });
  expect(screen.queryByRole('button', { name: 'Delete document' })).not.toBeInTheDocument();
  expect(api.deleteDocument).not.toHaveBeenCalled();
});
it('delete_disables_duplicate_submissions_and_replays_an_uncertain_result_with_the_same_key', async () => {
  let reject: (error: Error) => void = () => {};
  const deleteDocument = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((_, fail) => {
          reject = fail;
        }),
    )
    .mockResolvedValue({ id, revision: 4, deleted_at: '2026-11-13T01:00:00Z', purge: 'pending' });
  const navigate = vi.fn();
  page(client({ deleteDocument }), navigate);
  const dialog = await confirmDeletion();
  expect(within(dialog).getByRole('button', { name: 'Deleting…' })).toBeDisabled();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Deleting…' }));
  expect(deleteDocument).toHaveBeenCalledTimes(1);
  reject(new TypeError('Failed to fetch'));
  await screen.findByText(/Deletion could not be confirmed/);
  fireEvent.click(within(dialog).getByRole('checkbox'));
  fireEvent.click(within(dialog).getByRole('button', { name: 'Retry delete' }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith({ kind: 'artifacts' }));
  expect(deleteDocument.mock.calls[1]?.[1].idempotencyKey).toBe(
    deleteDocument.mock.calls[0]?.[1].idempotencyKey,
  );
  expect(deleteDocument.mock.calls[1]?.[1].expectedRevision).toBe(3);
});
