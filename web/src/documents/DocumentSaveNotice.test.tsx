import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DocumentsClient, DocumentView } from '../api/documents';
import { DocumentPage } from './DocumentPage';
import { DocumentSaveNotice } from './DocumentSaveNotice';
import { documentDraft, retainDocumentDraft } from './drafts';

const callbacks = vi.hoisted(() => ({ save: () => {} }));
vi.mock('./RichDocumentEditor', () => ({
  default: ({
    source,
    editable,
    onChange,
    onSave,
  }: {
    source: string;
    editable: boolean;
    onChange: (text: string) => void;
    onSave: () => void;
  }) => {
    callbacks.save = onSave;
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
const id = `doc_${'e'.repeat(32)}`;
const document: DocumentView = {
  id,
  title: 'Original',
  owner_subject: 'owner',
  revision: 3,
  source_revision: 1,
  sha256: 'b'.repeat(64),
  size_bytes: 10,
  created_at: '',
  updated_at: '',
  origin_agent_id: null,
  access: 'owner',
};
const client = (update: DocumentsClient['update']) =>
  ({
    read: vi.fn().mockResolvedValue(document),
    source: vi.fn().mockResolvedValue({ id, revision: 1, markdown: '# Original' }),
    comments: vi.fn().mockResolvedValue({ items: [], next: null }),
    update,
  }) as unknown as DocumentsClient;
const mount = (api: DocumentsClient) =>
  render(<DocumentPage client={api} id={id} viewer="owner" onNavigate={vi.fn()} />);
const setText = (name: string, value: string) =>
  fireEvent.change(screen.getByRole('textbox', { name }), { target: { value } });
beforeEach(() => {
  retainDocumentDraft('owner', id);
  window.history.replaceState({}, '', '/');
});
afterEach(() => {
  vi.unstubAllGlobals();
});

it.each([false, true])(
  'keeps a newer draft separate from an uncertain save through retry (return to document: %s)',
  async (returnToDocument) => {
    let acknowledge!: (value: DocumentView) => void;
    const update = vi
      .fn<DocumentsClient['update']>()
      .mockRejectedValueOnce(new TypeError('Response lost'))
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            acknowledge = resolve;
          }),
      )
      .mockResolvedValueOnce({ ...document, title: 'Draft B', revision: 5, source_revision: 5 });
    const api = client(update);
    let mounted = mount(api);
    await screen.findByRole('textbox', { name: 'Document content' });
    setText('Document title', 'Submitted A');
    setText('Document content', '# Submitted A');
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByText('We can’t confirm your last save');
    const original = documentDraft('owner', id)!.saveIntent!;
    expect(screen.getByRole('status')).toHaveTextContent('Save not confirmed');
    expect(screen.getByRole('textbox', { name: 'Document content' })).toBeEnabled();
    setText('Document title', 'Draft B');
    setText('Document content', '# Draft B');
    expect(documentDraft('owner', id)?.saveIntent).toEqual(original);
    if (returnToDocument) {
      mounted.unmount();
      vi.mocked(api.read).mockResolvedValue({
        ...document,
        title: 'Submitted A',
        revision: 4,
        source_revision: 4,
      });
      vi.mocked(api.source).mockResolvedValue({ id, revision: 4, markdown: '# Submitted A' });
      mounted = mount(api);
      expect(await screen.findByRole('textbox', { name: 'Document content' })).toHaveValue(
        '# Draft B',
      );
    }
    fireEvent.click(screen.getByRole('button', { name: 'Retry same save' }));
    expect(screen.getByRole('button', { name: 'Retry same save' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Retry same save' }));
    act(() => callbacks.save());
    expect(update).toHaveBeenCalledTimes(2);
    expect(update.mock.calls[1]?.[1]).toEqual(original.body);
    expect(update.mock.calls[1]?.[2].idempotencyKey).toBe(original.key);
    await act(async () =>
      acknowledge({ ...document, title: 'Submitted A', revision: 4, source_revision: 4 }),
    );
    expect(screen.getByRole('textbox', { name: 'Document title' })).toHaveValue('Draft B');
    expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue('# Draft B');
    expect(screen.getByRole('status')).toHaveTextContent('Unsaved changes');
    expect(documentDraft('owner', id)).toMatchObject({
      title: 'Draft B',
      markdown: '# Draft B',
      sourceRevision: 4,
    });
    expect(documentDraft('owner', id)?.saveIntent).toBeUndefined();
    const unload = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Saved'));
    expect(update.mock.calls[2]?.[1]).toEqual({
      title: 'Draft B',
      markdown: '# Draft B',
      expected_revision: 4,
    });
    expect(update.mock.calls[2]?.[2].idempotencyKey).not.toBe(original.key);
    // The retained draft is cleared by a passive effect after the commit that
    // shows "Saved", so wait for the draft store itself rather than the status.
    await waitFor(() => expect(documentDraft('owner', id)).toBeUndefined());
    mounted.unmount();
  },
);

it('retries the valid original snapshot even when the newer draft is incomplete', async () => {
  const update = vi
    .fn<DocumentsClient['update']>()
    .mockRejectedValueOnce(new TypeError('Offline'))
    .mockResolvedValueOnce({ ...document, title: 'Submitted', revision: 4, source_revision: 4 });
  mount(client(update));
  await screen.findByRole('textbox', { name: 'Document title' });
  setText('Document title', 'Submitted');
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('We can’t confirm your last save');
  setText('Document title', '');
  setText('Document content', '');
  expect(screen.getByRole('button', { name: 'Retry same save' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry same save' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
  expect(update.mock.calls[1]?.[1]).toEqual(update.mock.calls[0]?.[1]);
  expect(screen.getByRole('textbox', { name: 'Document title' })).toHaveValue('');
  expect(screen.getByRole('textbox', { name: 'Document content' })).toHaveValue('');
  expect(documentDraft('owner', id)).toMatchObject({ title: '', markdown: '' });
});

it('copies the current editor text and reports success only after the clipboard accepts it', async () => {
  let finish!: () => void;
  const writeText = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  vi.stubGlobal('navigator', { clipboard: { writeText } });
  const onRetry = vi.fn();
  const rendered = render(
    <DocumentSaveNotice text="Original snapshot" busy={false} canRetry onRetry={onRetry} />,
  );
  rendered.rerender(
    <DocumentSaveNotice text="Current draft" busy={false} canRetry onRetry={onRetry} />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Copy all text' }));
  expect(writeText).toHaveBeenCalledExactlyOnceWith('Current draft');
  expect(screen.queryByText('All text copied.')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Copy all text' })).toBeDisabled();
  await act(async () => finish());
  expect(screen.getByText('All text copied.')).toBeVisible();
  expect(onRetry).not.toHaveBeenCalled();
  rendered.rerender(
    <DocumentSaveNotice text="Newer draft" busy={false} canRetry onRetry={onRetry} />,
  );
  expect(screen.queryByText('All text copied.')).not.toBeInTheDocument();
});

it.each(['denied', 'unavailable'])(
  'offers selectable current text when clipboard is %s',
  async (failure) => {
    vi.stubGlobal('navigator', {
      clipboard:
        failure === 'denied'
          ? { writeText: vi.fn().mockRejectedValue(new Error('Denied')) }
          : undefined,
    });
    const onRetry = vi.fn();
    const rendered = render(
      <DocumentSaveNotice text="Draft A" busy={false} canRetry onRetry={onRetry} />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Copy all text' }));
    const fallback = await screen.findByRole('textbox', { name: 'Document text to copy' });
    expect(fallback).toHaveValue('Draft A');
    rendered.rerender(
      <DocumentSaveNotice text="Draft B" busy={false} canRetry onRetry={onRetry} />,
    );
    expect(fallback).toHaveValue('Draft B');
    expect(fallback).toHaveAttribute('readonly');
    fireEvent.focus(fallback);
    expect((fallback as HTMLTextAreaElement).selectionEnd).toBe('Draft B'.length);
    expect(screen.queryByText('All text copied.')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry same save' })).toBeEnabled();
    expect(onRetry).not.toHaveBeenCalled();
  },
);

it('keeps retry subject to the existing busy and permission checks', () => {
  const onRetry = vi.fn();
  const rendered = render(<DocumentSaveNotice text="Draft" busy canRetry onRetry={onRetry} />);
  fireEvent.click(screen.getByRole('button', { name: 'Retry same save' }));
  rendered.rerender(
    <DocumentSaveNotice text="Draft" busy={false} canRetry={false} onRetry={onRetry} />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Retry same save' }));
  expect(onRetry).not.toHaveBeenCalled();
});
