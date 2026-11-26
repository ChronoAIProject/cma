import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { FileExplorer } from './FileExplorer';
import { FileNavigationMemory } from './useFileNavigation';
import { AgentFiles } from './AgentFiles';
import { CmaClientProvider } from '../../api/context';
import { unavailableClient } from '../../api/client';
import {
  AgentFileError,
  fileBase64,
  type AgentFileEntry,
  type AgentFilesClient,
} from '../../api/agentFiles';

afterEach(() => vi.restoreAllMocks());
const directory = 'cma/project/.worktrees/wt-fixture';
const name = '笔记 with spaces.md';
const path = `${directory}/${name}`;
const encode = (text: string) => fileBase64(new TextEncoder().encode(text));
const entry: AgentFileEntry = { path, name, kind: 'file', size_bytes: 0, hidden: false };
function filesFixture() {
  let content = '';
  let exists = false;
  const files: AgentFilesClient = {
    list: vi.fn(async (_id, root) => (root === directory && exists ? [entry] : [])),
    read: vi.fn(async (_id, file) => ({
      path: file,
      size_bytes: content.length,
      sha256: 'a'.repeat(64),
      content_base64: content,
    })),
    write: vi.fn(async (_id, input) => {
      exists = true;
      content = input.content_base64;
      return { path: input.path, sha256: 'a'.repeat(64), size_bytes: content.length };
    }),
    download: vi.fn(),
    remove: vi.fn(),
  };
  return files;
}

it('uses the same nested path for creation, listing, preview, save and copy', async () => {
  const user = userEvent.setup();
  const clipboard = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const files = filesFixture();
  render(
    <FileExplorer
      client={files}
      agentId="agent-one"
      initialDirectory={directory}
      readOnly={false}
      onClose={vi.fn()}
    />,
  );
  expect(screen.getByLabelText('Sandbox root')).toHaveTextContent('/home/');
  for (const label of ['Working directory', 'Tree root', 'Current directory'])
    expect(screen.getByLabelText(label)).toHaveTextContent(`/home/${directory}`);
  await waitFor(() =>
    expect(files.list).toHaveBeenCalledWith('agent-one', directory, expect.any(Object)),
  );
  await user.click(screen.getByRole('button', { name: 'New file' }));
  await user.type(screen.getByRole('textbox', { name: 'File name' }), name);
  await user.click(screen.getByRole('button', { name: 'Create' }));
  const editor = await screen.findByRole('textbox', { name: `Contents of ${name}` });
  expect(files.write).toHaveBeenCalledWith(
    'agent-one',
    { path, content_base64: '', directory: false },
    expect.any(Object),
  );
  expect(files.read).toHaveBeenCalledWith('agent-one', path, expect.any(Object));
  expect(screen.getByRole('button', { name })).toHaveAttribute('title', `/home/${path}`);
  expect(screen.getByLabelText('Selected file')).toHaveTextContent(`/home/${path}`);
  fireEvent.change(editor, { target: { value: 'Same synthetic file' } });
  await user.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('File saved.');
  await user.click(screen.getByRole('button', { name }));
  await waitFor(() => expect(editor).toHaveValue('Same synthetic file'));
  expect(files.write).toHaveBeenLastCalledWith(
    'agent-one',
    { path, content_base64: encode('Same synthetic file'), expected_sha256: 'a'.repeat(64) },
    expect.any(Object),
  );
  await user.click(screen.getByRole('button', { name: 'Copy path' }));
  expect(clipboard).toHaveBeenCalledExactlyOnceWith(`/home/${path}`);
  expect(files.write).toHaveBeenCalledTimes(2);
  await user.click(screen.getByRole('button', { name: 'Sandbox root /home/' }));
  expect(screen.getByLabelText('Current directory')).toHaveTextContent('/home/');
  expect(screen.getByLabelText('Working directory')).toHaveTextContent(`/home/${directory}`);
  await waitFor(() =>
    expect(files.list).toHaveBeenLastCalledWith('agent-one', '', expect.any(Object)),
  );
  const upload = new File(['uploaded'], 'upload.txt');
  Object.defineProperty(upload, 'arrayBuffer', {
    value: async () => new TextEncoder().encode('uploaded').buffer,
  });
  fireEvent.change(screen.getByLabelText('Upload files'), { target: { files: [upload] } });
  await waitFor(() =>
    expect(files.write).toHaveBeenLastCalledWith(
      'agent-one',
      { path: 'upload.txt', content_base64: encode('uploaded') },
      expect.any(Object),
    ),
  );
});

it.each(['denied', 'missing'] as const)(
  'keeps a full selectable path and manual guidance when clipboard is %s',
  async (failure) => {
    const user = userEvent.setup();
    if (failure === 'denied')
      vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue(new Error('Denied'));
    else vi.spyOn(navigator, 'clipboard', 'get').mockReturnValue(undefined as unknown as Clipboard);
    const files = filesFixture();
    vi.mocked(files.list).mockResolvedValue([entry]);
    render(
      <FileExplorer
        client={files}
        agentId="agent-one"
        initialDirectory={directory}
        readOnly
        onClose={vi.fn()}
      />,
    );
    await user.click(await screen.findByRole('button', { name }));
    await user.click(screen.getByRole('button', { name: 'Copy path' }));
    expect(
      await screen.findByText('Could not copy. Select the full path above and copy it manually.'),
    ).toBeVisible();
    expect(within(screen.getByLabelText('Selected file')).getByText(`/home/${path}`)).toBeVisible();
    expect(files.write).not.toHaveBeenCalled();
    expect(files.remove).not.toHaveBeenCalled();
  },
);

