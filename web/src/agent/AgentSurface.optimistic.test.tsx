import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { accepted, binding, harness, json, queuedPage } from './transport/controller.fixture';
import { agentClient } from '../test/agent';
import { status } from './transport/queuedReconciliation.fixture';

const connected: ConnectionReport = {
  owner: 'alice',
  ready: true,
  checked_at: '2026-10-15T00:00:00Z',
  services: [],
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue(connected);
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

async function setup() {
  const h = harness();
  function Agent({ viewer = 'alice', agent = 'agt_1' }: { viewer?: string; agent?: string }) {
    const controller = useAgentController(h.client, agent, { viewerId: viewer });
    return (
      <CmaClientProvider client={h.client}>
        <AgentSurface
          agentId={agent}
          viewerId={viewer}
          lifecycleClient={h.client}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
          onNavigate={() => {}}
        />
      </CmaClientProvider>
    );
  }
  const view = render(<Agent />);
  await waitFor(() => expect(h.streaming).toBe(true));
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Immediate local message' } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
  return { h, view, field, Agent };
}
const click = () => fireEvent.click(screen.getByRole('button', { name: /^(Send|Queue) message$/ }));
const bubbles = (text: string) =>
  screen
    .getAllByRole('article', { name: 'You' })
    .filter((article) => within(article).queryByText(text) !== null);

it('does not flash a queue card when discovery finishes before the idle Send receipt', async () => {
  const { h } = await setup();
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  h.listQueued.mockImplementation(async () => json(queuedPage('Immediate local message')));
  const reads = h.listQueued.mock.calls.length;
  fireEvent(window, new Event('focus'));
  await waitFor(() => expect(h.listQueued.mock.calls.length).toBeGreaterThan(reads), {
    timeout: 3000,
  });
  expect(document.querySelector('.queued-messages')).toBeNull();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  await act(async () => h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding));
  expect(document.querySelector('.queued-messages')).toBeNull();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  await act(async () => response.resolve(accepted()));
  await screen.findByText('Sent');
  expect(document.querySelector('.queued-messages')).toBeNull();
  expect(bubbles('Immediate local message')).toHaveLength(1);
});

it('keeps idle input in one ordinary bubble through queued discovery and its own turn startup', async () => {
  const { h, field } = await setup();
  h.listQueued.mockImplementation(async () => json(queuedPage('Immediate local message')));
  const reads = h.listQueued.mock.calls.length;
  click();
  await waitFor(() => expect(field).toHaveValue(''));
  await waitFor(() => expect(h.listQueued.mock.calls.length).toBeGreaterThan(reads));
  expect(screen.queryByRole('region', { name: 'Queued messages' })).not.toBeInTheDocument();
  expect(document.querySelector('.queued-messages')).toBeNull();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(screen.getByText('Sent')).toBeInTheDocument();

  await act(async () => h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding));
  expect(document.querySelector('.queued-messages')).toBeNull();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  await act(async () => {
    h.durable(2, { type: 'message', role: 'user', content: 'Immediate local message' });
    h.durable(3, { type: 'turn_completed', turn_sequence: 1 });
  });
  const nextReads = h.listQueued.mock.calls.length;
  await waitFor(() => expect(h.listQueued.mock.calls.length).toBeGreaterThan(nextReads));
  expect(document.querySelector('.queued-messages')).toBeNull();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(h.post).toHaveBeenCalledOnce();
});

it('skips the per-message catalog wait, retains identity and acknowledgement gates, and merges exactly once', async () => {
  const { h, field } = await setup();
  const connections = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(connections.promise);
  const initialChecks = vi.mocked(connectionClient.check).mock.calls.length;
  const identity = deferred<string>();
  h.currentUser.mockReturnValueOnce(identity.promise);
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(screen.getByText('Sending…')).toBeInTheDocument();
  expect(h.post).not.toHaveBeenCalled();
  expect(field).toHaveValue('');
  expect(connectionClient.check).toHaveBeenCalledTimes(initialChecks);
  click();
  expect(h.post).not.toHaveBeenCalled();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  await act(async () => identity.resolve('alice'));
  expect(h.post).toHaveBeenCalledOnce();
  expect(screen.getByText('Sending…')).toBeInTheDocument();
  expect(screen.queryByLabelText('Pending request')).not.toBeInTheDocument();
  expect(screen.queryByText('We can’t confirm this message arrived')).not.toBeInTheDocument();
  expect(screen.queryByText('Sent')).not.toBeInTheDocument();
  await act(async () => response.resolve(accepted()));
  await waitFor(() => expect(field).toHaveValue(''));
  expect(screen.getByText('Sent')).toBeInTheDocument();
  expect(screen.queryByLabelText('Pending request')).not.toBeInTheDocument();
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.durable(2, { type: 'message', role: 'user', content: 'Immediate local message' });
  });
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(screen.queryByText('Sent')).not.toBeInTheDocument();
  expect(h.post).toHaveBeenCalledOnce();
  expect(connectionClient.check).toHaveBeenCalledTimes(initialChecks);
});

