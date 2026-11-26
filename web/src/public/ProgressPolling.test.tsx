import { act, render, renderHook, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Launch, LaunchBlockedReason } from '@cma/client';
import { PUBLIC_RECORD, publicClient } from '../test/public';
import { Progress } from './Progress';
import { useRecordedProgress } from './useRecordedProgress';

afterEach(() => vi.useRealTimers());

describe('recorded progress polling', () => {
  it.each([undefined, { type: 'sandbox_paused_reviving' } as const])(
    'follows healthy preparation for thirty minutes and opens the ready Agent with blocker %j',
    async (blockedReason) => {
      vi.useFakeTimers();
      const client = publicClient();
      const pending: Launch = {
        ...PUBLIC_RECORD,
        state: 'sandbox_pending',
        blocked_reason: blockedReason,
      };
      vi.mocked(client.requests.read).mockResolvedValue(pending);
      vi.mocked(client.requests.stream).mockRejectedValue(new Error('stream unavailable'));
      const navigate = vi.fn();
      const { unmount } = render(
        <Progress client={client} id={pending.id} onNavigate={navigate} />,
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30 * 60_000);
      });
      expect(client.requests.read).toHaveBeenCalledTimes(601);
      expect(screen.getByText('Preparing software')).toBeInTheDocument();
      expect(screen.queryByText(/Updates are unavailable/)).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Check again' })).not.toBeInTheDocument();
      expect(navigate).not.toHaveBeenCalled();

      vi.mocked(client.requests.read).mockResolvedValue({
        ...pending,
        state: 'ready',
        blocked_reason: undefined,
        event_head: pending.event_head + 1,
        resolution: { ...pending.resolution, agent: { state: 'reused', id: 'agt_restored' } },
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3000);
      });
      expect(navigate).toHaveBeenCalledExactlyOnceWith(
        { kind: 'agent', agentId: 'agt_restored' },
        { replace: true },
      );
      const reads = vi.mocked(client.requests.read).mock.calls.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(60_000);
      });
      expect(client.requests.read).toHaveBeenCalledTimes(reads);
      expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
      expect(client.requests.start).not.toHaveBeenCalled();
      expect(client.requests.cancel).not.toHaveBeenCalled();
      unmount();
    },
  );

  it('backs off failed reads, resets after recovery, and bounds only uninterrupted outages', async () => {
    vi.useFakeTimers();
    const client = publicClient();
    vi.mocked(client.requests.stream).mockRejectedValue(new Error('stream unavailable'));
    const { result, unmount } = renderHook(() => useRecordedProgress(client, PUBLIC_RECORD.id));
    await act(async () => undefined);
    vi.mocked(client.requests.read).mockRejectedValue(new Error('offline'));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(2);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5999);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(2);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(3);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(11_999);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(3);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(4);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(219_000);
    });
    expect(result.current.record).toBe(PUBLIC_RECORD);
    expect(result.current.error).not.toMatch(/Automatic updates have paused/);

    vi.mocked(client.requests.read).mockResolvedValueOnce(PUBLIC_RECORD);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6000);
    });
    expect(result.current.error).toBeUndefined();
    const reads = vi.mocked(client.requests.read).mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2999);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(reads);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(reads + 1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(240_000);
    });
    expect(result.current.error).not.toMatch(/Automatic updates have paused/);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(result.current.error).toMatch(/Automatic updates have paused/);
    const stoppedReads = vi.mocked(client.requests.read).mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(stoppedReads);
    expect(result.current.record).toBe(PUBLIC_RECORD);
    unmount();
  });

  it.each(['draft', 'ready', 'completed', 'failed_terminal', 'cancelled'] as const)(
    'stops after observing %s',
    async (state) => {
      vi.useFakeTimers();
      const client = publicClient();
      vi.mocked(client.requests.stream).mockRejectedValue(new Error('stream unavailable'));
      const { result, unmount } = renderHook(() => useRecordedProgress(client, PUBLIC_RECORD.id));
      await act(async () => undefined);
      vi.mocked(client.requests.read).mockResolvedValue({ ...PUBLIC_RECORD, state });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(60_000);
      });
      expect(result.current.record?.state).toBe(state);
      expect(client.requests.read).toHaveBeenCalledTimes(2);
      unmount();
    },
  );

  it('aborts an outstanding read on unmount without scheduling another poll', async () => {
    vi.useFakeTimers();
    const client = publicClient();
    vi.mocked(client.requests.stream).mockRejectedValue(new Error('stream unavailable'));
    const { unmount } = renderHook(() => useRecordedProgress(client, PUBLIC_RECORD.id));
    await act(async () => undefined);
    let finish: ((record: Launch) => void) | undefined;
    vi.mocked(client.requests.read).mockImplementation(
      () => new Promise((resolve) => (finish = resolve)),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(2);
    const signal = vi.mocked(client.requests.read).mock.calls[1]?.[1]?.signal;
    expect(signal?.aborted).toBe(false);
    unmount();
    expect(signal?.aborted).toBe(true);
    await act(async () => {
      finish?.(PUBLIC_RECORD);
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(client.requests.read).toHaveBeenCalledTimes(2);
  });
});

describe('blocked preparation state', () => {
  it.each([
    [{ type: 'sandbox_paused_reviving' }, 'In progress', 'ongoing'],
    [{ type: 'orchestration_paused' }, 'Paused by CMA', 'neutral'],
    [{ type: 'recovery_required' }, 'Needs you', 'warning'],
    [{ type: 'connection_required', service: 'code' }, 'Needs you', 'warning'],
  ] satisfies [LaunchBlockedReason, string, string][])(
    'shows %j as %s without requesting visitor action for automatic revival',
    async (blockedReason, label, dotState) => {
      const client = publicClient();
      vi.mocked(client.requests.read).mockResolvedValue({
        ...PUBLIC_RECORD,
        state: 'sandbox_pending',
        blocked_reason: blockedReason,
        steps: [{ key: 'sandbox', status: 'blocked', safe_detail: '' }],
      });
      const { unmount } = render(
        <Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />,
      );
      const row = (await screen.findByRole('heading', { name: 'Sandbox ready' })).closest('li');
      expect(row).not.toBeNull();
      expect(within(row!).getByText(label)).toBeInTheDocument();
      // Rows draw the in-app state dot, never the brand character.
      expect(row?.querySelector('.landing__character')).toBeNull();
      expect(row?.querySelector('.statedot')).toHaveAttribute('data-state', dotState);
      unmount();
    },
  );
});

describe('overall state and failure', () => {
  it.each([
    [{ state: 'sandbox_pending' }, 'In progress', 'active'],
    [{ state: 'draft' }, 'Needs you', 'warning'],
    [{ state: 'cancelled' }, 'Cancelled', 'muted'],
    [{ state: 'failed_terminal' }, "Didn't work", 'error'],
    [
      { state: 'sandbox_pending', blocked_reason: { type: 'orchestration_paused' } },
      'Paused by CMA',
      'muted',
    ],
  ] satisfies [Partial<Launch>, string, string][])(
    'announces %j as a %s badge',
    async (change, word, tone) => {
      const client = publicClient();
      vi.mocked(client.requests.read).mockResolvedValue({ ...PUBLIC_RECORD, ...change });
      const { unmount } = render(
        <Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />,
      );
      await screen.findByRole('heading', { name: 'Sandbox ready' });
      const badge = screen
        .getAllByRole('status')
        .find((element) => element.classList.contains('badge'));
      expect(badge).toHaveTextContent(word);
      expect(badge).toHaveAttribute('data-tone', tone);
      unmount();
    },
  );

  it('puts every recovery action on the failure alert, in visitor words', async () => {
    const client = publicClient();
    vi.mocked(client.requests.read).mockResolvedValue({
      ...PUBLIC_RECORD,
      state: 'failed_terminal',
      failure: {
        code: 'launch_failed',
        safe_detail: 'internal detail',
        retryable: false,
        occurred_at: PUBLIC_RECORD.updated_at,
      },
    });
    render(<Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveAttribute('data-tone', 'error');
    for (const name of ['Check again', 'Start over', 'Copy support code'])
      expect(within(alert).getByRole('button', { name })).toBeInTheDocument();
    expect(alert).not.toHaveTextContent('internal detail');
    expect(alert).not.toHaveTextContent(/launch/i);
  });

  it('reserves the timeline while the first read is pending', () => {
    const client = publicClient();
    vi.mocked(client.requests.read).mockReturnValue(new Promise(() => undefined));
    const { container } = render(
      <Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />,
    );
    expect(screen.getByRole('status', { name: 'Loading' })).toBeInTheDocument();
    expect(container.querySelector('.landing__character')).toBeNull();
  });
});
