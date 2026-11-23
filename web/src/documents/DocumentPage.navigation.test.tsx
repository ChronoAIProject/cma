import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import { useAppRouter } from '../routing/routes';
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
const id = `doc_${'c'.repeat(32)}`;
const view: DocumentView = {
  id,
  owner_subject: 'owner',
  title: 'Saved title',
  revision: 3,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '',
  updated_at: '',
  origin_agent_id: null,
  access: 'owner',
};
function client(overrides: Partial<DocumentsClient> = {}): DocumentsClient {
  return {
    read: vi.fn().mockResolvedValue(view),
    source: vi.fn().mockResolvedValue({ id, revision: 1, markdown: '# Saved body' }),
    comments: vi.fn().mockResolvedValue({ items: [], next: null }),
    update: vi
      .fn()
      .mockResolvedValue({ ...view, title: 'Changed title', revision: 4, source_revision: 4 }),
    ...overrides,
  } as unknown as DocumentsClient;
}
function Page({ api, viewer = 'owner' }: { api: DocumentsClient; viewer?: string }) {
  const router = useAppRouter();
  return router.route.kind === 'document' ? (
    <DocumentPage
      client={api}
      id={router.route.documentId}
      viewer={viewer}
      onNavigate={router.navigate}
    />
  ) : (
    <h1>Artifacts</h1>
  );
}
const back = () => fireEvent.click(screen.getByRole('button', { name: 'Back to artifacts' }));
const unloadBlocked = () => {
  const event = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(event);
  return event.defaultPrevented;
};
beforeEach(() => {
  retainDocumentDraft('owner', id);
  retainDocumentDraft('other', id);
  history.replaceState(null, '', `/artifacts/${id}`);
});

it.each(['title', 'body', 'both'])(
  'protects %s changes in Preview and cancels without writes',
  async (kind) => {
    const api = client();
    render(<Page api={api} />);
    const body = await screen.findByLabelText('Document content');
    const title = screen.getByLabelText('Document title');
    if (kind !== 'body') fireEvent.change(title, { target: { value: 'Changed title' } });
    if (kind !== 'title') fireEvent.change(body, { target: { value: '# Changed body' } });
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(unloadBlocked()).toBe(true);
    back();
    fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
    expect(screen.getByLabelText('Document content')).toBe(body);
    expect(title).toHaveValue(kind === 'body' ? 'Saved title' : 'Changed title');
    expect(body).toHaveValue(kind === 'title' ? '# Saved body' : '# Changed body');
    expect(api.update).not.toHaveBeenCalled();
    expect(title).toBeDisabled();
  },
);

it.each(['save', 'revert'])('removes both protections after %s', async (kind) => {
  render(<Page api={client()} />);
  const title = await screen.findByLabelText('Document title');
  fireEvent.change(title, { target: { value: 'Changed title' } });
  if (kind === 'save') {
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Saved'));
  } else fireEvent.change(title, { target: { value: 'Saved title' } });
  expect(unloadBlocked()).toBe(false);
  back();
  expect(screen.getByRole('heading', { name: 'Artifacts' })).toBeVisible();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it.each([409, 503])(
  'keeps a %s save failure protected and preserves its intent',
  async (status) => {
    const api = client({
      update: vi.fn().mockRejectedValue(new DocumentError('Save failed', status)),
    });
    render(<Page api={api} />);
    fireEvent.change(await screen.findByLabelText('Document title'), {
      target: { value: 'Changed title' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByText('Save failed');
    await waitFor(() =>
      expect(Boolean(documentDraft('owner', id)?.saveIntent)).toBe(status === 503),
    );
    const draft = documentDraft('owner', id);
    back();
    fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
    expect(unloadBlocked()).toBe(true);
    expect(documentDraft('owner', id)).toEqual(draft);
    expect(api.update).toHaveBeenCalledOnce();
    if (status === 503) {
      fireEvent.click(screen.getByRole('button', { name: 'Retry same save' }));
      await waitFor(() => expect(api.update).toHaveBeenCalledTimes(2));
      const calls = vi.mocked(api.update).mock.calls;
      expect(calls[1]![1]).toEqual(calls[0]![1]);
      expect(calls[1]![2].idempotencyKey).toBe(calls[0]![2].idempotencyKey);
    }
  },
);

it('protects an unresolved save even when its retained content equals the saved baseline', async () => {
  retainDocumentDraft('owner', id, {
    title: view.title,
    markdown: '# Saved body',
    sourceRevision: 1,
    saveIntent: {
      key: 'original-save',
      body: { title: view.title, markdown: '# Saved body', expected_revision: 3 },
    },
  });
  render(<Page api={client()} />);
  await screen.findByLabelText('Document content');
  expect(unloadBlocked()).toBe(true);
  back();
  expect(screen.getByRole('dialog')).toHaveTextContent('Your save has not been confirmed');
  fireEvent.click(screen.getByRole('button', { name: 'Leave without saving' }));
  expect(documentDraft('owner', id)?.saveIntent?.key).toBe('original-save');
});

it('allows a pending save to finish while Stay keeps the same editor', async () => {
  let finish!: (value: DocumentView) => void;
  const api = client({
    update: vi.fn(
      () =>
        new Promise<DocumentView>((resolve) => {
          finish = resolve;
        }),
    ),
  });
  render(<Page api={api} />);
  const body = await screen.findByLabelText('Document content');
  fireEvent.change(screen.getByLabelText('Document title'), { target: { value: 'Changed title' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  back();
  fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
  expect(screen.getByLabelText('Document content')).toBe(body);
  await act(async () =>
    finish({ ...view, title: 'Changed title', revision: 4, source_revision: 4 }),
  );
  expect(unloadBlocked()).toBe(false);
  expect(api.update).toHaveBeenCalledOnce();
});

it.each([401, 403, 404])(
  'removes protected content after a %s save refusal without a Stay veto',
  async (status) => {
    let refuse!: (error: Error) => void;
    const api = client({
      update: vi.fn(
        () =>
          new Promise<DocumentView>((_resolve, reject) => {
            refuse = reject;
          }),
      ),
    });
    render(<Page api={api} />);
    fireEvent.change(await screen.findByLabelText('Document title'), {
      target: { value: 'Private draft' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    back();
    await act(async () => refuse(new DocumentError('Access unavailable', status)));
    expect(screen.queryByLabelText('Document content')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Stay' })).not.toBeInTheDocument();
    expect(documentDraft('owner', id)).toBeUndefined();
    expect(unloadBlocked()).toBe(false);
  },
);

it('replaces the viewer scope immediately and leaves a read-only viewer unguarded', async () => {
  const api = client();
  const rendered = render(<Page api={api} />);
  fireEvent.change(await screen.findByLabelText('Document title'), {
    target: { value: 'Private draft' },
  });
  back();
  vi.mocked(api.read).mockResolvedValue({ ...view, access: 'read' });
  rendered.rerender(<Page api={api} viewer="other" />);
  expect(screen.queryByDisplayValue('Private draft')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Stay' })).not.toBeInTheDocument();
  expect(unloadBlocked()).toBe(false);
  expect(await screen.findByLabelText('Document title')).toHaveValue(view.title);
  back();
  expect(screen.getByRole('heading', { name: 'Artifacts' })).toBeVisible();
});
