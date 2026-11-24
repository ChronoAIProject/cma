import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import { BUILD_INFO } from '../shell/build';
import { DocumentRegistry } from './DocumentRegistry';

function client() {
  return {
    list: vi.fn().mockResolvedValue({ items: [], next: null }),
    create: vi.fn<DocumentsClient['create']>(),
    read: vi.fn(),
    source: vi.fn(),
    update: vi.fn(),
    revisions: vi.fn(),
    shares: vi.fn(),
    shareRecipients: vi.fn(),
    grant: vi.fn(),
    revoke: vi.fn(),
    links: vi.fn(),
    createLink: vi.fn(),
    revokeLink: vi.fn(),
    comments: vi.fn(),
    comment: vi.fn(),
    removeComment: vi.fn(),
  } satisfies DocumentsClient;
}
function pendingDocument() {
  let resolve!: (value: DocumentView) => void;
  const promise = new Promise<DocumentView>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
const created: DocumentView = {
  id: `doc_${'a'.repeat(32)}`,
  owner_subject: 'synthetic-owner',
  title: 'Original title',
  revision: 1,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '2026-11-21T00:00:00Z',
  updated_at: '2026-11-21T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
};
function begin(api: DocumentsClient, onNavigate = vi.fn()) {
  const view = render(
    <DocumentRegistry client={api} viewer="owner" canCreate onNavigate={onNavigate} />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'New document' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
    target: { value: 'Original title' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Create document' }));
  return { ...view, onNavigate };
}
const retry = () => fireEvent.click(screen.getByRole('button', { name: 'Retry creation' }));
beforeEach(() => sessionStorage.clear());
afterEach(() => vi.unstubAllGlobals());

it.each([
  new DocumentError('Storage unavailable', 503),
  new DocumentError('Request timed out', 408),
  new TypeError('Network failed'),
])('keeps the original title and key after %s, cancel and explicit resume', async (failure) => {
  const api = client();
  api.create.mockRejectedValue(failure);
  const { onNavigate } = begin(api);
  await screen.findByText('Creation has not been confirmed. Retry uses the same request.');
  const title = screen.getByRole('textbox', { name: 'Title' });
  expect(title).toBeDisabled();
  expect(title).toHaveValue('Original title');
  expect(title).toHaveAccessibleDescription('The title stays with this request.');
  expect(screen.queryByRole('button', { name: 'Edit as a new request' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByText('Creation request: Original title')).toBeVisible();
  expect(api.create).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Resume creation' }));
  expect(api.create).toHaveBeenCalledTimes(1);
  retry();
  await screen.findByText('Creation has not been confirmed. Retry uses the same request.');
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(api.create.mock.calls[1]![0]).toEqual(api.create.mock.calls[0]![0]);
  expect(api.create.mock.calls[1]![1].idempotencyKey).toBe(
    api.create.mock.calls[0]![1].idempotencyKey,
  );
  expect(onNavigate).not.toHaveBeenCalled();
});

it('prevents duplicate submission and waits for confirmed success before navigation', async () => {
  const api = client();
  const pending = pendingDocument();
  api.create.mockReturnValue(pending.promise);
  const { onNavigate } = begin(api);
  fireEvent.click(screen.getByRole('button', { name: 'Creating…' }));
  expect(
    screen.getByText('Sending this creation request. Waiting for confirmation.'),
  ).toBeVisible();
  expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
  expect(screen.getByRole('textbox', { name: 'Title' })).toBeDisabled();
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(onNavigate).not.toHaveBeenCalled();
  await act(() => pending.resolve(created));
  expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'document', documentId: created.id });
});

it('keeps refusal distinct, retries the same request, and explicitly starts a new editable request', async () => {
  const api = client();
  api.create.mockRejectedValue(new DocumentError('Invalid document request', 400));
  begin(api);
  await screen.findByText('Creation request refused');
  expect(
    screen.queryByText('Creation has not been confirmed. Retry uses the same request.'),
  ).not.toBeInTheDocument();
  retry();
  await screen.findByText('Creation request refused');
  expect(api.create.mock.calls[1]![1].idempotencyKey).toBe(
    api.create.mock.calls[0]![1].idempotencyKey,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Edit as a new request' }));
  expect(api.create).toHaveBeenCalledTimes(2);
  fireEvent.change(screen.getByRole('textbox', { name: 'Title' }), {
    target: { value: 'Different title' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Create document' }));
  await screen.findByText('Creation request refused');
  expect(api.create.mock.calls[2]![1].idempotencyKey).not.toBe(
    api.create.mock.calls[0]![1].idempotencyKey,
  );
  expect(api.create.mock.calls[2]![0].title).toBe('Different title');
});

it('does not lose an uncertain original request when a later attempt is refused', async () => {
  const api = client();
  api.create
    .mockRejectedValueOnce(new TypeError('Network failed'))
    .mockRejectedValue(new DocumentError('Refused', 403));
  begin(api);
  await screen.findByText('Creation not confirmed');
  retry();
  await screen.findByText('Creation request refused');
  expect(screen.getByText('The original creation is still unconfirmed.')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Edit as a new request' })).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Title' })).toBeDisabled();
  retry();
  await screen.findByText('Creation request refused');
  expect(new Set(api.create.mock.calls.map(([, options]) => options.idempotencyKey)).size).toBe(1);
});

it('retains an original key after an identity conflict', async () => {
  const api = client();
  api.create.mockRejectedValue(new DocumentError('Key already used', 409));
  begin(api);
  await screen.findByText('Creation request refused');
  expect(screen.queryByRole('button', { name: 'Edit as a new request' })).not.toBeInTheDocument();
});

it('copies only build, time, request reference and classification, with a manual fallback', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal('navigator', { clipboard: { writeText } });
  const api = client();
  api.create.mockRejectedValue(
    new DocumentError('private response body / synthetic-owner', 503, 'untrusted-code'),
  );
  begin(api);
  await screen.findByText('Creation not confirmed');
  fireEvent.click(screen.getByText('Get help with this request'));
  fireEvent.click(screen.getByRole('button', { name: 'Copy request details' }));
  await screen.findByText('Request details copied.');
  const details = String(writeText.mock.calls[0]![0]);
  expect(details).toContain(BUILD_INFO.copyText);
  expect(details).toMatch(/Request started \(UTC\): \d{4}-\d{2}-\d{2}T/);
  expect(details).toContain(
    `Creation request reference: ${api.create.mock.calls[0]![1].idempotencyKey}`,
  );
  expect(details).toContain('HTTP status: 503');
  for (const excluded of [
    'Original title',
    'private response',
    'synthetic-owner',
    'untrusted-code',
    '# Untitled',
  ])
    expect(details).not.toContain(excluded);
  writeText.mockRejectedValueOnce(new Error('Clipboard denied'));
  fireEvent.click(screen.getByRole('button', { name: 'Copy request details' }));
  await screen.findByText('Could not copy. Select the details above and copy them manually.');
  expect(screen.getByLabelText('Request help details').textContent).toBe(details);
  expect(api.create).toHaveBeenCalledTimes(1);
});

it('blocks retry when creation permission is removed', async () => {
  const api = client();
  api.create.mockRejectedValue(new DocumentError('Unavailable', 503));
  const { rerender, onNavigate } = begin(api);
  await screen.findByText('Creation not confirmed');
  rerender(
    <DocumentRegistry client={api} viewer="owner" canCreate={false} onNavigate={onNavigate} />,
  );
  expect(screen.getByRole('button', { name: 'Retry creation' })).toBeDisabled();
  retry();
  expect(api.create).toHaveBeenCalledTimes(1);
});

it('aborts on unmount and ignores a late success', async () => {
  const api = client();
  const pending = pendingDocument();
  api.create.mockReturnValue(pending.promise);
  const { unmount, onNavigate } = begin(api);
  unmount();
  expect(api.create.mock.calls[0]![1].signal?.aborted).toBe(true);
  await act(() => pending.resolve(created));
  expect(onNavigate).not.toHaveBeenCalled();
});

it('recovers an in-flight request on reload and dispatches only when the same owner retries', async () => {
  const api = client();
  const pending = pendingDocument();
  api.create.mockReturnValueOnce(pending.promise).mockResolvedValueOnce(created);
  const { unmount, onNavigate } = begin(api);
  unmount();
  const other = render(
    <DocumentRegistry client={api} viewer="other" canCreate onNavigate={onNavigate} />,
  );
  expect(screen.getByRole('button', { name: 'New document' })).toBeVisible();
  expect(screen.queryByText('Creation request: Original title')).not.toBeInTheDocument();
  other.unmount();

  const owner = render(
    <DocumentRegistry client={api} viewer="owner" canCreate onNavigate={onNavigate} />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Resume creation' }));
  expect(api.create).toHaveBeenCalledOnce();
  expect(screen.getByRole('textbox', { name: 'Title' })).toHaveValue('Original title');
  expect(screen.getByRole('textbox', { name: 'Title' })).toBeDisabled();
  await screen.findByText('Creation has not been confirmed. Retry uses the same request.');
  retry();
  await waitFor(() =>
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith({
      kind: 'document',
      documentId: created.id,
    }),
  );
  expect(api.create.mock.calls[1]![0]).toEqual(api.create.mock.calls[0]![0]);
  expect(api.create.mock.calls[1]![1].idempotencyKey).toBe(
    api.create.mock.calls[0]![1].idempotencyKey,
  );
  owner.unmount();
  render(<DocumentRegistry client={api} viewer="owner" canCreate onNavigate={onNavigate} />);
  expect(screen.getByRole('button', { name: 'New document' })).toBeVisible();
});

it('keeps a confirmed refusal distinct after reload without persisting the response text', async () => {
  const api = client();
  api.create.mockRejectedValue(new DocumentError('Private response detail', 400));
  const { unmount, onNavigate } = begin(api);
  await screen.findByText('Creation request refused');
  unmount();

  render(<DocumentRegistry client={api} viewer="owner" canCreate onNavigate={onNavigate} />);
  fireEvent.click(screen.getByRole('button', { name: 'Resume creation' }));
  expect(screen.getByText('Creation request refused')).toBeVisible();
  expect(screen.queryByText('Private response detail')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Edit as a new request' })).toBeEnabled();
  expect(api.create).toHaveBeenCalledOnce();
});

it('blocks dispatch when session storage cannot retain the original request', () => {
  const api = client();
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Quota exhausted', 'QuotaExceededError');
  });
  try {
    begin(api);
    expect(
      screen.getByText(/Document creation recovery needs browser session storage/),
    ).toBeVisible();
    expect(api.create).not.toHaveBeenCalled();
  } finally {
    write.mockRestore();
  }
});
