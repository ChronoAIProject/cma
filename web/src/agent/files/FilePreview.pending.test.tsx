import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentFilesClient } from '../../api/rest/agentFiles';
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
import { AgentFileError, type AgentFilesClient } from '../../api/agentFiles';
import { FileExplorer } from './FileExplorer';

const file = { path: 'a.txt', name: 'a.txt', kind: 'file' as const, size_bytes: 2, hidden: false };
const content = { path: file.path, size_bytes: 2, sha256: 'a'.repeat(64), content_base64: 'aGk=' };

it('keeps navigation usable during a slow preview and exposes retry after failure', async () => {
  let reject!: (error: unknown) => void;
  let signal: AbortSignal | undefined;
  const client: AgentFilesClient = {
    list: vi.fn(async () => [file]),
    read: vi.fn((_agent, _path, options) => {
      signal = options?.signal;
      return new Promise<typeof content>((_resolve, fail) => {
        reject = fail;
      });
    }),
    write: vi.fn(),
    remove: vi.fn(),
    download: vi.fn(),
  };
  const close = vi.fn();
  const { unmount } = render(
    <FileExplorer client={client} agentId="one" readOnly={false} onClose={close} />,
  );
  fireEvent.click(await screen.findByRole('button', { name: file.name }));
  expect(screen.getByText('Opening file…')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Close' })).toBeEnabled();
  expect(screen.getByRole('button', { name: file.name })).toBeEnabled();
  reject(new AgentFileError('Preview timed out', 408, 'file_preview_timeout'));
  fireEvent.click(await screen.findByRole('button', { name: 'Retry preview' }));
  expect(client.read).toHaveBeenCalledTimes(2);
  expect(screen.getByText('Opening file…')).toBeInTheDocument();
  unmount();
  expect(signal?.aborted).toBe(true);
  expect(client.write).not.toHaveBeenCalled();
});

it('shows a successful fresh preview after failure without a mutation', async () => {
  const client: AgentFilesClient = {
    list: vi.fn(async () => [file]),
    read: vi.fn().mockRejectedValueOnce(new Error('Unavailable')).mockResolvedValueOnce(content),
    write: vi.fn(),
    remove: vi.fn(),
    download: vi.fn(),
  };
  render(<FileExplorer client={client} agentId="one" readOnly={false} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: file.name }));
  fireEvent.click(await screen.findByRole('button', { name: 'Retry preview' }));
  expect(await screen.findByRole('textbox', { name: 'Contents of a.txt' })).toHaveValue('hi');
  expect(client.read).toHaveBeenCalledTimes(2);
  expect(client.write).not.toHaveBeenCalled();
});

it('keeps the latest A selection when aborted A and B responses arrive late', async () => {
  const pending: { signal?: AbortSignal; resolve: (value: typeof content) => void }[] = [];
  const second = { ...file, path: 'b.txt', name: 'b.txt' };
  const client: AgentFilesClient = {
    list: vi.fn(async () => [file, second]),
    read: vi.fn(
      (_agent, _path, options) =>
        new Promise<typeof content>((resolve) => {
          pending.push({ signal: options?.signal, resolve });
        }),
    ),
    write: vi.fn(),
    remove: vi.fn(),
    download: vi.fn(),
  };
  render(<FileExplorer client={client} agentId="one" readOnly={false} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'a.txt' }));
  fireEvent.click(screen.getByRole('button', { name: 'b.txt' }));
  fireEvent.click(screen.getByRole('button', { name: 'a.txt' }));
  expect(pending).toHaveLength(3);
  expect(pending[0]!.signal?.aborted).toBe(true);
  expect(pending[1]!.signal?.aborted).toBe(true);
  pending[2]!.resolve(content);
  const editor = await screen.findByRole('textbox', { name: 'Contents of a.txt' });
  expect(editor).toHaveValue('hi');
  await act(async () => {
    pending[0]!.resolve({ ...content, content_base64: 'eHg=' });
    pending[1]!.resolve({ ...content, path: 'b.txt', content_base64: 'eHg=' });
  });
  expect(editor).toHaveValue('hi');
  expect(screen.queryByRole('textbox', { name: 'Contents of b.txt' })).toBeNull();
});

it('keeps explicit preview pacing through selection changes, hiding and remount', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const other = { ...file, name: 'b.txt', path: 'b.txt' };
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          code: 'store_failed',
          retryable: false,
          detail: 'private upstream content',
        }),
        { status: 503, headers: { 'Retry-After': '3' } },
      ),
    )
    .mockImplementation(async () => new Response(JSON.stringify(content)));
  vi.stubGlobal('fetch', fetcher);
  const client = {
    ...agentFilesClient(),
    list: vi.fn(async () => [file, other]),
    write: vi.fn(),
    remove: vi.fn(),
    download: vi.fn(),
  };
  const props = { client, agentId: 'one', readOnly: false, onClose: vi.fn() };
  const first = render(<FileExplorer {...props} />);
  await act(async () => vi.advanceTimersByTimeAsync(0));
  fireEvent.click(screen.getByRole('button', { name: 'a.txt' }));
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(screen.queryByText(/private upstream/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Retry preview' }));
  await act(async () => vi.advanceTimersByTimeAsync(500));
  fireEvent.click(screen.getByRole('button', { name: 'b.txt' }));
  first.rerender(<FileExplorer {...props} active={false} />);
  await act(async () => vi.advanceTimersByTimeAsync(500));
  first.unmount();
  expect(vi.getTimerCount()).toBe(0);
  const second = render(<FileExplorer {...props} />);
  await act(async () => vi.advanceTimersByTimeAsync(0));
  fireEvent.click(screen.getByRole('button', { name: 'a.txt' }));
  await act(async () => vi.advanceTimersByTimeAsync(1999));
  expect(fetcher).toHaveBeenCalledOnce();
  expect(screen.queryByRole('textbox', { name: 'Contents of a.txt' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Close' })).toBeEnabled();
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByRole('textbox', { name: 'Contents of a.txt' })).toHaveValue('hi');
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(client.write).not.toHaveBeenCalled();
  expect(client.remove).not.toHaveBeenCalled();
  expect(client.download).not.toHaveBeenCalled();
  second.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('clears preview access on a malformed HTTP 403 without displaying the body', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(content)))
    .mockResolvedValueOnce(new Response('<html>private upstream content</html>', { status: 403 }));
  vi.stubGlobal('fetch', fetcher);
  const accessChanged = vi.fn();
  const client = { ...agentFilesClient(), list: vi.fn(async () => [file]) };
  const view = render(
    <FileExplorer
      client={client}
      agentId="one"
      readOnly={false}
      onClose={vi.fn()}
      onAccessChanged={accessChanged}
    />,
  );
  await act(async () => vi.advanceTimersByTimeAsync(0));
  fireEvent.click(screen.getByRole('button', { name: 'a.txt' }));
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('textbox', { name: 'Contents of a.txt' })).toHaveValue('hi');
  fireEvent.click(screen.getByRole('button', { name: 'a.txt' }));
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(accessChanged).toHaveBeenCalledOnce();
  expect(screen.queryByRole('textbox', { name: 'Contents of a.txt' })).toBeNull();
  expect(screen.queryByText(/private upstream/)).toBeNull();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
