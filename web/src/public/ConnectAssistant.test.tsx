import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { PUBLIC_RECORD, publicClient } from '../test/public';
import { ConnectAssistant, secureSetupUrl } from './ConnectAssistant';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});
const service = {
  service: 'code',
  state: 'missing',
  display_name: 'Code host',
  connection_mode: 'hosted',
} as const;

describe('connection assistant', () => {
  it.each(['transport', 'idempotency_in_flight', 'idempotency_recovery_required'])(
    'preserves the connection body and key after uncertain creation %s',
    async (code) => {
      vi.spyOn(window, 'open').mockReturnValue(null);
      const client = publicClient();
      vi.mocked(client.requests.createConnection).mockRejectedValueOnce(
        code === 'transport' ? new Error('offline') : new ApiError(409, 'Unknown', code),
      );
      render(
        <ConnectAssistant
          client={client}
          requestId={PUBLIC_RECORD.id}
          service={service}
          onSatisfied={vi.fn()}
          onRecheck={vi.fn()}
        />,
      );
      fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
      await screen.findByRole('alert');
      await act(async () => undefined);
      fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
      await act(async () => undefined);
      const calls = vi.mocked(client.requests.createConnection).mock.calls;
      expect(calls).toHaveLength(2);
      expect(calls[1]).toEqual(calls[0]);
    },
  );
  it('recovers a same-tab handoff without storing the hosted URL or creating another command', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-09-06T00:00:00Z'));
    vi.spyOn(window, 'open').mockReturnValue(null);
    const client = publicClient();
    const satisfied = vi.fn();
    const props = {
      client,
      requestId: PUBLIC_RECORD.id,
      service,
      onSatisfied: satisfied,
      onRecheck: vi.fn(),
    };
    const view = render(<ConnectAssistant {...props} />);
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    expect(sessionStorage.getItem(`cma:connection-return:${PUBLIC_RECORD.id}:code`)).toBe(
      JSON.stringify({ handoff_id: 'handoff-one', expires_at: '2026-09-06T00:10:00Z' }),
    );
    view.unmount();
    render(<ConnectAssistant {...props} />);
    expect(screen.getByRole('button', { name: 'Verify' })).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(satisfied).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await act(async () => undefined);
    expect(satisfied).toHaveBeenCalledWith('code');
    expect(client.requests.createConnection).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem(`cma:connection-return:${PUBLIC_RECORD.id}:code`)).toBeNull();
  });

  it('aborts an in-flight status read at the five-minute deadline', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'open').mockReturnValue(null);
    const client = publicClient();
    vi.mocked(client.requests.readConnection).mockImplementation(
      () => new Promise(() => undefined),
    );
    render(
      <ConnectAssistant
        client={client}
        requestId={PUBLIC_RECORD.id}
        service={service}
        onSatisfied={vi.fn()}
        onRecheck={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300_000);
    });
    expect(vi.mocked(client.requests.readConnection).mock.calls[0]?.[1]?.signal?.aborted).toBe(
      true,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('We stopped checking after five minutes.');
  });
  it('opens the popup synchronously, polls, closes on satisfied and continues', async () => {
    vi.useFakeTimers();
    const client = publicClient();
    const popup = { opener: {}, location: { href: '' }, close: vi.fn(), closed: false };
    const open = vi.spyOn(window, 'open').mockImplementation(() => popup as unknown as Window);
    const satisfied = vi.fn();
    render(
      <ConnectAssistant
        client={client}
        requestId={PUBLIC_RECORD.id}
        service={service}
        onSatisfied={satisfied}
        onRecheck={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    expect(open).toHaveBeenCalledOnce();
    expect(open.mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(client.requests.createConnection).mock.invocationCallOrder[0]!,
    );
    await act(async () => undefined);
    expect(popup.opener).toBeNull();
    expect(popup.location.href).toBe('https://identity.example.test/connect');
    expect(client.requests.readConnection).not.toHaveBeenCalled();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(satisfied).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await act(async () => undefined);
    expect(satisfied).toHaveBeenCalledOnce();
    expect(satisfied).toHaveBeenCalledWith('code');
    expect(popup.close).toHaveBeenCalled();
  });

  it('offers same-tab setup and explicit recheck when the popup is blocked', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'open').mockReturnValue(null);
    const client = publicClient();
    render(
      <ConnectAssistant
        client={client}
        requestId={PUBLIC_RECORD.id}
        service={service}
        onSatisfied={vi.fn()}
        onRecheck={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    expect(screen.getByRole('link', { name: 'Continue in NyxID' })).toHaveAttribute(
      'href',
      'https://identity.example.test/connect',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(client.requests.readConnection).toHaveBeenCalledOnce();
  });

  it('offers another explicit attempt after expiry and never navigates to insecure URLs', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'open').mockReturnValue(null);
    const client = publicClient();
    vi.mocked(client.requests.readConnection).mockResolvedValue({
      handoff_id: 'handoff-one',
      launch_id: PUBLIC_RECORD.id,
      service_id: 'code',
      state: 'expired',
      expires_at: PUBLIC_RECORD.created_at,
      updated_at: PUBLIC_RECORD.updated_at,
    });
    render(
      <ConnectAssistant
        client={client}
        requestId={PUBLIC_RECORD.id}
        service={service}
        onSatisfied={vi.fn()}
        onRecheck={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(screen.getByText('That link expired. Try again.')).toBeInTheDocument();
    const first = vi.mocked(client.requests.createConnection).mock.calls[0];
    vi.mocked(client.requests.createConnection).mockResolvedValue({
      handoff_id: 'handoff-two',
      hosted_url: 'http://identity.example.test/connect',
      expires_at: PUBLIC_RECORD.created_at,
    });
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    expect(
      screen.getByText('NyxID did not provide a secure account setup page. Try again later.'),
    ).toBeInTheDocument();
    expect(vi.mocked(client.requests.createConnection).mock.calls[1]?.[1].idempotencyKey).not.toBe(
      first?.[1].idempotencyKey,
    );
  });

  it('uses manual metadata without claiming or requesting hosted setup', async () => {
    const client = publicClient();
    const recheck = vi.fn();
    render(
      <ConnectAssistant
        client={client}
        requestId={PUBLIC_RECORD.id}
        service={{
          ...service,
          connection_mode: 'manual',
          setup_url: 'https://identity.example.test/services',
        }}
        onSatisfied={vi.fn()}
        onRecheck={recheck}
      />,
    );
    expect(screen.getByRole('link', { name: 'Connect' })).toHaveAttribute(
      'href',
      'https://identity.example.test/services',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    expect(recheck).toHaveBeenCalledOnce();
    expect(client.requests.createConnection).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: 'Connect' })).not.toBeInTheDocument();
  });

  it('backs off failed polls and stops after five minutes', async () => {
    vi.useFakeTimers();
    vi.spyOn(window, 'open').mockReturnValue(null);
    const client = publicClient();
    vi.mocked(client.requests.readConnection).mockRejectedValue(new Error('offline'));
    render(
      <ConnectAssistant
        client={client}
        requestId={PUBLIC_RECORD.id}
        service={service}
        onSatisfied={vi.fn()}
        onRecheck={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300_000);
    });
    expect(
      screen.getByText(
        'We stopped checking after five minutes. Check again when you have connected the account.',
      ),
    ).toBeInTheDocument();
    const count = vi.mocked(client.requests.readConnection).mock.calls.length;
    expect(count).toBeLessThan(30);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(client.requests.readConnection).toHaveBeenCalledTimes(count);
  });
});

it.each([
  'http://example.test',
  'javascript:alert(1)',
  'https://user:pass@example.test',
  '/local',
  'not a URL',
])('rejects unsafe setup URL %s', (value) => {
  expect(secureSetupUrl(value)).toBeUndefined();
});
