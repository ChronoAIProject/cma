import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { createClient } from '@chronoai/cma-sdk';
import { AgentV2Session } from './transport/client';
import { agentTransportJournal } from './transport/journal';
import { prepareAgentConnections } from '../connections/preparation';
import { workspaceFixture } from '../test/hierarchy';
import { ConnectionFailure, connectionClient, type ConnectionReport } from '../connections/client';
import { AgentSurface } from './AgentSurface';
import { accessRequests } from './accessRequests';
import { conversationFromTranscript } from './conversation';
import type { AgentController } from './controller';

const connected: ConnectionReport = {
  owner: 'owner',
  ready: true,
  checked_at: '2026-09-17T00:00:00Z',
  services: [
    {
      service: 'provider',
      display_name: 'Provider',
      kind: 'codex',
      state: 'satisfied',
      connection_mode: 'hosted',
    },
  ],
};
const missing: ConnectionReport = {
  ...connected,
  ready: false,
  services: connected.services.map((row) => ({ ...row, state: 'missing' })),
};

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  vi.spyOn(connectionClient, 'check').mockResolvedValue(connected);
  vi.spyOn(connectionClient, 'start');
  vi.spyOn(window, 'open').mockReturnValue(null);
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

async function setup(prepared = false, detailsOpen = false, finishConnections?: () => void) {
  const lifecycleClient = agentClient();
  if (prepared) prepareAgentConnections(lifecycleClient, 'agt_1', 'owner').handoff();
  const send = vi.fn<AgentController['send']>().mockResolvedValue(undefined);
  const controller: AgentController = {
    state: {
      status: 'ready',
      detail: AGENT_DETAIL,
      workspace: workspaceFixture(),
      conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
      ...(prepared
        ? {
            v2: new AgentV2Session(
              createClient(),
              agentTransportJournal(sessionStorage, 'owner', 'agt_1'),
              'owner',
              { storage: sessionStorage, currentUser: async () => 'owner' },
            ),
          }
        : {}),
    },
    inputAvailability: { allowed: true },
    resourceAvailability: { state: 'live', runtimeVersion: 'runtime-one' },
    send,
    reload: vi.fn(),
    clearActionError: vi.fn(),
    interrupt: vi.fn(),
    approve: vi.fn(),
    stop: vi.fn(),
    delete: vi.fn(),
  };
  const view = render(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      controller={controller}
      lifecycleClient={prepared ? lifecycleClient : undefined}
      detailsOpen={detailsOpen}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: '  Keep this message  ' } });
  if (finishConnections !== undefined) await act(async () => finishConnections());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
  return { ...view, send, field, controller, lifecycleClient };
}

const submit = () => fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));

it('sends ordinary live v2 messages without repeating the initial connection catalog check', async () => {
  const test = await setup(true);
  expect(connectionClient.check).toHaveBeenCalledOnce();
  await act(async () => submit());
  expect(test.send).toHaveBeenCalledExactlyOnceWith(
    'Keep this message',
    expect.any(Function),
    expect.any(Function),
  );
  expect(connectionClient.check).toHaveBeenCalledOnce();
  fireEvent.change(test.field, { target: { value: 'Next message' } });
  await act(async () => submit());
  expect(connectionClient.check).toHaveBeenCalledOnce();
  expect(test.send).toHaveBeenLastCalledWith(
    'Next message',
    expect.any(Function),
    expect.any(Function),
  );
});

