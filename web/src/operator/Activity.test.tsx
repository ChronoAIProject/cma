import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  it('shows authenticated channel and request ID and reapplies filters from the first page', async () => {
    const client = operatorClient();
    client.admin.listActivitySummary.mockResolvedValue({
      items: [
        {
          ...row('agent-target'),
          actor: 'verified-user',
          channel: 'api',
          method: 'POST',
          route: '/api/v1/agents/{id}/input',
          request_id: '00000000-0000-4000-8000-000000000022',
          http_status: 202,
        },
      ],
      next: null,
    });
    const user = userEvent.setup();
    render(<Activity client={client} />);
    expect(await screen.findByText('API · client unknown')).toBeVisible();
    expect(screen.getByText('00000000-0000-4000-8000-000000000022')).toBeVisible();
    expect(screen.getByText('POST /api/v1/agents/{id}/input')).toBeVisible();
    await chooseOptionValue(user, screen.getByLabelText('Activity channel'), 'api');
    await chooseOptionValue(user, screen.getByLabelText('Activity result'), 'succeeded');
    fireEvent.change(screen.getByLabelText('Recorded from (SGT)'), {
      target: { value: '2026-11-28T16:00' },
    });
    fireEvent.change(screen.getByLabelText('Recorded before (SGT)'), {
      target: { value: '2026-11-28T17:00' },
    });
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await waitFor(() =>
      expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
        expect.objectContaining({
          cursor: undefined,
          channel: 'api',
          result: 'succeeded',
          recordedFrom: '2026-11-28T16:00:00+08:00',
          recordedBefore: '2026-11-28T17:00:00+08:00',
        }),
      ),
    );
  });

  it('announces the cursor position and actual record range through navigation and refresh', async () => {
    const client = operatorClient();
    client.admin.listActivitySummary.mockImplementation(async (options) => {
      if (options?.action === 'delete') return { items: [row('filtered')], next: null };
      if (options?.cursor === 'middle')
        return { items: [row('third'), row('fourth'), row('fifth')], next: 'terminal' };
      if (options?.cursor === 'terminal') return { items: [row('sixth')], next: null };
      return { items: [row('first'), row('second')], next: 'middle' };
    });
    const user = userEvent.setup();
    render(<Activity client={client} />);
    const previous = screen.getByRole('button', { name: 'Previous page' });
    const next = screen.getByRole('button', { name: 'Next page' });
    const position = async (text: string) =>
      waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(text));

    await position('Page 1 · Records 1–2 · More records available');
    expect(previous).toBeDisabled();
    await user.click(next);
    await position('Page 2 · Records 3–5 · More records available');
    expect(previous).toBeEnabled();
    await user.click(next);
    await position('Page 3 · Records 6–6 · Last page');
    expect(next).toBeDisabled();

    await user.click(previous);
    await position('Page 2 · Records 3–5 · More records available');
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    await position('Page 2 · Records 3–5 · More records available');
    expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: 'middle' }),
    );

    await chooseOptionValue(user, screen.getByLabelText('Activity action'), 'delete');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await position('Page 1 · Records 1–1 · Last page');
    expect(screen.getByRole('button', { name: 'Previous page' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Next page' })).toBeDisabled();
  });

  it('finds an older actor on the server before paging and keeps the selection on refresh', async () => {
    const client = operatorClient();
    client.admin.searchActivityActors.mockResolvedValue([
      { subject: 'historical-actor', display_name: 'Historical actor' },
    ]);
    client.admin.listActivitySummary
      .mockResolvedValueOnce({ items: [row('newest')], next: '123:act_next' })
      .mockResolvedValue({
        items: [
          {
            ...row('historical record'),
            actor: 'historical-actor',
            actor_name: 'Historical actor',
          },
        ],
        next: null,
      });
    const user = userEvent.setup();
    render(<Activity client={client} canSearchActors />);
    await screen.findByText('newest');
    await user.type(screen.getByRole('searchbox', { name: 'Find actor by name or ID' }), 'history');
    await waitFor(() =>
      expect(client.admin.searchActivityActors).toHaveBeenCalledWith('history', expect.anything()),
    );
    await chooseOptionValue(user, screen.getByLabelText('Activity actor'), 'historical-actor');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await screen.findByText('historical record');
    expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(2);
    expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: undefined, actor: 'historical-actor' }),
    );
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    await waitFor(() => expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(3));
    expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: undefined, actor: 'historical-actor' }),
    );
  });

  it('looks up a known actor ID with Activity permission alone', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<Activity client={client} />);
    await user.type(screen.getByRole('searchbox', { name: 'Find actor by ID' }), 'known-id');
    await chooseOptionValue(user, screen.getByLabelText('Activity actor'), 'known-id');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await waitFor(() =>
      expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: undefined, actor: 'known-id' }),
      ),
    );
    expect(client.admin.searchActivityActors).not.toHaveBeenCalled();
  });

  it('shows seconds and preserves the API order for tied timestamps on refresh', async () => {
    const client = operatorClient();
    const latest = '2026-11-24T02:18:29.123Z';
    const earlier = '2026-11-24T02:18:03.456Z';
    client.admin.listActivitySummary.mockResolvedValue({
      items: [
        { ...row('second-tied'), at: latest },
        { ...row('first-tied'), at: latest },
        { ...row('earlier'), at: earlier },
      ],
      next: null,
    });
    const user = userEvent.setup();
    const { container } = render(<Activity client={client} />);
    await screen.findByText('earlier');
    const times = [...container.querySelectorAll<HTMLTimeElement>('tbody time')];
    expect(times.map((time) => time.dateTime)).toEqual([latest, latest, earlier]);
    expect(times.map((time) => time.title)).toEqual([latest, latest, earlier]);
    expect(times[0]).toHaveTextContent('10:18:29');
    expect(times[2]).toHaveTextContent('10:18:03');
    expect(screen.getByText('Newest recorded first.')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    await waitFor(() => expect(screen.getByRole('table')).not.toHaveAttribute('aria-busy'));
    expect(
      Array.from(container.querySelectorAll('tbody code'), (cell) => cell.textContent),
    ).toEqual(['second-tied', 'first-tied', 'earlier']);
  });

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
        excludeSuccessfulReads: true,
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

  it('offers successful reads through the explicit Read filter', async () => {
    const client = operatorClient();
    client.admin.listActivitySummary
      .mockResolvedValueOnce({ items: [], next: null })
      .mockResolvedValueOnce({ items: [row('routine read')], next: null });
    const user = userEvent.setup();
    render(<Activity client={client} />);
    await screen.findByText('No records match these filters.');
    expect(screen.getByLabelText('Activity action')).toHaveTextContent('Audit events');
    await chooseOptionValue(user, screen.getByLabelText('Activity action'), 'read');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await screen.findByText('routine read');
    expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
      expect.objectContaining({
        cursor: undefined,
        action: 'read',
        excludeSuccessfulReads: undefined,
      }),
    );
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
    expect(screen.getByRole('status')).toHaveTextContent('Page 1 · 0 records · Last page');
    await waitFor(() => expect(client.admin.listActivitySummary).toHaveBeenCalledTimes(3));
  });
});
