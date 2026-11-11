import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, type AgentBrowserControlClient } from '@chronoai/cma-sdk';
import { BrowserControl } from './environment/BrowserControl';

const context = 'e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24';
const token = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';

function fixture() {
  const requests: { method: string; value: Record<string, unknown> }[] = [];
  let revision = 0;
  let owner: string | null = null;
  let dropped = false;
  let replacement: Record<string, unknown> = {};
  let responseStatus = 200;
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
      if (responseStatus !== 200)
        return Response.json(
          { code: 'forbidden', detail: 'Control permission revoked', status: responseStatus },
          { status: responseStatus },
        );
      return Response.json(
        {
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
          ...replacement,
        },
        { status: responseStatus },
      );
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
    replaceReceipt: (value: Record<string, unknown>, status = 200) => {
      replacement = value;
      responseStatus = status;
    },
  };
}

function panel(
  client: AgentBrowserControlClient,
  accountId = 'owner-a',
  admissionAllowed = true,
  active = true,
) {
  return (
    <BrowserControl
      client={client}
      accountId={accountId}
      agentId="agent-a"
      incarnation="current"
      contextId={context}
      active={active}
      admissionAllowed={admissionAllowed}
    >
      <img alt="Agent browser page" src="data:image/jpeg;base64,/9j/" />
    </BrowserControl>
  );
}

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it('stops new input on policy closure while keeping mounted control and explicit return', async () => {
  const runtime = fixture();
  const view = render(panel(runtime.client));
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await screen.findByText('You are controlling');
  view.rerender(panel(runtime.client, 'owner-a', false));
  expect(screen.getByText('You are controlling')).toBeInTheDocument();
  expect(screen.getByLabelText('Text to type in browser')).toBeDisabled();
  fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
  expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Return to Agent' }));
  await screen.findByText('Agent paused · Desktop is idle');
  expect(
    runtime.requests
      .filter((request) => request.method === 'POST')
      .map((request) => request.value.command),
  ).toEqual(['acquire', 'return']);
  expect(screen.getByRole('button', { name: 'Take control' })).toBeDisabled();
});

it.each([
  ['expired', { expires_at: '2026-01-01T00:00:00Z' }, 200, 'Control state unknown'],
  ['foreign client', { client_id: 'another-client' }, 200, 'Another client is controlling'],
  ['different Agent', { agent_id: 'agent-b' }, 200, 'Control state unknown'],
  ['old incarnation', { runtime_incarnation: 'old' }, 200, 'Control state unknown'],
  ['old context', { context_id: 'old-context' }, 200, 'Control state unknown'],
  ['revoked permission', {}, 403, 'Browser control is not permitted.'],
] as const)(
  'does not adopt %s control authority when the frame is stale',
  async (_, receipt, status, label) => {
    const runtime = fixture();
    const view = render(panel(runtime.client));
    fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
    await screen.findByText('You are controlling');
    view.rerender(panel(runtime.client, 'owner-a', true, false));
    runtime.replaceReceipt(receipt, status);
    fireEvent(window, new Event('online'));
    expect(await screen.findByText(label)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Return to Agent' })).not.toBeInTheDocument();
    expect(screen.getByRole('application')).toHaveAttribute('aria-disabled', 'true');
    fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
    expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
  },
);

it('keeps the acquisition action unavailable while its current-state read is settling', async () => {
  const runtime = fixture();
  render(panel(runtime.client));
  const acquire = await screen.findByRole('button', { name: 'Take control' });
  let finish: (() => void) | undefined;
  runtime.hold(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  fireEvent(window, new Event('online'));
  await waitFor(() => expect(finish).toBeDefined());
  expect(acquire).toBeDisabled();
  runtime.hold();
  await act(async () => finish?.());
  expect(acquire).toBeEnabled();
  fireEvent.click(acquire);
  await screen.findByText('You are controlling');
  expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
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

it.each([true, false])(
  'recovers the lost operation without replaying text or adopting the old lease when admission is %s',
  async (enabled) => {
    const runtime = fixture();
    const original = render(panel(runtime.client));
    runtime.drop();
    fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
    await screen.findByText('Control state unknown');
    const acquisition = runtime.requests.find((request) => request.method === 'POST')!.value;
    original.unmount();
    render(panel(runtime.client, 'owner-a', enabled));
    await waitFor(() => {
      expect(runtime.requests.at(-1)?.method).toBe('GET');
      expect(runtime.requests.at(-1)?.value.operation_id).toBe(acquisition.operation_id);
    });
    expect(await screen.findByText('Another client is controlling')).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
    expect(runtime.requests.filter((request) => request.method === 'POST')).toHaveLength(1);
    expect(JSON.stringify(sessionStorage)).not.toContain(token);
  },
);

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
  render(panel(another.client, 'owner-b'));
  await screen.findByText('Agent paused · Desktop is idle');
  await act(async () => acknowledge?.());
  expect(screen.queryByText('You are controlling')).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('application'), { key: 'X' });
  expect(another.requests.every((request) => request.method === 'GET')).toBe(true);
});

it("does not query another account's stored uncertain operation after switching accounts", async () => {
  const original = fixture();
  const oldPanel = render(panel(original.client));
  original.drop();
  fireEvent.click(await screen.findByRole('button', { name: 'Take control' }));
  await screen.findByText('Control state unknown');
  oldPanel.unmount();
  const next = fixture();
  render(panel(next.client, 'owner-b'));
  await screen.findByText('Agent paused · Desktop is idle');
  expect(next.requests.at(-1)?.value.operation_id).toBeUndefined();
  expect(next.requests.every((request) => request.method === 'GET')).toBe(true);
});
