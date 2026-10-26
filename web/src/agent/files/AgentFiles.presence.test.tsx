import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../../api/context';
import { unavailableClient } from '../../api/client';
import { fileBase64, type AgentFilesClient } from '../../api/agentFiles';
import { AgentFiles } from './AgentFiles';

afterEach(() => vi.useRealTimers());

function fixture() {
  const files: AgentFilesClient = {
    list: vi.fn(async () => [
      { path: 'draft.txt', name: 'draft.txt', kind: 'file' as const, size_bytes: 5, hidden: false },
    ]),
    read: vi.fn(async () => ({
      path: 'draft.txt',
      size_bytes: 5,
      sha256: 'a'.repeat(64),
      content_base64: fileBase64(new TextEncoder().encode('hello')),
    })),
    write: vi.fn(),
    remove: vi.fn(),
    download: vi.fn(),
  };
  const client = { ...unavailableClient, files };
  const view = (active = true, agentId = 'agent-one') => (
    <CmaClientProvider client={client}>
      <AgentFiles agentId={agentId} active={active} />
    </CmaClientProvider>
  );
  return { files, view, ...render(view()) };
}

it('keeps draft confirmation interactive, then fades an inert dialog before unmounting', async () => {
  const { files } = fixture();
  const browse = screen.getByRole('button', { name: 'Browse files' });
  await userEvent.click(browse);
  await userEvent.click(await screen.findByRole('button', { name: 'draft.txt' }));
  const editor = await screen.findByRole('textbox', { name: 'Contents of draft.txt' });
  fireEvent.change(editor, { target: { value: 'unsaved work' } });
  const modal = document.querySelector('.modal');
  vi.useFakeTimers();
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  expect(modal).toHaveAttribute('data-presence', 'visible');
  expect(editor).toHaveValue('unsaved work');
  expect(screen.getByRole('alert')).toHaveTextContent('Discard');
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(editor).toHaveValue('unsaved work');
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  fireEvent.click(screen.getByRole('button', { name: 'Confirm' }));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(modal).toHaveAttribute('data-presence', 'exiting');
  expect(modal).toHaveAttribute('inert');
  expect(browse).toHaveFocus();
  expect(document.body.style.overflow).not.toBe('hidden');
  act(() => vi.advanceTimersByTime(199));
  expect(modal).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(1));
  expect(modal).not.toBeInTheDocument();
  expect(files.write).not.toHaveBeenCalled();
});

it('cancels a stale exit timer when Files is quickly reopened', async () => {
  fixture();
  const browse = screen.getByRole('button', { name: 'Browse files' });
  await userEvent.click(browse);
  await screen.findByRole('button', { name: 'draft.txt' });
  vi.useFakeTimers();
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  act(() => vi.advanceTimersByTime(100));
  fireEvent.click(browse);
  act(() => vi.advanceTimersByTime(200));
  expect(screen.getAllByRole('dialog', { name: 'Files' })).toHaveLength(1);
  expect(document.querySelector('.modal')).toHaveAttribute('data-presence', 'visible');
  expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
});

it('removes previews and cancels requests immediately when the panel becomes inactive', async () => {
  const { files, view, rerender } = fixture();
  let readSignal: AbortSignal | undefined;
  vi.mocked(files.read).mockImplementation((_id, _path, options) => {
    readSignal = options?.signal;
    return new Promise(() => {});
  });
  await userEvent.click(screen.getByRole('button', { name: 'Browse files' }));
  await userEvent.click(await screen.findByRole('button', { name: 'draft.txt' }));
  expect(readSignal?.aborted).toBe(false);
  rerender(view(false));
  expect(document.querySelector('.modal')).toBeNull();
  expect(readSignal?.aborted).toBe(true);
});

it('clears old previews and cancels requests immediately when the Agent changes', async () => {
  const { files, view, rerender } = fixture();
  let readSignal: AbortSignal | undefined;
  vi.mocked(files.read).mockImplementation((_id, _path, options) => {
    readSignal = options?.signal;
    return new Promise(() => {});
  });
  await userEvent.click(screen.getByRole('button', { name: 'Browse files' }));
  await userEvent.click(await screen.findByRole('button', { name: 'draft.txt' }));
  rerender(view(true, 'agent-two'));
  expect(readSignal?.aborted).toBe(true);
  expect(screen.queryByRole('textbox', { name: 'Contents of draft.txt' })).toBeNull();
  await screen.findByRole('button', { name: 'draft.txt' });
  expect(files.list).toHaveBeenLastCalledWith('agent-two', '', expect.any(Object));
});