it('retains exact command ownership through completion before acknowledgement and never matches old identical text', async () => {
  const { h, field } = await setup();
  await act(async () => {
    h.durable(
      1,
      { type: 'turn_started', turn_sequence: 1 },
      { ...binding, command_id: 'old_command', response_id: 'old_response' },
    );
    h.durable(2, { type: 'message', role: 'user', content: 'Immediate local message' });
    h.durable(3, { type: 'turn_completed', turn_sequence: 1 });
  });
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  expect(bubbles('Immediate local message')).toHaveLength(2);
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  await act(async () => {
    h.durable(4, { type: 'turn_started', turn_sequence: 4 }, { ...binding, turn_sequence: 4 });
    h.durable(5, { type: 'message', role: 'user', content: 'Immediate local message' });
    h.durable(6, { type: 'turn_completed', turn_sequence: 4 });
    response.resolve(accepted());
  });
  await waitFor(() => expect(field).toHaveValue(''));
  expect(bubbles('Immediate local message')).toHaveLength(2);
  expect(screen.queryByText('Sending…')).not.toBeInTheDocument();
});

it('preserves the draft and original request through a lost acknowledgement, reload and explicit recovery', async () => {
  const { h, field, view, Agent } = await setup();
  h.post.mockRejectedValueOnce(new TypeError('Network unavailable'));
  click();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  await waitFor(() => expect(screen.getByText('Awaiting confirmation')).toBeInTheDocument());
  expect(within(screen.getByLabelText('Pending request')).getByRole('alert')).toBeInTheDocument();
  expect(await screen.findByText('We can’t confirm this message arrived')).toBeInTheDocument();
  expect(field).toHaveValue('Immediate local message');
  expect(h.post).toHaveBeenCalledOnce();
  const original = h.post.mock.calls[0]![0]!;
  const reads = h.inputReceipt.mock.calls.length;
  view.unmount();
  render(<Agent />);
  await screen.findByText('Awaiting confirmation');
  expect(bubbles('Immediate local message')).toHaveLength(1);
  const recover = await screen.findByRole('button', { name: 'Recover pending request' });
  h.inputReceipt.mockImplementationOnce(async () => json(status(1, 'completed')));
  fireEvent.click(recover);
  await waitFor(() => expect(screen.getByText('Sent')).toBeInTheDocument());
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.inputReceipt.mock.calls.length).toBeGreaterThan(reads);
  for (const [lookup] of h.inputReceipt.mock.calls) {
    expect(lookup?.body).toBe(original.body);
    expect(new Headers(lookup?.headers).get('idempotency-key')).toBe(
      new Headers(original.headers).get('idempotency-key'),
    );
  }
  expect(bubbles('Immediate local message')).toHaveLength(1);
});

it('restores a committed artifact request after reopen without submitting it again', async () => {
  const { h, view, Agent } = await setup();
  const bytes = new TextEncoder().encode('Verified artifact revision');
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  const sha256 = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
  const artifact = {
    id: `art_${'a'.repeat(64)}`,
    agent_id: 'agt_1',
    name: 'report.txt',
    media_type: 'text/plain',
    sha256,
    size_bytes: bytes.length,
    created_at: '2026-09-16T00:00:00Z',
  };
  let committed: RequestInit | undefined;
  let exposeReceipt = false;
  const revisions: Uint8Array[] = [];
  h.post.mockImplementationOnce(async (init) => {
    committed = init;
    revisions.push(bytes);
    throw new TypeError('Connection dropped after durable admission');
  });
  const receipt = vi.fn(async (init?: RequestInit) => {
    if (!exposeReceipt || !committed) return json(null);
    expect(init?.body).toBe(committed.body);
    expect(new Headers(init?.headers).get('idempotency-key')).toBe(
      new Headers(committed.headers).get('idempotency-key'),
    );
    return json(status(1, 'completed'));
  });
  const request = h.fetch.getMockImplementation()!;
  const download = `/api/v1/agents/agt_1/artifacts/${artifact.id}`;
  h.fetch.mockImplementation(async (input, init) => {
    if (String(input) === '/api/v2/agents/agt_1/responses/receipt') return receipt(init);
    if (String(input) === download && revisions.length === 1) return json(artifact);
    if (String(input) === `${download}/download` && revisions.length === 1)
      return new Response(revisions[0]!.slice().buffer, {
        headers: { 'content-type': 'application/octet-stream' },
      });
    return request(input, init);
  });

  click();
  await screen.findByText('Awaiting confirmation');
  expect(h.post).toHaveBeenCalledOnce();
  view.unmount();
  exposeReceipt = true;
  render(<Agent />);
  await waitFor(() => expect(receipt).toHaveBeenCalled());
  await waitFor(() => expect(screen.queryByText('Awaiting confirmation')).not.toBeInTheDocument());
  expect(screen.queryByRole('button', { name: 'Recover pending request' })).not.toBeInTheDocument();
  expect(h.post).toHaveBeenCalledOnce();
  expect(revisions).toHaveLength(1);
  const downloaded = await h.sdk.agents.downloadArtifact('agt_1', artifact.id);
  expect(Array.from(downloaded)).toEqual(Array.from(bytes));
});

