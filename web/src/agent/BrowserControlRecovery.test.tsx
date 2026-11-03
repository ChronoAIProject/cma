import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { BrowserControl } from './environment/BrowserControl';

const context = 'e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24';
const token = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';

function fixture() {
  const requests: { method: string; value: Record<string, unknown> }[] = [];
  let revision = 0;
  let owner: string | null = null;
  let dropped = false;
  let pause: (() => Promise<void>) | undefined;
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (path, options) => {
      const method = options?.method ?? 'GET';
      const value = options?.body
        ? (JSON.parse(String(options.body)) as Record<string, unknown>)
        : Object.fromEntries(new URL(String(path)).searchParams);
      requests.push({ method, value });
      if (method === 'POST') {
        revision += 1;
        owner = value.command === 'return' ? null : String(value.client_id);
        if (dropped) {
          dropped = false;
          throw new TypeError('lost response');
        }
      } else if (pause) await pause();
      return Response.json({
        agent_id: 'agent-a',
        runtime_incarnation: 'current',
        context_id: context,
        state: owner ? 'user' : 'agent',
        revision,
        client_id: owner,
        lease_token: owner && value.operation_id ? token : null,
        expires_at: owner ? new Date(Date.now() + 15000).toISOString() : null,
        lease_duration_ms: 15000,
        operation_id: value.operation_id ?? null,
        operation_revision: value.operation_id ? revision : null,
        audit: [],
        audit_truncated_before: null,
      });
    },
  });
  return {
    requests,
    client: sdk.agentBrowserControl,
    drop: () => {
      dropped = true;
    },
    hold: (value?: () => Promise<void>) => {
      pause = value;
    },
  };
}

function panel(client: ReturnType<typeof fixture>['client']) {
  return (
    <BrowserControl
      client={client}
      agentId="agent-a"
      incarnation="current"
      contextId={context}
      active
    >
      <img alt="Agent browser page" src="data:image/jpeg;base64,/9j/" />
    </BrowserControl>
  );
}

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it('drains keyboard input queued while a status read is in flight', async () => {
  const runtime = fixture();
  render(panel(runtime.client));
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await screen.findByText('You are controlling');
  let finish: (() => void) | undefined;
  runtime.hold(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent(window, new Event('online'));
  await waitFor(() => expect(finish).toBeDefined());
  fireEvent.keyDown(screen.getByRole('application'), { key: 'Q' });
  runtime.hold();
  await act(async () => finish?.());
  await waitFor(() =>
    expect(runtime.requests.at(-1)?.value.input).toEqual({ kind: 'text', text: 'Q' }),
  );
});

it('recovers the exact lost operation after reload without replaying text or adopting the old client lease', async () => {
  const runtime = fixture();
  const original = render(panel(runtime.client));
  runtime.drop();
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await screen.findByText('Control state unknown');
  const acquisition = runtime.requests.find((request) => request.method === 'POST')!.value;
  original.unmount();
  render(panel(runtime.client));
  await waitFor(() => {
    expect(runtime.requests.at(-1)?.method).toBe('GET');
    expect(runtime.requests.at(-1)?.value.operation_id).toBe(acquisition.operation_id);
  });
  expect(await screen.findByText('Another client is controlling')).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
  expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
  expect(JSON.stringify(sessionStorage)).not.toContain(token);
});

it('stops input offline and verifies current state on reconnect before accepting more input', async () => {
  const runtime = fixture();
  render(panel(runtime.client));
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await screen.findByText('You are controlling');
  const online = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);
  fireEvent(window, new Event('offline'));
  await screen.findByText('Control state unknown');
  fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
  expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
  let finish: (() => void) | undefined;
  runtime.hold(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  online.mockReturnValue(true);
  fireEvent(window, new Event('online'));
  await waitFor(() => expect(finish).toBeDefined());
  fireEvent.keyDown(screen.getByRole('application'), { key: 'Y' });
  expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
  runtime.hold();
  await act(async () => finish?.());
  await screen.findByText('You are controlling');
  fireEvent.keyDown(screen.getByRole('application'), { key: 'Z' });
  await waitFor(() =>
    expect(runtime.requests.at(-1)?.value.input).toEqual({ kind: 'text', text: 'Z' }),
  );
});

it('ignores an old account acquisition response after its panel is removed', async () => {
  let acknowledge: (() => void) | undefined;
  const runtime = fixture();
  const oldClient = {
    ...runtime.client,
    read: runtime.client.read.bind(runtime.client),
    change: async (...args: Parameters<typeof runtime.client.change>) => {
      await new Promise<void>((resolve) => {
        acknowledge = resolve;
      });
      return runtime.client.change(...args);
    },
  };
  const oldPanel = render(panel(oldClient));
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await waitFor(() => expect(acknowledge).toBeDefined());
  oldPanel.unmount();
  const another = fixture();
  render(panel(another.client));
  await screen.findByText('Agent is controlling');
  await act(async () => acknowledge?.());
  expect(screen.queryByText('You are controlling')).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
  expect(another.requests.every((request) => request.method === 'GET')).toBe(true);
});
