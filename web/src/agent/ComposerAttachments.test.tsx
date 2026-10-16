import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Composer } from './Composer';
import { useAgentDraft } from './draft';
import { agentClient } from '../test/agent';
import type { UploadAttachment, UploadedAttachment } from './attachments';
import { useComposerAttachments } from './useComposerAttachments';

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function receipt(file: File): UploadedAttachment {
  return {
    id: 'att_1',
    agentId: 'agt_1',
    name: file.name,
    path: `/home/attachments/agt_1/att_1/${file.name}`,
    size: file.size,
    mediaType: file.type,
    sha256: 'a'.repeat(64),
  };
}

function surface(upload: UploadAttachment, onSend = vi.fn(), allowed = true) {
  return (
    <Composer
      status="ready"
      running={false}
      busy={false}
      inputAvailability={{ allowed }}
      uploadAttachment={upload}
      onSend={onSend}
      onStop={vi.fn()}
    />
  );
}

it('pastes an image, blocks sending until persisted, and hands its preview to the outgoing message immediately', async () => {
  const create = vi.fn(() => 'blob:image');
  const revoke = vi.fn();
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke }));
  let complete!: (value: UploadedAttachment) => void;
  const upload = vi.fn(
    () =>
      new Promise<UploadedAttachment>((resolve) => {
        complete = resolve;
      }),
  );
  let accept!: () => void;
  const send = vi.fn((_text: string, callback: () => void) => {
    accept = callback;
  });
  render(surface(upload, send));
  const file = new File(['png'], 'pasted.png', { type: 'image/png' });
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.paste(field, { clipboardData: { files: [file] } });
  await waitFor(() => expect(upload).toHaveBeenCalledOnce());
  expect(screen.getByText('Uploading…')).toBeInTheDocument();
  fireEvent.change(field, { target: { value: 'Explain this image' } });
  fireEvent.keyDown(field, { key: 'Enter' });
  expect(send).not.toHaveBeenCalled();
  await act(async () => complete(receipt(file)));
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(send.mock.calls[0]![0]).toContain('Explain this image');
  expect(send.mock.calls[0]![0]).toContain(receipt(file).path);
  expect(screen.queryByText('pasted.png')).not.toBeInTheDocument();
  expect(field).toHaveValue('');
  act(() => accept());
  expect(field).toHaveValue('');
  expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
  expect(create).toHaveBeenCalledWith(file);
  expect(revoke).toHaveBeenCalledWith('blob:image');
});

it('drops files, retains failed uploads and retries the identical file with the same key', async () => {
  const file = new File(['contents'], 'notes.txt', { type: 'text/plain' });
  const upload = vi
    .fn<UploadAttachment>()
    .mockRejectedValueOnce(new Error('Connection lost. Retry this upload.'))
    .mockResolvedValueOnce(receipt(file));
  const send = vi.fn();
  render(surface(upload, send));
  fireEvent.drop(screen.getByRole('textbox'), { dataTransfer: { files: [file] } });
  expect(await screen.findByText('Connection lost. Retry this upload.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  expect(await screen.findByText('Ready')).toBeInTheDocument();
  expect(upload.mock.calls[1]![0]).toBe(file);
  expect(upload.mock.calls[1]![1].idempotencyKey).toBe(upload.mock.calls[0]![1].idempotencyKey);
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(send.mock.calls[0]![0]).toContain('Please review the attached files.');
  expect(send.mock.calls[0]![0]).toContain(receipt(file).path);
});

it('reads screenshot file items when the clipboard file list is empty', async () => {
  const create = vi.fn(() => 'blob:screenshot');
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: vi.fn() }));
  const file = new File(['screenshot bytes'], 'image.png', { type: 'image/png' });
  const upload = vi.fn<UploadAttachment>().mockResolvedValue(receipt(file));
  render(surface(upload));
  const consumed = fireEvent.paste(screen.getByRole('textbox'), {
    clipboardData: {
      files: [],
      items: [
        { kind: 'string', getAsFile: () => null },
        { kind: 'file', getAsFile: () => file },
      ],
    },
  });
  expect(consumed).toBe(false);
  expect(screen.getByText('image.png')).toBeInTheDocument();
  expect(await screen.findByText('Ready')).toBeInTheDocument();
  expect(upload).toHaveBeenCalledExactlyOnceWith(file, expect.any(Object));
  expect(create).toHaveBeenCalledWith(file);
});

it('uploads each transferred file once when both files and items expose it', async () => {
  const file = new File(['contents'], 'notes.txt', { type: 'text/plain' });
  const upload = vi.fn<UploadAttachment>().mockResolvedValue(receipt(file));
  render(surface(upload));
  fireEvent.paste(screen.getByRole('textbox'), {
    clipboardData: {
      files: [file],
      items: [{ kind: 'file', getAsFile: () => file }],
    },
  });
  await screen.findByText('Ready');
  expect(upload).toHaveBeenCalledOnce();
  expect(screen.getAllByRole('listitem')).toHaveLength(1);
});