it('uses observed receipt metadata only to reconcile replay while the acceptance identity check is still pending', async () => {
  const { h, field } = await setup();
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  const identity = deferred<string>();
  h.currentUser.mockReturnValueOnce(identity.promise);
  await act(async () => response.resolve(accepted()));
  expect(screen.getByText('Sending…')).toBeInTheDocument();
  expect(screen.queryByText('Sent')).not.toBeInTheDocument();
  expect(field).toHaveValue('');
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.durable(2, { type: 'message', role: 'user', content: 'Immediate local message' });
    h.durable(3, { type: 'turn_completed', turn_sequence: 1 });
  });
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(field).toHaveValue('');
  await act(async () => identity.resolve('alice'));
  await waitFor(() => expect(field).toHaveValue(''));
  expect(bubbles('Immediate local message')).toHaveLength(1);
});

it('retains two separately accepted identical messages until their own command appears', async () => {
  const { h, field, view, Agent } = await setup();
  click();
  await waitFor(() => expect(field).toHaveValue(''));
  fireEvent.change(field, { target: { value: 'Immediate local message' } });
  h.post.mockResolvedValueOnce(
    new Response(new ReadableStream(), {
      headers: {
        'content-type': 'text/event-stream',
        'x-cma-response-id': 'resp_2',
        'x-cma-command-id': 'cmd_2',
      },
    }),
  );
  click();
  await waitFor(() => expect(field).toHaveValue(''));
  expect(bubbles('Immediate local message')).toHaveLength(2);
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.durable(2, { type: 'message', role: 'user', content: 'Immediate local message' });
    h.durable(3, { type: 'turn_completed', turn_sequence: 1 });
  });
  expect(bubbles('Immediate local message')).toHaveLength(2);
  expect(screen.getAllByText('Sent')).toHaveLength(1);
  view.unmount();
  render(<Agent />);
  // The last accepted request stays visible while its current status is unavailable.
  await screen.findAllByText('Message status could not be confirmed. Refresh status.');
  expect(h.post).toHaveBeenCalledTimes(2);
  const next = deferred<Response>();
  h.post.mockReturnValueOnce(next.promise);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'After reload' },
  });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
  click();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(bubbles('After reload')).toHaveLength(1);
});

it('keeps correlation after a transient post-receipt identity failure without claiming acceptance or duplicating replay', async () => {
  const { h, field } = await setup();
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  const identity = deferred<string>();
  h.currentUser.mockReturnValueOnce(identity.promise);
  await act(async () => response.resolve(accepted()));
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.durable(2, { type: 'message', role: 'user', content: 'Immediate local message' });
    identity.reject(new Error('Identity temporarily unavailable'));
  });
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(field).toHaveValue('Immediate local message');
  expect(screen.queryByText('Sent')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Recover pending request' })).toBeEnabled();
  expect(h.post).toHaveBeenCalledOnce();
});

it('hides a pending preview on account change without releasing the identity-gated send', async () => {
  const { h, field, view, Agent } = await setup();
  const identity = deferred<string>();
  h.currentUser.mockReturnValueOnce(identity.promise);
  click();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  view.rerender(<Agent viewer="bob" />);
  expect(screen.queryByText('Sending…')).not.toBeInTheDocument();
  expect(screen.queryAllByRole('article', { name: 'You' })).toHaveLength(0);
  await act(async () => identity.resolve('alice'));
  expect(h.post).not.toHaveBeenCalled();
  expect(field).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
});

