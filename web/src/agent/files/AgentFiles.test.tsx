import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { FileExplorer } from './AgentFiles';
import {
  AgentFileError,
  fileBase64,
  type AgentFilesClient,
  type AgentFileEntry,
} from '../../api/agentFiles';
const file: AgentFileEntry = {
  path: 'hello.txt',
  name: 'hello.txt',
  kind: 'file',
  size_bytes: 5,
  hidden: false,
};
const hidden: AgentFileEntry = {
  path: '.codex',
  name: '.codex',
  kind: 'directory',
  size_bytes: 0,
  hidden: true,
};
const encode = (text: string) => fileBase64(new TextEncoder().encode(text));
const original = {
  path: file.path,
  size_bytes: 5,
  sha256: 'a'.repeat(64),
  content_base64: encode('hello'),
};
function fixture(readOnly = false, overrides: Partial<AgentFilesClient> = {}) {
  const client: AgentFilesClient = {
    list: vi.fn(async (_id, path) => (path === '' ? [file, hidden] : [])),
    read: vi.fn(async () => original),
    download: vi.fn(async () => new Blob(['hello'])),
    write: vi.fn(async (_id, input) => ({
      path: input.path,
      size_bytes: 5,
      sha256: 'b'.repeat(64),
    })),
    remove: vi.fn(async () => {}),
    ...overrides,
  };
  const onClose = vi.fn();
  const onAccessChanged = vi.fn();
  render(
    <FileExplorer
      client={client}
      agentId="agent-one"
      readOnly={readOnly}
      onClose={onClose}
      onAccessChanged={onAccessChanged}
    />,
  );
  return { client, onClose, onAccessChanged };
}
it('loads only home root, reveals hidden folders on demand and opens text', async () => {
  const { client } = fixture();
  await screen.findByRole('button', { name: 'hello.txt' });
  expect(screen.queryByRole('button', { name: '.codex' })).not.toBeInTheDocument();
  expect(client.list).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('checkbox', { name: 'Show hidden' }));
  await userEvent.click(screen.getByRole('button', { name: '.codex' }));
  expect(client.list).toHaveBeenLastCalledWith(
    'agent-one',
    '.codex',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  await userEvent.click(screen.getByRole('button', { name: 'hello.txt' }));
  expect(await screen.findByRole('textbox', { name: 'Contents of hello.txt' })).toHaveValue(
    'hello',
  );
});
it('saves with original version and keeps draft after a concurrent edit conflict', async () => {
  const write = vi
    .fn()
    .mockRejectedValue(
      new AgentFileError('The file changed; refresh before saving.', 409, 'file_conflict'),
    );
  fixture(false, { write });
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  const editor = await screen.findByRole('textbox', { name: 'Contents of hello.txt' });
  fireEvent.change(editor, { target: { value: 'updated' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(write).toHaveBeenCalledWith(
    'agent-one',
    expect.objectContaining({
      path: file.path,
      expected_sha256: original.sha256,
      content_base64: encode('updated'),
    }),
    expect.any(Object),
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('file changed');
  expect(editor).toHaveValue('updated');
  expect(screen.getByLabelText('Unsaved changes')).toBeInTheDocument();
});
it('view-only allows preview/download and exposes no mutations', async () => {
  const { client } = fixture(true);
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  expect(await screen.findByRole('textbox', { name: 'Contents of hello.txt' })).toHaveAttribute(
    'readonly',
  );
  for (const name of ['New file', 'New folder', 'Upload', 'Save', 'Delete file'])
    expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Download file' })).toBeEnabled();
  expect(client.write).not.toHaveBeenCalled();
  expect(client.remove).not.toHaveBeenCalled();
});
it('copies the selected absolute path without writing file contents', async () => {
  const user = userEvent.setup();
  const clipboard = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const { client } = fixture(true);
  await user.click(await screen.findByRole('button', { name: 'hello.txt' }));
  await user.click(screen.getByRole('button', { name: 'Copy path' }));
  expect(clipboard).toHaveBeenCalledExactlyOnceWith('/home/hello.txt');
  expect(await screen.findByText('Path copied.')).toBeInTheDocument();
  expect(client.write).not.toHaveBeenCalled();
  expect(client.remove).not.toHaveBeenCalled();
});
it('confirms draft discard and deletion inline without nested dialogs', async () => {
  const { onClose, client } = fixture();
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  fireEvent.change(await screen.findByRole('textbox', { name: 'Contents of hello.txt' }), {
    target: { value: 'draft' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Close' }));
  expect(onClose).not.toHaveBeenCalled();
  expect(screen.getByRole('alert')).toHaveTextContent('Discard');
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await userEvent.click(screen.getByRole('button', { name: 'Delete file' }));
  expect(client.remove).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));
  await waitFor(() =>
    expect(client.remove).toHaveBeenCalledWith('agent-one', file.path, false, expect.any(Object)),
  );
});
it('creates folders with create-only intent', async () => {
  const { client } = fixture();
  await screen.findByRole('button', { name: 'hello.txt' });
  await userEvent.click(screen.getByRole('button', { name: 'New folder' }));
  await userEvent.type(screen.getByRole('textbox', { name: 'Folder name' }), 'research');
  await userEvent.click(screen.getByRole('button', { name: /^Create$/ }));
  await waitFor(() =>
    expect(client.write).toHaveBeenCalledWith(
      'agent-one',
      { path: 'research', content_base64: '', directory: true },
      expect.any(Object),
    ),
  );
});
it('preserves an unrelated file and its unsaved draft when deleting a folder', async () => {
  let folderExists = true;
  const folder = { ...hidden, path: 'unrelated', name: 'unrelated', hidden: false };
  const { client } = fixture(false, {
    list: vi.fn(async (_id, path) =>
      path === '' ? [file, ...(folderExists ? [folder] : [])] : [],
    ),
    remove: vi.fn(async () => {
      folderExists = false;
    }),
  });
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  const editor = await screen.findByRole('textbox', { name: 'Contents of hello.txt' });
  fireEvent.change(editor, { target: { value: 'Keep my unsaved work' } });
  await userEvent.click(screen.getByRole('button', { name: 'unrelated' }));
  await userEvent.click(screen.getByRole('button', { name: 'Delete folder' }));
  await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));
  await waitFor(() =>
    expect(screen.queryByRole('button', { name: 'unrelated' })).not.toBeInTheDocument(),
  );
  expect(editor).toBeInTheDocument();
  expect(editor).toHaveValue('Keep my unsaved work');
  expect(screen.getByLabelText('Unsaved changes')).toBeInTheDocument();
  expect(client.remove).toHaveBeenCalledWith('agent-one', 'unrelated', true, expect.any(Object));
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() =>
    expect(client.write).toHaveBeenCalledWith(
      'agent-one',
      expect.objectContaining({
        path: file.path,
        content_base64: encode('Keep my unsaved work'),
        expected_sha256: original.sha256,
      }),
      expect.any(Object),
    ),
  );
});
it('lets an unrelated file read finish after a folder is deleted', async () => {
  let finish!: (content: typeof original) => void;
  let signal: AbortSignal | undefined;
  fixture(false, {
    read: vi.fn((_id, _path, options) => {
      signal = options?.signal;
      return new Promise<typeof original>((resolve) => {
        finish = resolve;
      });
    }),
  });
  await userEvent.click(screen.getByRole('checkbox', { name: 'Show hidden' }));
  await userEvent.click(await screen.findByRole('button', { name: '.codex' }));
  await userEvent.click(screen.getByRole('button', { name: 'Delete folder' }));
  // The current selection can change while the inline confirmation remains open.
  await userEvent.click(screen.getByRole('button', { name: 'hello.txt' }));
  await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));
  await screen.findByText('Deleted.');
  expect(signal?.aborted).toBe(false);
  finish(original);
  expect(await screen.findByRole('textbox', { name: 'Contents of hello.txt' })).toHaveValue(
    'hello',
  );
});
it('clears the selected preview when its containing folder is deleted', async () => {
  const nested = { ...file, path: '.codex/hello.txt' };
  const { client } = fixture(false, {
    list: vi.fn(async (_id, path) => (path === '' ? [hidden] : [nested])),
    read: vi.fn(async () => ({ ...original, path: nested.path })),
  });
  await userEvent.click(screen.getByRole('checkbox', { name: 'Show hidden' }));
  await userEvent.click(await screen.findByRole('button', { name: '.codex' }));
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  await screen.findByRole('textbox', { name: 'Contents of hello.txt' });
  await userEvent.click(screen.getByRole('button', { name: 'Delete folder' }));
  await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));
  await screen.findByText('Deleted.');
  expect(screen.queryByRole('textbox', { name: 'Contents of hello.txt' })).not.toBeInTheDocument();
  expect(client.remove).toHaveBeenCalledWith('agent-one', '.codex', true, expect.any(Object));
});
it('clears file data and mutations after access revocation', async () => {
  const { onAccessChanged } = fixture(false, {
    read: vi.fn().mockRejectedValue(new AgentFileError('Access removed.', 403)),
  });
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  await screen.findByRole('alert');
  expect(onAccessChanged).toHaveBeenCalledOnce();
  expect(screen.queryByRole('button', { name: 'hello.txt' })).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: /Contents/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Upload' })).not.toBeInTheDocument();
});
it('uploads a selected file without overwriting an existing version', async () => {
  const { client } = fixture();
  await screen.findByRole('button', { name: 'hello.txt' });
  const upload = new File(['sample'], 'note.md', { type: 'text/markdown' });
  Object.defineProperty(upload, 'arrayBuffer', {
    value: async () => new TextEncoder().encode('sample').buffer,
  });
  fireEvent.change(screen.getByLabelText('Upload files'), { target: { files: [upload] } });
  await waitFor(() =>
    expect(client.write).toHaveBeenCalledWith(
      'agent-one',
      { path: 'note.md', content_base64: encode('sample') },
      expect.any(Object),
    ),
  );
});

