import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ApiError, type ActivitySummary } from '@cma/client';
import { Activity } from './Activity';
import { operatorClient, now } from './testFixtures';
import { chooseOptionValue } from '../test/select';

type Page = { items: ActivitySummary[]; next: string | null };
const row = (id: string): ActivitySummary => ({
  id,
  at: now,
  actor: null,
  action: 'read',
  target: { kind: 'resource', id },
  outcome: { status: 'succeeded' },
});
function pending() {
  let resolve!: (page: Page) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Page>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

describe('Activity summary pages', () => {
  it('requests one bounded first page and retains rows while explicit refresh is pending', async () => {
    const client = operatorClient();
    const held = pending();
    client.admin.listActivitySummary
      .mockResolvedValueOnce({ items: [row('first')], next: null })
      .mockReturnValueOnce(held.promise);
    const user = userEvent.setup();
    render(<Activity client={client} />);
    await screen.findByText('first');
    expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(1);
    expect(client.admin.listActivitySummary).toHaveBeenCalledWith(
      expect.objectContaining({
        limit: 50,
        cursor: undefined,
        actor: undefined,
        action: undefined,
      }),
    );
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    expect(screen.getByText('first')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Refreshing records');
    expect(screen.getByRole('button', { name: 'Refresh records' })).toBeDisabled();
    await act(async () => held.resolve({ items: [row('fresh')], next: null }));
    expect(screen.queryByText('first')).not.toBeInTheDocument();
    expect(screen.getByText('fresh')).toBeInTheDocument();
    expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(2);
  });

  it('resets cursor on filters and rejects a late abort-resistant page and unmount result', async () => {
    const client = operatorClient();
    const held = pending();
    const final = pending();
    client.admin.listActivitySummary
      .mockResolvedValueOnce({ items: [row('first')], next: '123:act_next' })
      .mockReturnValueOnce(held.promise)
      .mockResolvedValueOnce({ items: [row('filtered')], next: null })
      .mockReturnValueOnce(final.promise);
    const user = userEvent.setup();
    const mounted = render(<Activity client={client} />);
    await screen.findByText('first');
    await user.click(screen.getByRole('button', { name: 'Next page' }));
    const oldSignal = client.admin.listActivitySummary.mock.calls[1]![0]!.signal!;
    await chooseOptionValue(user, screen.getByLabelText('Activity action'), 'delete');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await screen.findByText('filtered');
    expect(oldSignal.aborted).toBe(true);
    expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: undefined, action: 'delete' }),
    );
    await act(async () => held.resolve({ items: [row('stale')], next: '123:act_stale' }));
    expect(screen.queryByText('stale')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next page' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    const signal = client.admin.listActivitySummary.mock.calls[3]![0]!.signal!;
    mounted.unmount();
    expect(signal.aborted).toBe(true);
    await act(async () => final.resolve({ items: [row('unmounted')], next: null }));
    expect(screen.queryByText('unmounted')).not.toBeInTheDocument();
  });

  it('clears retained rows on permission refusal and retries only explicitly', async () => {
    const client = operatorClient();
    client.admin.listActivitySummary
      .mockResolvedValueOnce({ items: [row('protected')], next: null })
      .mockRejectedValueOnce(new ApiError(403, 'Denied', 'forbidden'))
      .mockResolvedValueOnce({ items: [], next: null });
    const user = userEvent.setup();
    render(<Activity client={client} />);
    await screen.findByText('protected');
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    await screen.findByText('This account does not have permission for this operation.');
    expect(screen.queryByText('protected')).not.toBeInTheDocument();
    expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(2);
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    await screen.findByText('No records match these filters.');
    await waitFor(() => expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(3));
  });
});
