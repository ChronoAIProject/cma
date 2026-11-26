import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { sandboxFixture } from '../test/hierarchy';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { harness, json } from './transport/controller.fixture';
import { agentTransportJournal } from './transport/journal';
import { deferred, status } from './transport/queuedReconciliation.fixture';

const text = 'Send after checking connections';
const report = { owner: 'alice', ready: true, checked_at: '2026-11-20T00:00:00Z', services: [] };
const flush = () => act(() => vi.advanceTimersByTimeAsync(0));
const send = () => fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
const rows = () =>
  screen.queryAllByRole('article', { name: 'You' }).filter((row) => within(row).queryByText(text));

beforeEach(() => {
  vi.useFakeTimers();
  sessionStorage.clear();
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  sessionStorage.clear();
});

async function setup(prior: 'accepted' | 'empty' | 'sent' = 'accepted') {
  const h = harness();
  const readSandbox = vi.spyOn(h.client, 'readSandbox').mockResolvedValue(sandboxFixture('paused'));
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  if (prior === 'accepted') {
    journal.select(true);
    const previous = journal.retain('Earlier accepted message');
    journal.accepted(previous.key, { responseId: 'resp_1', commandId: 'cmd_1' });
  }
  const original = h.fetch.getMockImplementation()!;
  h.fetch.mockImplementation((input, init) => {
    const match = /\/commands\/cmd_([12])$/.exec(String(input));
    return match
      ? Promise.resolve(json(status(Number(match[1]), 'completed')))
      : original(input, init);
  });
  const check = vi.fn<typeof fetch>(async (input) => {
    if (String(input) !== '/auth/connections/check') throw new Error(`Unexpected request ${input}`);
    return json(report);
  });
  vi.stubGlobal('fetch', check);
  function Agent({ viewer = 'alice', readOnly = false }: { viewer?: string; readOnly?: boolean }) {
    const controller = useAgentController(h.client, 'agt_1', { viewerId: viewer });
    return (
      <CmaClientProvider client={h.client}>
        <AgentSurface
          agentId="agt_1"
          viewerId={viewer}
          lifecycleClient={h.client}
          controller={controller}
          accessUncertain={readOnly}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
          onNavigate={() => {}}
        />
      </CmaClientProvider>
    );
  }
  const view = render(<Agent />);
  await flush();
  expect(h.streaming).toBe(true);
  const field = screen.getByRole('textbox', { name: 'Message' });
  if (prior === 'sent') {
    fireEvent.change(field, { target: { value: 'Earlier accepted message' } });
    send();
    await flush();
    expect(h.post).toHaveBeenCalledOnce();
    expect(journal.read()?.state).toBe('accepted');
    expect(field).toHaveValue('');
  }
  h.post.mockImplementation(
    async () =>
      new Response(new ReadableStream(), {
        headers: {
          'content-type': 'text/event-stream',
          'x-cma-response-id': 'resp_2',
          'x-cma-command-id': 'cmd_2',
        },
      }),
  );
  fireEvent.change(field, { target: { value: text } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  // Consume no prepared handoff: this attempt must wait for the real connection gate.
  fireEvent(window, new Event('blur'));
  const ready = deferred<Response>();
  check.mockReturnValueOnce(ready.promise);
  const checksBefore = check.mock.calls.length;
  send();
  await flush();
  expect(check).toHaveBeenCalledTimes(checksBefore + 1);
  expect(screen.getByText('Sending…')).toBeInTheDocument();
  expect(h.post).toHaveBeenCalledTimes(prior === 'sent' ? 1 : 0);
  const previous = journal.read();
  const refresh = async () => {
    fireEvent(document, new Event('visibilitychange'));
    await flush();
  };
  return { h, journal, previous, readSandbox, field, view, Agent, ready, refresh };
}

it.each(['accepted', 'empty', 'sent'] as const)(
  'marks a deferred send not sent with %s history and replaces it only on explicit retry',
  async (prior) => {
    const f = await setup(prior);
    const writesBefore = f.h.post.mock.calls.length;
    f.readSandbox.mockResolvedValue(sandboxFixture('failed'));
    await f.refresh();
    await act(async () => f.ready.resolve(json(report)));
    await flush();
    expect(f.h.post).toHaveBeenCalledTimes(writesBefore);
    expect(f.journal.read()).toEqual(f.previous);
    expect(f.h.inputReceipt).not.toHaveBeenCalled();
    expect(f.field).toHaveValue(text);
    expect(rows()).toHaveLength(1);
    expect(within(rows()[0]!).getByText('Not sent')).toBeInTheDocument();
    expect(screen.queryByText('Awaiting confirmation')).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Recover pending request' }),
    ).not.toBeInTheDocument();

    f.readSandbox.mockResolvedValue(sandboxFixture('paused'));
    await f.refresh();
    expect(f.h.post).toHaveBeenCalledTimes(writesBefore);
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
    send();
    await flush();
    expect(f.h.post).toHaveBeenCalledTimes(writesBefore + 1);
    expect(f.field).toHaveValue('');
    expect(rows()).toHaveLength(1);
    expect(within(rows()[0]!).getByText('Sent')).toBeInTheDocument();
    expect(f.journal.read()).toMatchObject({ state: 'accepted', input: text });
    expect(f.journal.read()?.key).not.toBe(f.previous?.key);
    await act(async () => {
      f.h.durable(1, { type: 'message', role: 'user', content: text }, undefined, 'cmd_2');
    });
    expect(rows()).toHaveLength(1);
    expect(within(rows()[0]!).queryByText('Sent')).not.toBeInTheDocument();
  },
);

it.each(['denied', 'restored'] as const)(
  'refuses the captured send after input is %s before the connection reply',
  async (change) => {
    const f = await setup();
    if (change === 'denied') f.readSandbox.mockRejectedValue(new ApiError(403, 'Access denied'));
    else f.readSandbox.mockResolvedValue(sandboxFixture('failed'));
    await f.refresh();
    if (change === 'restored') {
      f.readSandbox.mockResolvedValue(sandboxFixture('paused'));
      await f.refresh();
    }
    await act(async () => f.ready.resolve(json(report)));
    await flush();
    expect(f.h.post).not.toHaveBeenCalled();
    expect(f.journal.read()).toEqual(f.previous);
    expect(f.field).toHaveValue(text);
    expect(within(rows()[0]!).getByText('Not sent')).toBeInTheDocument();
    expect(screen.queryByText('Awaiting confirmation')).not.toBeInTheDocument();
  },
);

it.each(['readOnly', 'owner', 'unmount'] as const)(
  'ignores a late connection reply after %s without changing another view',
  async (change) => {
    const f = await setup();
    if (change === 'unmount') f.view.unmount();
    else
      f.view.rerender(
        <f.Agent viewer={change === 'owner' ? 'bob' : 'alice'} readOnly={change === 'readOnly'} />,
      );
    await flush();
    expect(rows()).toHaveLength(0);
    const currentView = document.body.textContent;
    await act(async () => f.ready.resolve(json(report)));
    await flush();
    expect(f.h.post).not.toHaveBeenCalled();
    expect(f.journal.read()).toEqual(f.previous);
    expect(document.body.textContent).toBe(currentView);
    expect(rows()).toHaveLength(0);
  },
);

it('keeps an actually dispatched uncertain request pending after input eligibility changes', async () => {
  const f = await setup();
  const response = deferred<Response>();
  f.h.post.mockReturnValueOnce(response.promise);
  await act(async () => f.ready.resolve(json(report)));
  await flush();
  expect(f.h.post).toHaveBeenCalledOnce();
  const pending = f.journal.read();
  expect(pending).toMatchObject({ state: 'pending', input: text });
  f.readSandbox.mockResolvedValue(sandboxFixture('failed'));
  await f.refresh();
  await act(async () => response.resolve(new Response('Unavailable', { status: 503 })));
  await flush();
  expect(f.journal.read()).toEqual(pending);
  expect(f.field).toHaveValue(text);
  expect(within(rows()[0]!).getByText('Awaiting confirmation')).toBeInTheDocument();
  expect(screen.queryByText('Not sent')).not.toBeInTheDocument();
  expect(f.h.post).toHaveBeenCalledOnce();
  expect(f.h.inputReceipt).toHaveBeenCalled();
  for (const [lookup] of f.h.inputReceipt.mock.calls) {
    expect(lookup?.body).toBe(f.h.post.mock.calls[0]![0]!.body);
    expect(new Headers(lookup?.headers).get('idempotency-key')).toBe(pending?.key);
  }
});