it('restores an untouched draft after definite refusal without presenting it as uncertain', async () => {
  const { h, field } = await setup();
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  expect(field).toHaveValue('');
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  await act(async () =>
    response.resolve(
      new Response(
        JSON.stringify({
          error: {
            type: 'invalid_request_error',
            message: 'Invalid input',
            code: 'invalid_response_input',
            param: 'input',
          },
        }),
        { status: 422, headers: { 'content-type': 'application/json' } },
      ),
    ),
  );
  await waitFor(() => expect(screen.getByText('Not sent')).toBeInTheDocument());
  expect(field).toHaveValue('Immediate local message');
  expect(screen.queryByRole('button', { name: 'Recover pending request' })).not.toBeInTheDocument();
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(h.post).toHaveBeenCalledOnce();
});

it('keeps a new draft when the outgoing input is refused and retains that input in its failure row', async () => {
  const { h, field } = await setup();
  const response = deferred<Response>();
  h.post.mockReturnValueOnce(response.promise);
  click();
  expect(field).toHaveValue('');
  fireEvent.change(field, { target: { value: 'My next draft' } });
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  await act(async () =>
    response.resolve(
      new Response(
        JSON.stringify({
          error: {
            type: 'invalid_request_error',
            message: 'Invalid input',
            code: 'invalid_response_input',
            param: 'input',
          },
        }),
        { status: 422, headers: { 'content-type': 'application/json' } },
      ),
    ),
  );
  await screen.findByText('Not sent');
  expect(field).toHaveValue('My next draft');
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(h.post).toHaveBeenCalledOnce();
  click();
  await screen.findByText('Sent');
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(within(bubbles('Immediate local message')[0]!).getByText('Not sent')).toBeInTheDocument();
  expect(bubbles('My next draft')).toHaveLength(1);
  expect(h.post).toHaveBeenCalledTimes(2);
});

it('keeps a legacy ambiguous submission unconfirmed with its draft intact and no automatic retry', async () => {
  const request = deferred<void>();
  const runAgent = vi.fn(() => request.promise);
  const client = agentClient({ runAgent });
  function LegacyAgent() {
    const controller = useAgentController(client, 'agt_1');
    return (
      <CmaClientProvider client={client}>
        <AgentSurface
          agentId="agt_1"
          viewerId="alice"
          lifecycleClient={client}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
          onNavigate={() => {}}
        />
      </CmaClientProvider>
    );
  }
  render(<LegacyAgent />);
  const field = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Legacy input\nwith a second line' } });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
  click();
  const pending = screen.getAllByRole('article', { name: 'You' }).at(-1)!;
  expect(pending.querySelector('p')?.textContent).toBe('Legacy input\nwith a second line');
  expect(screen.getByText('Sending…')).toBeInTheDocument();
  await waitFor(() => expect(runAgent).toHaveBeenCalledOnce());
  await act(async () => request.reject(new TypeError('Lost response before first event')));
  expect(screen.getByText('Awaiting confirmation')).toBeInTheDocument();
  expect(screen.queryByText('Not sent')).not.toBeInTheDocument();
  expect(field).toHaveValue('Legacy input\nwith a second line');
  expect(runAgent).toHaveBeenCalledOnce();
});

it('replaces a discovered then absent admitted message with its expiry explanation through the real controller', async () => {
  const { h } = await setup();
  h.listQueued.mockImplementation(async () => json(queuedPage('Immediate local message')));
  click();
  await screen.findByText('Sent');
  await waitFor(() => expect(h.listQueued.mock.calls.length).toBeGreaterThan(1));
  expect(bubbles('Immediate local message')).toHaveLength(1);
  h.listQueued.mockImplementation(async () =>
    json({ object: 'cma.agent.queued_inputs', agent_id: 'agt_1', data: [], next_after: null }),
  );
  const originalFetch = h.fetch.getMockImplementation()!;
  h.fetch.mockImplementation((url, init) =>
    String(url).endsWith('/commands/cmd_1')
      ? Promise.resolve(json(status(1, 'expired')))
      : originalFetch(url, init),
  );
  fireEvent(window, new Event('focus'));
  await screen.findByText('Expired before starting', {}, { timeout: 3000 });
  expect(bubbles('Immediate local message')).toHaveLength(1);
  expect(screen.queryByText('Sent')).not.toBeInTheDocument();
  expect(h.post).toHaveBeenCalledOnce();
});
