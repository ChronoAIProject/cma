import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { v2CommandStatus } from '../api-client/responses-v2/controls-types';
import { attachmentMarker } from './attachments';
import { QueuedInputOutcomes } from './QueuedInputOutcomes';
import { commandOutcome, type QueuedInputOutcome } from './transport/queuedCommandOutcomes';
import { status } from './transport/queuedReconciliation.fixture';

afterEach(() => vi.unstubAllGlobals());

const expired = (input?: string): QueuedInputOutcome => ({
  commandId: 'command',
  responseId: 'response',
  ...commandOutcome(v2CommandStatus(status(1, 'expired'))),
  input,
});

it('restores and copies exact readable text without refreshing or submitting', async () => {
  const restore = vi.fn();
  const refresh = vi.fn();
  const copy = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal('navigator', { clipboard: { writeText: copy } });
  const text = '  Retained message\n第二行  ';
  render(<QueuedInputOutcomes items={[expired(text)]} onRefresh={refresh} onRestore={restore} />);
  expect(screen.getByText('Expired before starting')).toBeVisible();
  expect(screen.getByText('Nothing will be sent until you choose Send.')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  expect(restore).toHaveBeenCalledExactlyOnceWith(text);
  fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));
  await screen.findByText('Message text copied.');
  expect(copy).toHaveBeenCalledExactlyOnceWith(text);
  expect(refresh).not.toHaveBeenCalled();
});

it.each([
  ['queue_timeout', 'dispatch_fenced'],
  ['queue_timeout', 'write_confirmed'],
  ['queue_timeout', 'execution_observed'],
  ['queue_timeout', 'runtime_lost'],
  ['queue_timeout', 'recovery_required'],
  ['runtime_lost', null],
  ['runtime_lost', 'runtime_lost'],
  ['runtime_lost', 'recovery_required'],
  ['authority_revoked', null],
] as const)('never offers restoration for %s with delivery %s', (failure, delivery) => {
  const parsed = v2CommandStatus({
    ...status(1, 'expired'),
    failure,
    delivery,
    turn_sequence: delivery === null ? null : 1,
  });
  const restore = vi.fn();
  render(
    <QueuedInputOutcomes
      items={[{ ...expired('Readable message'), ...commandOutcome(parsed) }]}
      onRefresh={vi.fn()}
      onRestore={restore}
    />,
  );
  expect(screen.queryByRole('button', { name: 'Restore to draft' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Copy message' })).not.toBeInTheDocument();
  expect(restore).not.toHaveBeenCalled();
});

it.each([undefined, '', ' \n '])('explains unavailable text without inventing it (%s)', (text) => {
  render(<QueuedInputOutcomes items={[expired(text)]} onRefresh={vi.fn()} onRestore={vi.fn()} />);
  expect(screen.getByText('Message text is unavailable. It cannot be restored.')).toBeVisible();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it.each([
  JSON.stringify([
    {
      name: 'budget.txt',
      path: '/home/attachments/budget',
      media_type: 'text/plain',
      size_bytes: 6,
    },
  ]),
  JSON.stringify([{ name: 'image.png', path: 'blob:expired-preview' }]),
  '{unreadable attachment metadata',
])('restores text only and explains attachment reselection (%s)', async (manifest) => {
  const restore = vi.fn();
  const copy = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal('navigator', { clipboard: { writeText: copy } });
  render(
    <QueuedInputOutcomes
      items={[expired(`Read these files${attachmentMarker}${manifest}`)]}
      onRefresh={vi.fn()}
      onRestore={restore}
    />,
  );
  expect(
    screen.getByText('Attachments are not restored. Select the files again before sending.'),
  ).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  expect(restore).toHaveBeenCalledExactlyOnceWith('Read these files');
  fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));
  await waitFor(() => expect(copy).toHaveBeenCalledExactlyOnceWith('Read these files'));
});

it('keeps the original readable text when clipboard access fails', async () => {
  vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockRejectedValue(new Error()) } });
  render(<QueuedInputOutcomes items={[expired('Keep me')]} onRefresh={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));
  await screen.findByText('Could not copy. Select the message text and copy it manually.');
  expect(screen.getByText('Keep me')).toBeVisible();
});