it('keeps the selected file stable while its save is pending', async () => {
  let finish!: (value: { path: string; size_bytes: number; sha256: string }) => void;
  const write = vi.fn(
    () =>
      new Promise<{ path: string; size_bytes: number; sha256: string }>((resolve) => {
        finish = resolve;
      }),
  );
  fixture(false, { write });
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  fireEvent.change(await screen.findByRole('textbox', { name: 'Contents of hello.txt' }), {
    target: { value: 'saved draft' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(screen.getByRole('button', { name: 'hello.txt' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Sandbox root /home/' })).toBeDisabled();
  finish({ path: 'hello.txt', size_bytes: 11, sha256: 'b'.repeat(64) });
  await waitFor(() => expect(screen.getByRole('button', { name: 'hello.txt' })).toBeEnabled());
  expect(screen.getByRole('textbox', { name: 'Contents of hello.txt' })).toHaveValue('saved draft');
});

it.each(['../qa2.txt', 'folder/file.txt', 'folder\\file.txt', '.', '..', 'bad\u0001name', '   '])(
  'rejects invalid names before submit and allows correction: %j',
  async (name) => {
    const { client } = fixture();
    await screen.findByRole('button', { name: 'hello.txt' });
    await userEvent.click(screen.getByRole('button', { name: 'New file' }));
    const input = screen.getByRole('textbox', { name: 'File name' });
    fireEvent.change(input, { target: { value: name } });
    const submit = screen.getByRole('button', { name: /^Create$/ });
    expect(submit).toBeDisabled();
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription(
      'Enter a file or folder name without path separators.',
    );
    fireEvent.submit(submit.closest('form')!);
    expect(client.write).not.toHaveBeenCalled();
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: 'valid-note.txt' } });
    expect(submit).toBeEnabled();
    expect(input).not.toHaveAttribute('aria-invalid', 'true');
    await userEvent.click(submit);
    await waitFor(() =>
      expect(client.write).toHaveBeenCalledExactlyOnceWith(
        'agent-one',
        { path: 'valid-note.txt', content_base64: '', directory: false },
        expect.any(Object),
      ),
    );
  },
);

it('keeps invalid names disabled after a server rejection without losing the editable name', async () => {
  const { client } = fixture(false, {
    write: vi.fn().mockRejectedValue(new AgentFileError('Name already exists.', 409)),
  });
  await screen.findByRole('button', { name: 'hello.txt' });
  await userEvent.click(screen.getByRole('button', { name: 'New folder' }));
  const input = screen.getByRole('textbox', { name: 'Folder name' });
  fireEvent.change(input, { target: { value: 'existing' } });
  await userEvent.click(screen.getByRole('button', { name: /^Create$/ }));
  await screen.findByText('Name already exists.');
  expect(input).toHaveValue('existing');
  fireEvent.change(input, { target: { value: '../invalid' } });
  expect(screen.getByRole('button', { name: /^Create$/ })).toBeDisabled();
  fireEvent.submit(input.closest('form')!);
  expect(client.write).toHaveBeenCalledTimes(1);
});

it('clears a failed listing alert once refreshing that folder succeeds', async () => {
  let failing = true;
  const list = vi.fn(async (_id: string, path: string) => {
    if (failing) throw new AgentFileError('Files are temporarily unavailable.', 503);
    return path === '' ? [file] : [];
  });
  fixture(false, { list });
  expect(await screen.findByRole('alert')).toHaveTextContent('temporarily unavailable');
  failing = false;
  await userEvent.click(screen.getByRole('button', { name: 'Refresh files' }));
  await screen.findByRole('button', { name: 'hello.txt' });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});
it('keeps a save failure visible when an unrelated folder refresh succeeds', async () => {
  const write = vi
    .fn()
    .mockRejectedValue(
      new AgentFileError('The file changed; refresh before saving.', 409, 'file_conflict'),
    );
  const { client } = fixture(false, { write });
  await userEvent.click(await screen.findByRole('button', { name: 'hello.txt' }));
  const editor = await screen.findByRole('textbox', { name: 'Contents of hello.txt' });
  fireEvent.change(editor, { target: { value: 'updated' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('file changed');
  await userEvent.click(screen.getByRole('button', { name: 'Refresh files' }));
  await waitFor(() => expect(client.list).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('alert')).toHaveTextContent('file changed');
});

it.each([
  ['a'.repeat(256) + '.txt', 'a'.repeat(251) + '.txt'],
  ['界'.repeat(86), '界'.repeat(85)],
])(
  'refuses a filename over the UTF-8 byte limit and allows the boundary',
  async (invalid, valid) => {
    const { client } = fixture();
    await screen.findByRole('button', { name: 'hello.txt' });
    await userEvent.click(screen.getByRole('button', { name: 'New file' }));
    const input = screen.getByRole('textbox', { name: 'File name' });
    fireEvent.change(input, { target: { value: invalid } });
    const submit = screen.getByRole('button', { name: /^Create$/ });
    expect(submit).toBeDisabled();
    expect(input).toHaveAccessibleDescription(
      'File and folder names must be at most 255 UTF-8 bytes.',
    );
    fireEvent.submit(submit.closest('form')!);
    expect(client.write).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: valid } });
    expect(submit).toBeEnabled();
    await userEvent.click(submit);
    await waitFor(() =>
      expect(client.write).toHaveBeenCalledExactlyOnceWith(
        'agent-one',
        { path: valid, content_base64: '', directory: false },
        expect.any(Object),
      ),
    );
  },
);