it('keeps a live v2 send independent of the connection catalog after opening settings', async () => {
  const test = await setup(true);
  test.rerender(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      lifecycleClient={test.lifecycleClient}
      controller={test.controller}
      detailsOpen
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  await act(async () => submit());
  expect(screen.queryByRole('dialog', { name: 'Connect your accounts' })).not.toBeInTheDocument();
  expect(test.send).toHaveBeenCalledOnce();
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('sends after initial readiness completes with settings open without a new catalog request', async () => {
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockReturnValueOnce(
    new Promise((resolve) => (finish = resolve)),
  );
  const test = await setup(true, true, () => finish(connected));
  expect(connectionClient.check).toHaveBeenCalledOnce();
  test.rerender(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      lifecycleClient={test.lifecycleClient}
      controller={test.controller}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  await act(async () => submit());
  expect(screen.queryByRole('dialog', { name: 'Connect your accounts' })).not.toBeInTheDocument();
  expect(test.send).toHaveBeenCalledOnce();
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('lets live v2 admission and runtime permission requests own changed service grants', async () => {
  const test = await setup(true);
  if (test.controller.state.status !== 'ready') throw new Error('Ready fixture');
  test.rerender(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      lifecycleClient={test.lifecycleClient}
      controller={{
        ...test.controller,
        state: {
          ...test.controller.state,
          detail: { ...test.controller.state.detail, reach: ['new-service'] },
        },
      }}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  await act(async () => submit());
  expect(screen.queryByRole('dialog', { name: 'Connect your accounts' })).not.toBeInTheDocument();
  expect(test.send).toHaveBeenCalledOnce();
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('retains the connection gate when a v2 runtime is unavailable', async () => {
  const test = await setup(true);
  test.rerender(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      lifecycleClient={test.lifecycleClient}
      controller={{
        ...test.controller,
        resourceAvailability: { state: 'unavailable', notice: 'The sandbox is paused.' },
      }}
      detailsOpen
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  submit();
  expect(await screen.findByRole('dialog', { name: 'Connect your accounts' })).toBeInTheDocument();
  expect(test.send).not.toHaveBeenCalled();
});

it('cancels a pending connection check when access becomes uncertain and never releases its captured send', async () => {
  const test = await setup();
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockImplementationOnce(
    () => new Promise((resolve) => (finish = resolve)),
  );
  submit();
  const signal = vi.mocked(connectionClient.check).mock.calls.at(-1)?.[2];
  test.rerender(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      controller={test.controller}
      accessUncertain
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  expect(signal?.aborted).toBe(true);
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(test.field);
  expect(test.field).toHaveValue('  Keep this message  ');
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  await act(async () => finish(connected));
  expect(test.send).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('checks each send in the background, holds duplicate submissions, and sends the captured draft once', async () => {
  const test = await setup();
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockImplementationOnce(
    () => new Promise((resolve) => (finish = resolve)),
  );
  submit();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByText('Sending…')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(test.field).toBeEnabled();
  expect(test.send).not.toHaveBeenCalled();
  submit();
  fireEvent.change(test.field, { target: { value: 'A later draft' } });
  await act(async () => finish(connected));
  expect(test.send).toHaveBeenCalledExactlyOnceWith(
    'Keep this message',
    expect.any(Function),
    expect.any(Function),
  );
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(connectionClient.check).toHaveBeenLastCalledWith(
    { type: 'agent', id: 'agt_1' },
    undefined,
    expect.any(AbortSignal),
  );
  expect(window.open).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  act(() => test.send.mock.calls[0]?.[1]?.());
  expect(test.field).toHaveValue('A later draft');
  await act(async () => submit());
  expect(connectionClient.check).toHaveBeenCalledTimes(3);
  expect(test.send).toHaveBeenCalledTimes(2);
  expect(test.send).toHaveBeenLastCalledWith(
    'A later draft',
    expect.any(Function),
    expect.any(Function),
  );
});

it('opens connection recovery only for a missing connection and sends once after server verification', async () => {
  const test = await setup();
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  submit();
  expect(await screen.findByRole('dialog', { name: 'Connect your accounts' })).toBeInTheDocument();
  await screen.findByText('Connection needed');
  expect(test.send).not.toHaveBeenCalled();
  expect(test.field).toHaveValue('  Keep this message  ');
  expect(window.open).not.toHaveBeenCalled();
  vi.mocked(connectionClient.check).mockResolvedValue(connected);
  await waitFor(() => expect(test.send).toHaveBeenCalledOnce(), { timeout: 3000 });
  expect(test.send).toHaveBeenCalledWith(
    'Keep this message',
    expect.any(Function),
    expect.any(Function),
  );
  act(() => test.send.mock.calls[0]?.[1]?.());
  expect(test.field).toHaveValue('');
});

it('cancels connection recovery without sending or changing the draft', async () => {
  const test = await setup();
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  submit();
  await screen.findByRole('dialog', { name: 'Connect your accounts' });
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await act(async () => undefined);
  expect(test.send).not.toHaveBeenCalled();
  expect(test.field).toHaveValue('  Keep this message  ');
  expect(screen.getByText('Not sent')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
});

it.each([new ConnectionFailure(503), new ConnectionFailure(401)])(
  'keeps a failed verification inline and preserves the unsent draft: %s',
  async (error) => {
    const test = await setup();
    vi.mocked(connectionClient.check).mockRejectedValueOnce(error);
    submit();
    expect(await screen.findByText(error.message)).toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(test.send).not.toHaveBeenCalled();
    expect(test.field).toHaveValue('  Keep this message  ');
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
    await act(async () => submit());
    expect(test.send).toHaveBeenCalledOnce();
  },
);

it('does not infer a reconnect requirement from an unknown connection state', async () => {
  const test = await setup();
  vi.mocked(connectionClient.check).mockResolvedValue({
    ...missing,
    services: missing.services.map((row) => ({ ...row, state: 'unknown' })),
  });
  submit();
  await screen.findByText('The connections could not be verified. Please try sending again.');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(test.send).not.toHaveBeenCalled();
});

it('refuses a different owner before releasing a background send', async () => {
  const test = await setup();
  vi.mocked(connectionClient.check).mockResolvedValue({ ...connected, owner: 'different-owner' });
  submit();
  await screen.findByText('Your CMA account changed. Open the Agent again to continue.');
  expect(test.send).not.toHaveBeenCalled();
  expect(test.field).toHaveValue('  Keep this message  ');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('keeps the original owner bound when background verification opens recovery', async () => {
  const test = await setup();
  vi.mocked(connectionClient.check)
    .mockResolvedValueOnce(missing)
    .mockResolvedValue({ ...connected, owner: 'different-owner' });
  submit();
  await screen.findByText('Your CMA account changed. Reopen this connection step to continue.');
  expect(test.send).not.toHaveBeenCalled();
  expect(connectionClient.start).not.toHaveBeenCalled();
  expect(test.field).toHaveValue('  Keep this message  ');
});

it('aborts a background check and ignores its late reply after leaving the Agent', async () => {
  const test = await setup();
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockImplementationOnce(
    () => new Promise((resolve) => (finish = resolve)),
  );
  submit();
  const signal = vi.mocked(connectionClient.check).mock.calls.at(-1)?.[2];
  test.unmount();
  expect(signal?.aborted).toBe(true);
  await act(async () => finish(connected));
  expect(test.send).not.toHaveBeenCalled();
});

it('does not clear the draft when an already dispatched send accepts after access changes', async () => {
  const test = await setup();
  await act(async () => submit());
  expect(test.send).toHaveBeenCalledOnce();
  test.rerender(
    <AgentSurface
      agentId="agt_1"
      viewerId="owner"
      controller={test.controller}
      accessUncertain
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  act(() => test.send.mock.calls[0]?.[1]?.({ runId: 'old-generation' }));
  expect(test.field).toHaveValue('  Keep this message  ');
  expect(screen.queryByText('Sending…')).not.toBeInTheDocument();
  expect(screen.queryByText('Sent')).not.toBeInTheDocument();
});

it('keeps one durable access dialog when the Agent surface and resource footer refresh', async () => {
  vi.mocked(accessRequests.list).mockResolvedValue([
    {
      id: 'access-request',
      agent_id: 'agt_1',
      status: 'pending',
      services: ['api-github'],
      reason: 'Read repository',
    },
  ]);
  const view = await setup();
  await screen.findByRole('dialog', { name: 'Agent key access' });
  submit();
  await waitFor(() => expect(view.send).toHaveBeenCalledOnce());
  expect(screen.getAllByRole('dialog', { name: 'Agent key access' })).toHaveLength(1);
});
