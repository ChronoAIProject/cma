import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { QueuedMessages, type QueuedMessageItem } from './QueuedMessages';
import {
  attachmentMarker,
  splitAttachmentMessage,
  type UploadAttachment,
  type UploadedAttachment,
} from './attachments';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const saved = {
  name: 'original.txt',
  path: '/home/attachments/agent/att_original/original.txt',
  media_type: 'text/plain',
  size_bytes: 4,
};
const first: QueuedMessageItem = {
  responseId: 'resp_one',
  commandId: 'cmd_one',
  revision: 3,
  input: `Original request${attachmentMarker}${JSON.stringify([saved], null, 2)}`,
};
const second: QueuedMessageItem = {
  responseId: 'resp_two',
  commandId: 'cmd_two',
  revision: 7,
  input: 'Another queued request',
};
const uploaded = (file: File): UploadedAttachment => ({
  id: 'att_revised',
  agentId: 'agent',
  name: file.name,
  path: `/home/attachments/agent/att_revised/${file.name}`,
  mediaType: file.type,
  size: file.size,
  sha256: 'a'.repeat(64),
});
const queued = (position: number) =>
  within(screen.getByRole('article', { name: `Queued message ${String(position)}` }));
const retained = () => within(screen.getByRole('article', { name: 'Retained message draft 1' }));
function finishExit() {
  act(() => vi.advanceTimersByTime(250));
  vi.useRealTimers();
}