it('preserves ordinary text paste and explains when image uploading is unavailable', () => {
  render(
    <Composer
      status="ready"
      running={false}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={vi.fn()}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox');
  expect(
    fireEvent.paste(field, {
      clipboardData: { files: [], items: [{ kind: 'string', getAsFile: () => null }] },
    }),
  ).toBe(true);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  fireEvent.paste(field, {
    clipboardData: {
      files: [],
      items: [
        { kind: 'file', getAsFile: () => new File(['image'], 'image.png', { type: 'image/png' }) },
      ],
    },
  });
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Attachments are available when this sandbox is ready.',
  );
});

it('cancels a removed upload and ignores its late response', async () => {
  let finish!: (value: UploadedAttachment) => void;
  const upload = vi.fn<UploadAttachment>(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const file = new File(['private'], 'private.txt');
  render(surface(upload));
  fireEvent.change(screen.getByLabelText('Choose attachments'), { target: { files: [file] } });
  await waitFor(() => expect(upload).toHaveBeenCalledOnce());
  fireEvent.click(screen.getByRole('button', { name: 'Remove private.txt' }));
  expect(upload.mock.calls[0]![1].signal.aborted).toBe(true);
  await act(async () => finish(receipt(file)));
  expect(screen.queryByText('private.txt')).not.toBeInTheDocument();
});

it('enforces file/count bounds and refuses uploads without current input authority', async () => {
  const upload = vi.fn<UploadAttachment>();
  const view = render(surface(upload));
  const huge = new File(['a'], 'huge.bin');
  Object.defineProperty(huge, 'size', { value: 20 * 1024 * 1024 + 1 });
  const field = screen.getByRole('textbox');
  fireEvent.drop(field, { dataTransfer: { files: [huge] } });
  expect(screen.getByRole('alert')).toHaveTextContent('20 MiB');
  fireEvent.drop(field, {
    dataTransfer: { files: Array.from({ length: 11 }, () => new File(['a'], 'file.txt')) },
  });
  expect(screen.getByRole('alert')).toHaveTextContent('10 files');
  view.rerender(surface(upload, vi.fn(), false));
  fireEvent.paste(field, { clipboardData: { files: [new File(['a'], 'file.txt')] } });
  expect(screen.getByRole('alert')).toHaveTextContent('sandbox is ready');
  expect(upload).not.toHaveBeenCalled();
});

it('dismisses a rejected oversized selection while preserving the draft and ready attachments', async () => {
  const file = new File(['notes'], 'notes.txt');
  const huge = new File(['a'], 'huge.bin');
  Object.defineProperty(huge, 'size', { value: 20 * 1024 * 1024 + 1 });
  const upload = vi.fn<UploadAttachment>().mockResolvedValue(receipt(file));
  render(surface(upload));
  const field = screen.getByRole('textbox');
  fireEvent.change(field, { target: { value: 'Keep my message' } });
  fireEvent.drop(field, { dataTransfer: { files: [file] } });
  await screen.findByText('Ready');
  fireEvent.drop(field, { dataTransfer: { files: [huge] } });
  expect(screen.getByRole('alert')).toHaveTextContent('20 MiB');
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss attachment notice' }));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(field).toHaveValue('Keep my message');
  expect(screen.getByText('notes.txt')).toBeInTheDocument();
  expect(screen.getByText('Ready')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  expect(upload).toHaveBeenCalledExactlyOnceWith(file, expect.any(Object));
  fireEvent.drop(field, { dataTransfer: { files: [huge] } });
  expect(screen.getByRole('alert')).toHaveTextContent('20 MiB');
});

it('expires selection notices after five seconds and gives repeated rejections a fresh interval', () => {
  vi.useFakeTimers();
  const upload = vi.fn<UploadAttachment>();
  const huge = new File(['a'], 'huge.bin');
  Object.defineProperty(huge, 'size', { value: 20 * 1024 * 1024 + 1 });
  render(surface(upload));
  const field = screen.getByRole('textbox');
  fireEvent.drop(field, { dataTransfer: { files: [huge] } });
  act(() => vi.advanceTimersByTime(4000));
  fireEvent.drop(field, { dataTransfer: { files: [huge] } });
  act(() => vi.advanceTimersByTime(1000));
  expect(screen.getByRole('alert')).toHaveTextContent('20 MiB');
  act(() => vi.advanceTimersByTime(3999));
  expect(screen.getByRole('alert')).toHaveTextContent('20 MiB');
  act(() => vi.advanceTimersByTime(1));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  act(() => vi.advanceTimersByTime(200));
  expect(screen.queryByText('Each attachment must be 20 MiB or smaller.')).not.toBeInTheDocument();
  expect(upload).not.toHaveBeenCalled();
});

it('keeps failed uploads and their retry actions after a selection notice expires', async () => {
  const file = new File(['notes'], 'notes.txt');
  const upload = vi
    .fn<UploadAttachment>()
    .mockRejectedValue(new Error('Upload failed. Try again.'));
  render(surface(upload));
  const field = screen.getByRole('textbox');
  fireEvent.drop(field, { dataTransfer: { files: [file] } });
  await screen.findByText('Upload failed. Try again.');
  vi.useFakeTimers();
  fireEvent.drop(field, {
    dataTransfer: { files: Array.from({ length: 10 }, () => new File(['a'], 'extra.txt')) },
  });
  expect(screen.getByRole('alert')).toHaveTextContent('10 files');
  act(() => vi.advanceTimersByTime(5000));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByText('notes.txt')).toBeInTheDocument();
  expect(screen.getByText('Upload failed. Try again.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Retry' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(upload).toHaveBeenCalledOnce();
});

it('clears a selection notice on a valid add and cancels its dismissal timer on unmount', async () => {
  vi.useFakeTimers();
  const file = new File(['notes'], 'notes.txt');
  const upload = vi.fn<UploadAttachment>().mockResolvedValue(receipt(file));
  const { result, unmount } = renderHook(() => useComposerAttachments(undefined, upload, true));
  const tooMany = Array.from({ length: 11 }, () => file);
  act(() => result.current.add(tooMany));
  expect(result.current.error).toContain('10 files');
  expect(vi.getTimerCount()).toBe(1);
  await act(async () => result.current.add([file]));
  expect(result.current.error).toBeUndefined();
  expect(vi.getTimerCount()).toBe(0);
  expect(result.current.items[0]?.status).toBe('ready');
  act(() => result.current.add(tooMany));
  expect(vi.getTimerCount()).toBe(1);
  unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('retains ready attachments and text across visits, aborts pending uploads, and isolates viewers', async () => {
  const client = agentClient();
  let pendingSignal: AbortSignal | undefined;
  const upload = vi.fn<UploadAttachment>((file, options) => {
    if (file.name === 'pending.txt') {
      pendingSignal = options.signal;
      return new Promise(() => undefined);
    }
    return Promise.resolve(receipt(file));
  });
  function Visit({ id, viewer = 'owner' }: { id: string; viewer?: string }) {
    const draft = useAgentDraft(client, viewer, id);
    return (
      <Composer
        key={`${viewer}:${id}`}
        draft={draft}
        status="ready"
        running={false}
        busy={false}
        inputAvailability={{ allowed: true }}
        uploadAttachment={upload}
        onSend={vi.fn()}
        onStop={vi.fn()}
      />
    );
  }
  const view = render(<Visit id="agt_1" />);
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Keep my draft' } });
  fireEvent.drop(screen.getByRole('textbox'), {
    dataTransfer: { files: [new File(['a'], 'ready.txt')] },
  });
  await screen.findByText('Ready');
  fireEvent.drop(screen.getByRole('textbox'), {
    dataTransfer: { files: [new File(['b'], 'pending.txt')] },
  });
  await waitFor(() => expect(pendingSignal).toBeDefined());
  view.rerender(<Visit id="agt_2" />);
  expect(pendingSignal?.aborted).toBe(true);
  expect(screen.queryByText('ready.txt')).not.toBeInTheDocument();
  expect(screen.getByRole('textbox')).toHaveValue('');
  view.rerender(<Visit id="agt_1" />);
  expect(screen.getByRole('textbox')).toHaveValue('Keep my draft');
  expect(screen.getByText('ready.txt')).toBeInTheDocument();
  expect(await screen.findByText('Upload interrupted. Retry to continue.')).toBeInTheDocument();
  view.rerender(<Visit id="agt_1" viewer="other" />);
  expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
  expect(screen.getByRole('textbox')).toHaveValue('');
});

it('limits concurrent uploads to two and drains the queue after completion', async () => {
  const finish: (() => void)[] = [];
  const upload = vi.fn<UploadAttachment>(
    (file) =>
      new Promise((resolve) => {
        finish.push(() => resolve(receipt(file)));
      }),
  );
  render(surface(upload));
  fireEvent.drop(screen.getByRole('textbox'), {
    dataTransfer: { files: [1, 2, 3].map((id) => new File(['a'], `${String(id)}.txt`)) },
  });
  await waitFor(() => expect(upload).toHaveBeenCalledTimes(2));
  expect(screen.getByText('Queued')).toBeInTheDocument();
  await act(async () => finish[0]!());
  await waitFor(() => expect(upload).toHaveBeenCalledTimes(3));
});
