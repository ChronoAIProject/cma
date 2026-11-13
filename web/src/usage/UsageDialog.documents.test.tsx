import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { UsageDialog } from './UsageDialog';

const usage = {
  enforcement: 'enforced' as const,
  documents: { used: 37, limit: 100 },
  bytes: { used: 210 * 2 ** 20, limit: 2 ** 30 },
  objects: { used: 40, limit: 32768 },
  pending_deletions: 1,
};
it('adds Documents using the existing usage popup and explains pending cleanup', async () => {
  const documents = { documentUsage: vi.fn().mockResolvedValue(usage) };
  render(<UsageDialog documents={documents} open onClose={vi.fn()} />);
  const dialog = screen.getByRole('dialog', { name: 'Your usage' });
  expect(await within(dialog).findByRole('heading', { name: 'Documents' })).toBeVisible();
  expect(await within(dialog).findByText('37 / 100 documents · 210 MiB / 1 GiB')).toBeVisible();
  expect(within(dialog).getByText(/Storage is being released/)).toBeVisible();
});
it('defers document usage while closed, aborts on close and refreshes on reopen', async () => {
  const documents = { documentUsage: vi.fn().mockResolvedValue(usage) };
  const props = { documents, onClose: vi.fn() };
  const rendered = render(<UsageDialog {...props} open={false} />);
  expect(documents.documentUsage).not.toHaveBeenCalled();
  rendered.rerender(<UsageDialog {...props} open />);
  await screen.findByText('37 / 100 documents · 210 MiB / 1 GiB');
  const signal = documents.documentUsage.mock.calls[0]?.[0]?.signal;
  rendered.rerender(<UsageDialog {...props} open={false} />);
  expect(signal.aborted).toBe(true);
  rendered.rerender(<UsageDialog {...props} open />);
  await waitFor(() => expect(documents.documentUsage).toHaveBeenCalledTimes(2));
});
it('hides document numbers when not enforced and offers retry after usage failure', async () => {
  const documents = {
    documentUsage: vi
      .fn()
      .mockRejectedValueOnce(new Error('Unavailable'))
      .mockResolvedValue({ ...usage, enforcement: 'not_enforced' }),
  };
  render(<UsageDialog documents={documents} open onClose={vi.fn()} />);
  await screen.findByText('Document usage could not be loaded.');
  fireEvent.click(screen.getByRole('button', { name: 'Retry usage' }));
  await screen.findByText('Document quotas are not enforced.');
  expect(screen.getByText(/Storage is being released/)).toBeVisible();
  expect(screen.queryByText(/37 \/ 100|210 MiB/)).not.toBeInTheDocument();
});
