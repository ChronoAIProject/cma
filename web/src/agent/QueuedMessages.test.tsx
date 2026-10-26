import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { QueuedMessages, type QueuedMessageItem } from './QueuedMessages';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const first: QueuedMessageItem = {
  responseId: 'resp_one',
  commandId: 'cmd_one',
  revision: 3,
  input: 'First queued message',
};
const second: QueuedMessageItem = {
  responseId: 'resp_two',
  commandId: 'cmd_two',
  revision: 7,
  input: 'Second queued message',
};
const row = (position: number) =>
  within(screen.getByRole('article', { name: `Queued message ${String(position)}` }));

it('edits and removes only the chosen entry, keeping another row draft and original queue position', async () => {
  const edit = vi.fn().mockResolvedValue(undefined),
    remove = vi.fn().mockResolvedValue(undefined);
  const draw = (items: readonly QueuedMessageItem[]) => (
    <QueuedMessages
      items={items}
      onEdit={edit}
      onRemove={remove}
      user={{ displayName: 'Sam Owner', handle: 'sam' }}
    />
  );
  const { rerender } = render(draw([first, second]));
  expect(screen.getAllByText('Sam Owner')).toHaveLength(2);
  await userEvent.click(row(2).getByRole('button', { name: 'Edit' }));
  fireEvent.change(row(2).getByRole('textbox'), { target: { value: 'Second draft remains' } });
  await userEvent.click(row(1).getByRole('button', { name: 'Edit queued message content' }));
  fireEvent.change(row(1).getByRole('textbox'), { target: { value: 'Edited first' } });
  await userEvent.click(row(1).getByRole('button', { name: 'Save' }));
  expect(edit).toHaveBeenCalledExactlyOnceWith(first, 'Edited first');
  const changed = { ...first, revision: 4, input: 'Edited first' };
  rerender(draw([changed, second]));
  expect(row(2).getByRole('textbox')).toHaveValue('Second draft remains');
  expect(row(1).getByRole('button', { name: 'Edit queued message content' })).toHaveTextContent(
    'Edited first',
  );
  await userEvent.click(row(1).getByRole('button', { name: 'Remove' }));
  expect(remove).toHaveBeenCalledExactlyOnceWith(changed);
  rerender(draw([second]));
  expect(row(1).getByRole('textbox')).toHaveValue('Second draft remains');
  expect(edit).toHaveBeenCalledOnce();
});

it('uses the original revision if a refresh races an edit and retains a refused edit', async () => {
  const edit = vi
    .fn()
    .mockRejectedValue(new Error('This message has started. It can no longer be edited.'));
  const remove = vi.fn();
  const { rerender } = render(<QueuedMessages items={[first]} onEdit={edit} onRemove={remove} />);
  await userEvent.click(screen.getByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'My revised request' } });
  rerender(
    <QueuedMessages
      items={[{ ...first, revision: 4, input: 'Refreshed original' }]}
      onEdit={edit}
      onRemove={remove}
    />,
  );
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(edit).toHaveBeenCalledExactlyOnceWith(first, 'My revised request');
  expect(await screen.findByRole('alert')).toHaveTextContent('This message has started');
  expect(screen.getByRole('textbox')).toHaveValue('My revised request');
  expect(remove).not.toHaveBeenCalled();
});

it('opens from the keyboard, returns focus on Cancel, and resets a cancelled draft on a quick reopen', async () => {
  const edit = vi.fn(),
    remove = vi.fn();
  render(<QueuedMessages items={[first]} onEdit={edit} onRemove={remove} />);
  const content = screen.getByRole('button', { name: 'Edit queued message content' });
  content.focus();
  fireEvent.keyDown(content, { key: 'Enter' });
  expect(screen.getByRole('textbox')).toHaveFocus();
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Cancelled draft' } });
  fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' });
  expect(content).toHaveFocus();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  fireEvent.keyDown(content, { key: ' ' });
  expect(screen.getByRole('textbox')).toHaveValue(first.input);
  expect(edit).not.toHaveBeenCalled();
  expect(remove).not.toHaveBeenCalled();
});

it('locks only an unresolved row and keeps its edit available for explicit recovery', async () => {
  const edit = vi.fn().mockRejectedValue(new Error('Update outcome unknown'));
  const remove = vi.fn().mockResolvedValue(undefined),
    recover = vi.fn().mockResolvedValue(undefined);
  const draw = (items: readonly QueuedMessageItem[]) => (
    <QueuedMessages items={items} onEdit={edit} onRemove={remove} onRecover={recover} />
  );
  const { rerender } = render(draw([first, second]));
  await userEvent.click(row(1).getByRole('button', { name: 'Edit' }));
  fireEvent.change(row(1).getByRole('textbox'), { target: { value: 'Recover this edit' } });
  await userEvent.click(row(1).getByRole('button', { name: 'Save' }));
  const pending = { ...first, needsRecovery: true };
  rerender(draw([pending, second]));
  expect(row(1).getByRole('textbox')).toBeDisabled();
  expect(row(1).getByRole('textbox')).toHaveValue('Recover this edit');
  expect(row(1).getByRole('button', { name: 'Remove' })).toBeDisabled();
  expect(row(2).getByRole('button', { name: 'Remove' })).toBeEnabled();
  await userEvent.click(row(1).getByRole('button', { name: 'Recover request' }));
  expect(recover).toHaveBeenCalledExactlyOnceWith(pending);
  await waitFor(() => expect(row(1).queryByRole('textbox')).not.toBeInTheDocument());
});

it('retains a removed row only as an inert exit frame and disables unavailable actions', () => {
  vi.useFakeTimers();
  const edit = vi.fn(),
    remove = vi.fn();
  const draw = (items: readonly QueuedMessageItem[]) => (
    <QueuedMessages items={items} disabled onEdit={edit} onRemove={remove} />
  );
  const { rerender } = render(draw([first, second]));
  const removed = screen.getByRole('article', { name: 'Queued message 1' }).closest('li')!;
  expect(row(1).getByRole('button', { name: 'Edit' })).toBeDisabled();
  fireEvent.click(row(1).getByRole('button', { name: 'Edit queued message content' }));
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  rerender(draw([second]));
  expect(removed).toHaveAttribute('data-presence', 'exiting');
  expect(removed).toHaveAttribute('inert');
  expect(screen.getAllByRole('article')).toHaveLength(1);
  act(() => vi.advanceTimersByTime(200));
  expect(removed).not.toBeInTheDocument();
});

it('removes the exit frame immediately for reduced motion', () => {
  vi.spyOn(window, 'matchMedia').mockImplementation(
    () =>
      ({
        matches: true,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      }) as unknown as MediaQueryList,
  );
  const edit = vi.fn(),
    remove = vi.fn();
  const { rerender } = render(
    <QueuedMessages items={[first, second]} onEdit={edit} onRemove={remove} />,
  );
  rerender(<QueuedMessages items={[second]} onEdit={edit} onRemove={remove} />);
  expect(screen.queryByText(first.input)).not.toBeInTheDocument();
  expect(screen.getByText(second.input)).toBeVisible();
});
