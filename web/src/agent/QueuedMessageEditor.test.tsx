import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { QueuedMessageEditor } from './QueuedMessageEditor';
import {
  attachmentMarker,
  splitAttachmentMessage,
  type MessageAttachment,
  type UploadedAttachment,
  type UploadAttachment,
} from './attachments';

afterEach(() => vi.unstubAllGlobals());
const saved: MessageAttachment = {
  name: 'existing.png',
  path: '/home/attachments/agent/existing.png',
  media_type: 'image/png',
  size_bytes: 4096,
};
const withFiles = (files: readonly MessageAttachment[]) =>
  `Original text${attachmentMarker}${JSON.stringify(files, null, 2)}`;
const uploaded = (file: File): UploadedAttachment => ({
  id: 'att_new',
  agentId: 'agent',
  name: file.name,
  path: `/home/attachments/agent/att_new/${file.name}`,
  mediaType: file.type,
  size: file.size,
  sha256: 'a'.repeat(64),
});

it('preserves existing uploaded references without reuploading or inventing image previews', async () => {
  const upload = vi.fn<UploadAttachment>(),
    save = vi.fn().mockResolvedValue(undefined);
  const { container } = render(
    <QueuedMessageEditor
      input={withFiles([saved])}
      disabled={false}
      uploadAttachment={upload}
      onSave={save}
      onCancel={vi.fn()}
    />,
  );
  expect(screen.getByRole('textbox')).toHaveValue('Original text');
  expect(screen.getByRole('list', { name: 'Saved attachments' })).toHaveTextContent('existing.png');
  expect(container.querySelector('img')).toBeNull();
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Changed text' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(splitAttachmentMessage(save.mock.calls[0]![0])).toEqual({
    text: 'Changed text',
    files: [saved],
  });
  expect(upload).not.toHaveBeenCalled();
});

it('uploads a pasted image once, keeps existing files, and waits for its server path before saving', async () => {
  let complete!: (value: UploadedAttachment) => void;
  const file = new File(['image bytes'], 'pasted.png', { type: 'image/png' });
  const create = vi.fn(() => 'blob:new-preview'),
    revoke = vi.fn();
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke }));
  const upload = vi.fn<UploadAttachment>(
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  const save = vi.fn().mockResolvedValue(undefined);
  const { unmount } = render(
    <QueuedMessageEditor
      input={withFiles([saved])}
      disabled={false}
      uploadAttachment={upload}
      onSave={save}
      onCancel={vi.fn()}
    />,
  );
  fireEvent.paste(screen.getByRole('textbox'), {
    clipboardData: { files: [], items: [{ kind: 'file', getAsFile: () => file }] },
  });
  await waitFor(() => expect(upload).toHaveBeenCalledOnce());
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(create).toHaveBeenCalledExactlyOnceWith(file);
  await act(async () => complete(uploaded(file)));
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(splitAttachmentMessage(save.mock.calls[0]![0])?.files).toEqual([
    saved,
    { name: file.name, path: uploaded(file).path, media_type: file.type, size_bytes: file.size },
  ]);
  unmount();
  expect(revoke).toHaveBeenCalledWith('blob:new-preview');
});

it('supports file picker and drop, removes saved and new files independently, and retries an upload identity', async () => {
  const note = new File(['notes'], 'notes.txt', { type: 'text/plain' });
  const dropped = new File(['report'], 'report.txt', { type: 'text/plain' });
  const upload = vi
    .fn<UploadAttachment>()
    .mockRejectedValueOnce(new Error('Upload interrupted'))
    .mockImplementation(async (file) => uploaded(file));
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <QueuedMessageEditor
      input={withFiles([saved])}
      disabled={false}
      uploadAttachment={upload}
      onSave={save}
      onCancel={vi.fn()}
    />,
  );
  fireEvent.change(screen.getByLabelText('Choose attachments'), { target: { files: [note] } });
  await screen.findByText('Upload interrupted');
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await screen.findByText('Ready');
  expect(upload.mock.calls[1]![1].idempotencyKey).toBe(upload.mock.calls[0]![1].idempotencyKey);
  fireEvent.drop(screen.getByRole('textbox'), { dataTransfer: { files: [dropped] } });
  await waitFor(() => expect(upload).toHaveBeenCalledTimes(3));
  await waitFor(() => expect(screen.getAllByText('Ready')).toHaveLength(2));
  await userEvent.click(screen.getByRole('button', { name: 'Remove existing.png' }));
  await userEvent.click(screen.getByRole('button', { name: 'Remove report.txt' }));
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(splitAttachmentMessage(save.mock.calls[0]![0])?.files).toEqual([
    { name: note.name, path: uploaded(note).path, media_type: note.type, size_bytes: note.size },
  ]);
});

it('enforces one ten-file limit across existing and newly uploaded attachments', async () => {
  const originals = Array.from({ length: 10 }, (_, index) => ({
    ...saved,
    name: `saved-${String(index)}.png`,
    path: `/home/attachments/agent/saved-${String(index)}.png`,
  }));
  const file = new File(['notes'], 'new.txt', { type: 'text/plain' });
  const upload = vi.fn<UploadAttachment>().mockImplementation(async (file) => uploaded(file));
  const save = vi.fn().mockResolvedValue(undefined);
  render(
    <QueuedMessageEditor
      input={withFiles(originals)}
      disabled={false}
      uploadAttachment={upload}
      onSave={save}
      onCancel={vi.fn()}
    />,
  );
  fireEvent.drop(screen.getByRole('textbox'), { dataTransfer: { files: [file] } });
  expect(await screen.findByRole('alert')).toHaveTextContent('Attach up to 10 files');
  expect(upload).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Remove saved-0.png' }));
  fireEvent.drop(screen.getByRole('textbox'), { dataTransfer: { files: [file] } });
  await screen.findByText('Ready');
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(splitAttachmentMessage(save.mock.calls[0]![0])?.files).toHaveLength(10);
});

it('keeps a failed edit and respects IME confirmation, modifier Enter, and empty input', async () => {
  const save = vi.fn().mockRejectedValue(new Error('Dispatch won the race'));
  render(
    <QueuedMessageEditor input="Original" disabled={false} onSave={save} onCancel={vi.fn()} />,
  );
  const input = screen.getByRole('textbox');
  fireEvent.change(input, { target: { value: 'Edited request' } });
  fireEvent.compositionStart(input);
  fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
  expect(save).not.toHaveBeenCalled();
  fireEvent.compositionEnd(input);
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 229, ctrlKey: true });
  expect(save).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
  expect(await screen.findByRole('alert')).toHaveTextContent('Dispatch won the race');
  expect(input).toHaveValue('Edited request');
  expect(save).toHaveBeenCalledExactlyOnceWith('Edited request');
  fireEvent.change(input, { target: { value: '  ' } });
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
});

it('aborts an outstanding upload when the editor is dismissed', async () => {
  const upload = vi.fn<UploadAttachment>(() => new Promise(() => {}));
  const { unmount } = render(
    <QueuedMessageEditor
      input="Original"
      disabled={false}
      uploadAttachment={upload}
      onSave={vi.fn()}
      onCancel={vi.fn()}
    />,
  );
  fireEvent.drop(screen.getByRole('textbox'), {
    dataTransfer: { files: [new File(['notes'], 'pending.txt', { type: 'text/plain' })] },
  });
  await waitFor(() => expect(upload).toHaveBeenCalledOnce());
  expect(
    within(screen.getByRole('list', { name: 'Attachments' })).getByText('Uploading…'),
  ).toBeVisible();
  unmount();
  expect(upload.mock.calls[0]![1].signal.aborted).toBe(true);
});