it('keeps a dispatched edit and revised attachments until copied or dismissed without disturbing another draft', async () => {
  const user = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const edit = vi.fn().mockResolvedValue(undefined),
    remove = vi.fn();
  const upload = vi.fn<UploadAttachment>().mockImplementation(async (file) => uploaded(file));
  const draw = (items: readonly QueuedMessageItem[]) => (
    <QueuedMessages items={items} onEdit={edit} onRemove={remove} uploadAttachment={upload} />
  );
  const { rerender } = render(draw([first, second]));
  await user.click(queued(1).getByRole('button', { name: 'Edit' }));
  fireEvent.change(queued(1).getByRole('textbox'), {
    target: { value: 'Keep my revised request' },
  });
  await user.click(queued(1).getByRole('button', { name: 'Remove original.txt' }));
  const file = new File(['revised'], 'revised.txt', { type: 'text/plain' });
  fireEvent.change(queued(1).getByLabelText('Choose attachments'), { target: { files: [file] } });
  await queued(1).findByText('Ready');
  await user.click(queued(2).getByRole('button', { name: 'Edit' }));
  fireEvent.change(queued(2).getByRole('textbox'), {
    target: { value: 'Independent second draft' },
  });

  vi.useFakeTimers();
  rerender(draw([second]));
  finishExit();
  expect(retained().getByRole('textbox')).toHaveValue('Keep my revised request');
  expect(retained().getByRole('textbox')).toHaveAttribute('readonly');
  expect(retained().getByText('revised.txt')).toBeVisible();
  expect(retained().queryByRole('list', { name: 'Saved attachments' })).not.toBeInTheDocument();
  expect(retained().getByText(/no longer queued and cannot be edited/)).toBeVisible();
  expect(retained().queryByText('Queued', { exact: true })).not.toBeInTheDocument();
  expect(retained().queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(retained().queryByRole('button', { name: 'Remove' })).not.toBeInTheDocument();
  fireEvent.keyDown(retained().getByRole('textbox'), { key: 'Enter', ctrlKey: true });
  fireEvent.submit(retained().getByRole('form'));
  await user.click(retained().getByRole('button', { name: 'Copy draft' }));
  expect(splitAttachmentMessage(copy.mock.calls[0]![0])).toEqual({
    text: 'Keep my revised request',
    files: [
      { name: file.name, path: uploaded(file).path, media_type: file.type, size_bytes: file.size },
    ],
  });
  expect(queued(1).getByRole('textbox')).toHaveValue('Independent second draft');
  await user.click(retained().getByRole('button', { name: 'Dismiss' }));
  expect(
    screen.queryByRole('article', { name: 'Retained message draft 1' }),
  ).not.toBeInTheDocument();
  expect(queued(1).getByRole('textbox')).toHaveValue('Independent second draft');
  expect(edit).not.toHaveBeenCalled();
  expect(remove).not.toHaveBeenCalled();
  expect(upload).toHaveBeenCalledOnce();
});

it('retains the last editor through dispatch, finishes or retries its existing upload, and preserves failed clipboard copies', async () => {
  const user = userEvent.setup();
  vi.spyOn(window, 'matchMedia').mockImplementation(
    () =>
      ({
        matches: true,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      }) as unknown as MediaQueryList,
  );
  const copy = vi
    .spyOn(navigator.clipboard, 'writeText')
    .mockRejectedValueOnce(new Error('denied'))
    .mockResolvedValue();
  let reject!: (error: Error) => void;
  const upload = vi
    .fn<UploadAttachment>()
    .mockImplementationOnce(
      () =>
        new Promise((_, fail) => {
          reject = fail;
        }),
    )
    .mockImplementation(async (file) => uploaded(file));
  const edit = vi.fn(),
    remove = vi.fn();
  const draw = (items: readonly QueuedMessageItem[], disabled = false) => (
    <QueuedMessages
      items={items}
      disabled={disabled}
      onEdit={edit}
      onRemove={remove}
      uploadAttachment={upload}
    />
  );
  const { rerender } = render(draw([first]));
  await user.click(queued(1).getByRole('button', { name: 'Edit' }));
  fireEvent.change(queued(1).getByRole('textbox'), {
    target: { value: 'Retain pending attachment' },
  });
  const file = new File(['pending'], 'pending.txt', { type: 'text/plain' });
  fireEvent.change(queued(1).getByLabelText('Choose attachments'), { target: { files: [file] } });
  await waitFor(() => expect(upload).toHaveBeenCalledOnce());
  vi.useFakeTimers();
  rerender(draw([]));
  finishExit();
  expect(screen.getByRole('heading', { name: 'Retained drafts 1' })).toBeVisible();
  expect(retained().getByRole('button', { name: 'Copy draft' })).toBeEnabled();
  expect(retained().getByText(/Unfinished uploads are not included/)).toBeVisible();
  expect(upload.mock.calls[0]![1].signal.aborted).toBe(false);
  await act(async () => reject(new Error('Upload interrupted')));
  rerender(draw([], true));
  expect(retained().getByRole('button', { name: 'Retry pending.txt' })).toBeDisabled();
  expect(retained().getByRole('button', { name: 'Copy draft' })).toBeEnabled();
  expect(retained().getByRole('button', { name: 'Dismiss' })).toBeEnabled();
  rerender(draw([]));
  await user.click(retained().getByRole('button', { name: 'Retry pending.txt' }));
  await retained().findByText('Ready');
  expect(upload.mock.calls[1]![1].idempotencyKey).toBe(upload.mock.calls[0]![1].idempotencyKey);
  await user.click(retained().getByRole('button', { name: 'Copy draft' }));
  expect(retained().getByRole('alert')).toHaveTextContent('draft could not be copied');
  expect(retained().getByRole('textbox')).toHaveValue('Retain pending attachment');
  await user.click(retained().getByRole('button', { name: 'Copy draft' }));
  expect(splitAttachmentMessage(copy.mock.calls[1]![0])?.files).toEqual([
    saved,
    { name: file.name, path: uploaded(file).path, media_type: file.type, size_bytes: file.size },
  ]);
  await user.click(retained().getByRole('button', { name: 'Dismiss' }));
  expect(screen.queryByRole('region', { name: 'Retained drafts' })).not.toBeInTheDocument();
  expect(document.querySelector('form[aria-label="Retained message draft"]')).toBeNull();
  expect(edit).not.toHaveBeenCalled();
  expect(remove).not.toHaveBeenCalled();
});

it('keeps an edit that loses the dispatch race while its save acknowledgment is pending', async () => {
  let refuse!: (error: Error) => void;
  const edit = vi.fn(
    () =>
      new Promise<void>((_, reject) => {
        refuse = reject;
      }),
  );
  const draw = (items: readonly QueuedMessageItem[]) => (
    <QueuedMessages items={items} onEdit={edit} onRemove={vi.fn()} />
  );
  const { rerender } = render(draw([first]));
  await userEvent.click(queued(1).getByRole('button', { name: 'Edit' }));
  fireEvent.change(queued(1).getByRole('textbox'), { target: { value: 'Save lost the race' } });
  await userEvent.click(queued(1).getByRole('button', { name: 'Save' }));
  rerender(draw([]));
  await act(async () => refuse(new Error('This message already started.')));
  expect(retained().getByRole('textbox')).toHaveValue('Save lost the race');
  expect(retained().getByRole('alert')).toHaveTextContent('already started');
  expect(retained().getByRole('button', { name: 'Copy draft' })).toBeEnabled();
  expect(edit).toHaveBeenCalledExactlyOnceWith(
    first,
    expect.stringContaining('Save lost the race'),
  );
});
