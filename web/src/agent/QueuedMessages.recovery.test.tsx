import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { QueuedMessages, type QueuedMessageItem } from './QueuedMessages';
import { attachmentMarker } from './attachments';

const text = `Revised instruction\n保留原文${attachmentMarker}${JSON.stringify([
  {
    name: 'notes.txt',
    path: '/home/attachments/notes.txt',
    media_type: 'text/plain',
    size_bytes: 8,
  },
])}`;
const pending: QueuedMessageItem = {
  responseId: 'resp_one',
  commandId: 'cmd_one',
  revision: 7,
  input: 'Original queued message',
  needsRecovery: true,
  pendingEdit: text,
};

it.each([false, true])(
  'keeps a recovered edit readable when clipboard failure is %s',
  async (fails) => {
    const user = userEvent.setup();
    const copy = vi.spyOn(navigator.clipboard, 'writeText');
    if (fails) copy.mockRejectedValue(new Error('Denied'));
    else copy.mockResolvedValue();
    const edit = vi.fn(),
      remove = vi.fn(),
      recover = vi.fn().mockRejectedValue(new Error('Still uncertain'));
    const neighbor = {
      responseId: 'resp_two',
      commandId: 'cmd_two',
      revision: 3,
      input: 'Neighbor',
    };
    const draw = (item = pending) => (
      <QueuedMessages
        items={[item, neighbor]}
        onEdit={edit}
        onRemove={remove}
        onRecover={recover}
      />
    );
    const view = render(draw());
    const field = screen.getByRole('textbox', { name: 'Edit awaiting confirmation' });
    expect(field).toHaveValue(text);
    expect(field).toHaveAttribute('readonly');
    expect(screen.getByText('Original queued message')).toBeVisible();
    const row = within(screen.getByRole('article', { name: 'Queued message 1' }));
    expect(row.getByRole('button', { name: 'Edit queued message content' })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(row.getByRole('button', { name: 'Remove queued message 1' })).toBeDisabled();
    await user.click(
      within(screen.getByRole('article', { name: 'Queued message 2' })).getByRole('button', {
        name: 'Edit queued message content',
      }),
    );
    fireEvent.change(screen.getByRole('textbox', { name: 'Queued message' }), {
      target: { value: 'Neighbor draft' },
    });
    await user.click(row.getByRole('button', { name: 'Copy edit' }));
    expect(copy).toHaveBeenCalledExactlyOnceWith(text);
    expect(await screen.findByRole('status')).toHaveTextContent(
      fails ? 'Select and copy' : 'Edit copied',
    );
    expect(field).toHaveValue(text);
    expect(edit).not.toHaveBeenCalled();
    expect(remove).not.toHaveBeenCalled();
    expect(recover).not.toHaveBeenCalled();
    await user.click(row.getByRole('button', { name: 'Recover request' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Still uncertain');
    expect(recover).toHaveBeenCalledExactlyOnceWith(pending);
    expect(field).toHaveValue(text);
    expect(screen.getByRole('textbox', { name: 'Queued message' })).toHaveValue('Neighbor draft');
    view.rerender(draw({ ...pending, needsRecovery: false, pendingEdit: undefined }));
    expect(screen.queryByRole('textbox', { name: 'Edit awaiting confirmation' })).toBeNull();
    copy.mockRestore();
  },
);