it.each([undefined, '', '/outside', '/home/../outside'])(
  'keeps unknown or invalid metadata %j separate from the root',
  async (workingDirectory) => {
    const files = filesFixture();
    render(
      <CmaClientProvider client={{ ...unavailableClient, files }}>
        <AgentFiles agentId="agent-one" active workingDirectory={workingDirectory} />
      </CmaClientProvider>,
    );
    expect(screen.getByLabelText('Working directory')).toHaveTextContent('Unknown');
    await userEvent.click(screen.getByRole('button', { name: 'Browse files' }));
    await waitFor(() =>
      expect(files.list).toHaveBeenCalledWith('agent-one', '', expect.any(Object)),
    );
    expect(screen.queryByRole('button', { name: 'Copy path' })).toBeNull();
  },
);

it('adopts late metadata only before interaction and preserves a draft through later metadata', async () => {
  const files = filesFixture();
  vi.mocked(files.list).mockImplementation(async (_id, root) =>
    root === directory ? [entry] : [],
  );
  const view = (initialDirectory?: string) => (
    <FileExplorer
      client={files}
      agentId="agent-one"
      initialDirectory={initialDirectory}
      readOnly={false}
      onClose={vi.fn()}
    />
  );
  const { rerender } = render(view());
  await waitFor(() => expect(files.list).toHaveBeenCalledWith('agent-one', '', expect.any(Object)));
  rerender(view(directory));
  await userEvent.click(await screen.findByRole('button', { name }));
  const editor = await screen.findByRole('textbox', { name: `Contents of ${name}` });
  fireEvent.change(editor, { target: { value: 'Keep this draft' } });
  rerender(view('cma/another'));
  expect(screen.getByLabelText('Working directory')).toHaveTextContent('/home/cma/another');
  expect(screen.getByLabelText('Current directory')).toHaveTextContent(`/home/${directory}`);
  expect(editor).toHaveValue('Keep this draft');
  expect(files.write).not.toHaveBeenCalled();
});

it('ignores a late old-Agent listing when switching to another working directory', async () => {
  const files = filesFixture();
  let finish!: (value: readonly AgentFileEntry[]) => void;
  let signal: AbortSignal | undefined;
  vi.mocked(files.list).mockImplementation(async (id, _root, options) => {
    if (id !== 'agent-one') return [];
    signal = options?.signal;
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  const view = (agentId: string, workingDirectory: string) => (
    <CmaClientProvider client={{ ...unavailableClient, files }}>
      <AgentFiles agentId={agentId} active workingDirectory={workingDirectory} />
    </CmaClientProvider>
  );
  const { rerender } = render(view('agent-one', `/home/${directory}`));
  await userEvent.click(screen.getByRole('button', { name: 'Browse files' }));
  await waitFor(() => expect(files.list).toHaveBeenCalledOnce());
  rerender(view('agent-two', '/home/cma/second'));
  await waitFor(() =>
    expect(files.list).toHaveBeenLastCalledWith('agent-two', 'cma/second', expect.any(Object)),
  );
  expect(signal?.aborted).toBe(true);
  await act(async () => finish([entry]));
  expect(screen.queryByRole('button', { name })).toBeNull();
  expect(screen.getByLabelText('Current directory')).toHaveTextContent('/home/cma/second');
});

it.each([true, false])(
  'retains the last successful directory across access rechecks (readOnly=%s)',
  async (readOnly) => {
    const files = filesFixture();
    const memory = new FileNavigationMemory();
    const folder = (name: string): AgentFileEntry => ({
      path: name,
      name,
      kind: 'directory',
      size_bytes: 0,
      hidden: false,
    });
    let finishOld!: (entries: readonly AgentFileEntry[]) => void;
    vi.mocked(files.list).mockImplementation(async (_id, path) => {
      if (!path) return [folder('old'), folder('cma'), folder('denied')];
      if (path === 'old')
        return new Promise((resolve) => {
          finishOld = resolve;
        });
      if (path === 'denied') throw new AgentFileError('Access denied. Check access again.', 403);
      return [];
    });
    const onAccessChanged = vi.fn();
    const view = (active = true) => (
      <FileExplorer
        client={files}
        agentId="agent-one"
        readOnly={readOnly}
        navigationMemory={memory}
        onClose={vi.fn()}
        active={active}
        onAccessChanged={onAccessChanged}
      />
    );
    const user = userEvent.setup();
    const first = render(view());
    await user.click(await screen.findByRole('button', { name: 'old' }));
    await user.click(screen.getByRole('button', { name: 'cma' }));
    await waitFor(() =>
      expect(screen.getByLabelText('Current directory')).toHaveTextContent('/home/cma'),
    );
    await act(async () => finishOld([]));
    expect(screen.getByLabelText('Current directory')).toHaveTextContent('/home/cma');
    await user.click(screen.getByRole('button', { name: 'denied' }));
    expect(await screen.findByText('Access denied. Check access again.')).toBeVisible();
    expect(screen.getByLabelText('Current directory')).toHaveTextContent('/home/cma');
    expect(memory.read()).toBe('cma');
    expect(onAccessChanged).toHaveBeenCalledOnce();
    first.unmount();
    const reopened = render(view(false));
    expect(screen.getByRole('status')).toHaveTextContent('Checking file access');
    reopened.rerender(view());
    await waitFor(() =>
      expect(files.list).toHaveBeenLastCalledWith('agent-one', 'cma', expect.any(Object)),
    );
    expect(screen.getByLabelText('Current directory')).toHaveTextContent('/home/cma');
  },
);
