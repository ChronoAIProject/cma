import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { harness, json } from './transport/controller.fixture';
import { deferred, status } from './transport/queuedReconciliation.fixture';

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-11-20T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

async function setup() {
  const h = harness();
  const receipt = deferred<Response>();
  h.post.mockRejectedValueOnce(new TypeError('Network unavailable'));
  h.inputReceipt.mockReturnValueOnce(receipt.promise);
  function Agent() {
    const controller = useAgentController(h.client, 'agt_1', { viewerId: 'alice' });
    return (
      <CmaClientProvider client={h.client}>
        <AgentSurface
          agentId="agt_1"
          viewerId="alice"
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
  const field = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep this exact request' } });
  const send = screen.getByRole('button', { name: 'Send message' });
  await waitFor(() => expect(send).toBeEnabled());
  fireEvent.click(send);
  await screen.findByText('CMA could not be reached. Check your connection, then try again.');
  await waitFor(() => expect(h.inputReceipt).toHaveBeenCalledOnce());
  expect(field).toHaveValue('Keep this exact request');
  expect(send).toBeDisabled();
  expect(h.post).toHaveBeenCalledOnce();
  return { h, receipt, field, send, view };
}

it('removes only the resolved send error when its automatic receipt confirms acceptance', async () => {
  const { h, receipt, field } = await setup();
  await act(async () => receipt.resolve(json(status(1, 'completed'))));
  await screen.findByText('Sent');
  expect(field).toHaveValue('');
  expect(screen.queryByText('Awaiting confirmation')).not.toBeInTheDocument();
  expect(screen.queryByText('Checking your message receipt')).not.toBeInTheDocument();
  await waitFor(() =>
    expect(
      screen.queryByText('CMA could not be reached. Check your connection, then try again.'),
    ).not.toBeInTheDocument(),
  );
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.inputReceipt).toHaveBeenCalledOnce();
  const sent = h.post.mock.calls[0]![0]!;
  const lookup = h.inputReceipt.mock.calls[0]![0]!;
  expect(lookup.body).toBe(sent.body);
  expect(new Headers(lookup.headers).get('idempotency-key')).toBe(
    new Headers(sent.headers).get('idempotency-key'),
  );
});

it('keeps the uncertain error and draft when the automatic lookup finds no receipt', async () => {
  const { h, receipt, field, send } = await setup();
  await act(async () => receipt.resolve(json(null)));
  await screen.findByRole('button', { name: 'Recover pending request' });
  expect(
    screen.getByText('CMA could not be reached. Check your connection, then try again.'),
  ).toBeInTheDocument();
  expect(field).toHaveValue('Keep this exact request');
  expect(send).toBeDisabled();
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.inputReceipt).toHaveBeenCalledOnce();
});

it('reveals an independent live-stream failure after the send receipt resolves', async () => {
  const { h, receipt, field } = await setup();
  await act(async () => h.live('text_delta', 'Invalid fixture frame', -1));
  await act(async () => receipt.resolve(json(status(1, 'completed'))));
  await screen.findByText('Sent');
  expect(field).toHaveValue('');
  await screen.findByText(/Live Agent updates are unavailable/);
  expect(h.post).toHaveBeenCalledOnce();
});
