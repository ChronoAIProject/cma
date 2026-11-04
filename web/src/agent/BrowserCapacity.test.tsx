import { StrictMode } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { BrowserControl } from './environment/BrowserControl';
import { BrowserPanel } from './environment/BrowserPanel';
import { createRestWebClient } from '../api/rest';

it('shows uncertainty after four seconds and recovers the original stalled control change', async () => {
  vi.useFakeTimers();
  try {
    const requests: { method: string; operation: string | null }[] = [];
    const sdk = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async (url, options) => {
        const method = options?.method ?? 'GET';
        const operation =
          method === 'POST'
            ? (JSON.parse(String(options?.body)).operation_id as string)
            : new URL(String(url)).searchParams.get('operation_id');
        requests.push({ method, operation });
        if (method === 'POST') return new Promise<Response>(() => {});
        return Response.json({
          agent_id: 'agent-stalled',
          runtime_incarnation: 'current',
          context_id: 'context-stalled',
          state: 'agent',
          revision: operation ? 1 : 0,
          client_id: null,
          lease_token: null,
          expires_at: null,
          lease_duration_ms: 15000,
          operation_id: operation,
          operation_revision: operation ? 1 : null,
          audit: [],
          audit_truncated_before: null,
        });
      },
    });
    const view = render(
      <BrowserControl
        client={createRestWebClient(sdk).browserControl!}
        accountId="owner-stalled"
        agentId="agent-stalled"
        incarnation="current"
        contextId="context-stalled"
        active
        admissionAllowed
      >
        <span>Browser</span>
      </BrowserControl>,
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    fireEvent.click(screen.getByRole('button', { name: 'Take control' }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4000);
    });
    expect(screen.getByText('Control state unknown')).toBeVisible();
    const key =
      'cma-browser-control:' +
      JSON.stringify(['owner-stalled', 'agent-stalled', 'current', 'context-stalled']);
    expect(JSON.parse(sessionStorage.getItem(key)!).operation_id).toBe(requests[1]!.operation);
    fireEvent.click(screen.getByRole('button', { name: 'Check original operation' }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText('Agent is controlling')).toBeVisible();
    expect(requests.map(({ method }) => method)).toEqual(['GET', 'POST', 'GET']);
    expect(requests[2]!.operation).toBe(requests[1]!.operation);
    expect(sessionStorage.getItem(key)).toBeNull();
    view.unmount();
  } finally {
    vi.useRealTimers();
  }
});

it('verifies control once on a development remount while browser observation occupies the other permit', async () => {
  let finish: () => void = () => {};
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let active = 1;
  const fetch = vi.fn(async () => {
    active += 1;
    try {
      if (active > 2)
        return Response.json({ title: 'File capacity reached', status: 429 }, { status: 429 });
      await pending;
      return Response.json({
        agent_id: 'agent-a',
        runtime_incarnation: 'current',
        context_id: 'context-a',
        state: 'agent',
        revision: 0,
        client_id: null,
        lease_token: null,
        expires_at: null,
        lease_duration_ms: 15000,
        operation_id: null,
        operation_revision: null,
        audit: [],
        audit_truncated_before: null,
      });
    } finally {
      active -= 1;
    }
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  render(
    <StrictMode>
      <BrowserControl
        client={sdk.agentBrowserControl}
        accountId="owner-a"
        agentId="agent-a"
        incarnation="current"
        contextId="context-a"
        active
        admissionAllowed
      >
        <span>Actual browser observation uses the other public IO permit.</span>
      </BrowserControl>
    </StrictMode>,
  );
  await act(async () => finish());
  expect(await screen.findByText('Agent is controlling')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Take control' })).toBeEnabled();
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('withdraws a closed browser immediately and releases after its in-flight observation settles', async () => {
  let finish: () => void = () => {};
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let active = 1;
  const statuses: number[] = [];
  const methods: string[] = [];
  const fetch = vi.fn(async (_input: RequestInfo | URL, options?: RequestInit) => {
    const method = options?.method ?? 'GET';
    methods.push(method);
    active += 1;
    try {
      if (active > 2) {
        statuses.push(429);
        return Response.json({ title: 'File capacity reached', status: 429 }, { status: 429 });
      }
      if (method === 'GET') await pending;
      statuses.push(200);
      return Response.json({
        agent_id: 'agent-a',
        runtime_incarnation: 'current',
        context_id: 'context-a',
        state: 'ready',
        frame: null,
        observers: method === 'GET' ? 1 : 0,
        captures: 0,
        bytes: 0,
        min_interval_ms: 1000,
        fresh_for_ms: 3000,
        max_frame_bytes: 262144,
        max_observers: 16,
      });
    } finally {
      active -= 1;
    }
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const view = (active: boolean) => (
    <BrowserPanel
      client={sdk.agentBrowser}
      agentId="agent-a"
      incarnation="current"
      active={active}
      onReload={() => {}}
    />
  );
  const { rerender } = render(view(true));
  await act(async () => {});
  expect(methods).toEqual(['GET']);
  rerender(view(false));
  await act(async () => {});
  expect(screen.queryByAltText('Agent browser page')).not.toBeInTheDocument();
  expect(methods).toEqual(['GET']);
  await act(async () => finish());
  expect(methods).toEqual(['GET', 'DELETE']);
  expect(statuses).toEqual([200, 200]);
});
