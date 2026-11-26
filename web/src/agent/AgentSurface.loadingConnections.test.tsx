import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { workspaceFixture, sandboxFixture } from '../test/hierarchy';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { AgentSurface } from './AgentSurface';
import { accessRequests } from './accessRequests';
import { conversationFromTranscript } from './conversation';
import type { AgentController } from './controller';
import { createClient } from '@cma/client';
import { AgentV2Session } from './transport/client';
import { agentTransportJournal, clearAgentTransport } from './transport/journal';

const connected: ConnectionReport = {
  owner: 'owner',
  ready: true,
  checked_at: '2026-10-14T00:00:00Z',
  services: [],
};
function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
}
function controller(): AgentController {
  return {
    state: {
      status: 'ready',
      detail: AGENT_DETAIL,
      workspace: workspaceFixture(),
      conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
    },
    inputAvailability: { allowed: true },
    resourceAvailability: { state: 'live', runtimeVersion: 'one' },
    send: vi.fn(async () => undefined),
    reload: vi.fn(),
    refreshDetail: vi.fn(),
    clearActionError: vi.fn(),
    interrupt: vi.fn(),
    approve: vi.fn(),
    stop: vi.fn(),
    delete: vi.fn(),
  };
}
function setup(overrides: Partial<React.ComponentProps<typeof AgentSurface>> = {}) {
  const props = {
    agentId: 'agt_1',
    viewerId: 'owner',
    controller: controller(),
    accessAuthorized: true,
    detailsOpen: false,
    onDetailsOpenChange: vi.fn(),
    onNavigate: vi.fn(),
    ...overrides,
  };
  return { ...render(<AgentSurface {...props} />), props };
}
beforeEach(() => {
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  vi.spyOn(connectionClient, 'check').mockResolvedValue(connected);
});
afterEach(() => vi.restoreAllMocks());

it('allows Send during history loading after current connection and parent checks', async () => {
  const pending = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(pending.promise);
  const ready = controller();
  const view = setup({ controller: { ...ready, state: { status: 'loading' } } });
  expect(connectionClient.check).toHaveBeenCalledExactlyOnceWith(
    { type: 'agent', id: 'agt_1' },
    undefined,
    expect.any(AbortSignal),
  );
  await act(async () => pending.resolve(connected));
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  view.rerender(
    <AgentSurface
      {...view.props}
      controller={{ ...ready, inputAvailability: { allowed: false } }}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep the draft' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  if (ready.state.status !== 'ready') throw new Error('Ready fixture');
  view.rerender(
    <AgentSurface
      {...view.props}
      controller={{ ...ready, state: { ...ready.state, historyLoading: true } }}
    />,
  );
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  expect(connectionClient.check).toHaveBeenCalledOnce();
  const fresh = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(fresh.promise);
  fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  expect(ready.send).not.toHaveBeenCalled();
  await act(async () => fresh.resolve(connected));
  expect(ready.send).toHaveBeenCalledExactlyOnceWith(
    'Keep the draft',
    expect.any(Function),
    expect.any(Function),
    expect.any(Function),
  );
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it('waits for the early check when Agent loading finishes first without launching another check', async () => {
  const pending = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(pending.promise);
  const ready = controller();
  const view = setup({ controller: { ...ready, state: { status: 'loading' } } });
  view.rerender(<AgentSurface {...view.props} controller={ready} />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Wait' } });
  expect(
    screen.queryByText('Verify the required connections to continue.'),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  expect(ready.send).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Wait');
  await act(async () => pending.resolve(connected));
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('preserves actual lifecycle notices while connection readiness is checked in the background', () => {
  vi.mocked(connectionClient.check).mockReturnValueOnce(new Promise(() => undefined));
  setup({
    controller: {
      ...controller(),
      inputAvailability: { allowed: false, notice: 'This sandbox is resuming.' },
    },
  });
  expect(screen.getByText('This sandbox is resuming.')).toBeInTheDocument();
  expect(
    screen.queryByText('Verify the required connections to continue.'),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
});

it('keeps connection failures actionable and retries without flashing a generic verification notice', async () => {
  vi.mocked(connectionClient.check).mockRejectedValueOnce(new Error('Connection check failed.'));
  const view = setup();
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep my draft' } });
  expect(await screen.findByText('Connection check failed.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  const retry = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(retry.promise);
  fireEvent.click(screen.getByRole('button', { name: 'Retry connections' }));
  expect(
    screen.queryByText('Verify the required connections to continue.'),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(field).toHaveValue('Keep my draft');
  expect(view.props.controller.send).not.toHaveBeenCalled();
  await act(async () => retry.resolve(connected));
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  expect(field).toHaveValue('Keep my draft');
});

it('still prompts for confirmed missing services while leaving the draft unsent', async () => {
  vi.mocked(connectionClient.check).mockResolvedValue({
    ...connected,
    ready: false,
    services: [
      {
        service: 'provider',
        display_name: 'Provider',
        kind: 'codex',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  });
  const view = setup();
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Keep my draft' },
  });
  expect(
    await screen.findByRole('dialog', { name: 'Connect required services' }),
  ).toBeInTheDocument();
  expect(await screen.findByText('Connection needed')).toBeInTheDocument();
  expect(view.props.controller.send).not.toHaveBeenCalled();
  expect(
    screen.queryByText('Verify the required connections to continue.'),
  ).not.toBeInTheDocument();
});

it.each(['not authorized', 'uncertain', 'load refusal', 'wrong parent owner'] as const)(
  'does not check connections when %s is already known',
  async (reason) => {
    const ready = controller();
    const value =
      reason === 'wrong parent owner'
        ? {
            ...ready,
            progressObservation: {
              sandbox: { ...sandboxFixture(), owner: 'different-owner' },
              stale: false,
            },
          }
        : {
            ...ready,
            state:
              reason === 'load refusal'
                ? { status: 'error' as const, message: 'Access refused' }
                : { status: 'loading' as const },
          };
    setup({
      controller: value,
      accessAuthorized: reason !== 'not authorized',
      accessUncertain: reason === 'uncertain',
    });
    await act(async () => undefined);
    expect(connectionClient.check).not.toHaveBeenCalled();
    expect(ready.send).not.toHaveBeenCalled();
  },
);

it('keeps compatibility clients on the existing ready-state connection check', async () => {
  const ready = controller();
  const view = setup({
    accessAuthorized: false,
    controller: { ...ready, state: { status: 'loading' } },
  });
  expect(connectionClient.check).not.toHaveBeenCalled();
  view.rerender(<AgentSurface {...view.props} controller={ready} />);
  await waitFor(() => expect(connectionClient.check).toHaveBeenCalledOnce());
});

it.each(['owner', 'target', 'client', 'uncertain', 'refusal', 'unmount'] as const)(
  'cancels an early check on %s change and ignores its late ready result',
  async (change) => {
    const old = deferred<ConnectionReport>();
    const next = deferred<ConnectionReport>();
    vi.mocked(connectionClient.check)
      .mockReturnValueOnce(old.promise)
      .mockReturnValue(next.promise);
    const ready = controller();
    const view = setup({
      lifecycleClient: agentClient(),
      controller: { ...ready, state: { status: 'loading' } },
    });
    const signal = vi.mocked(connectionClient.check).mock.calls[0]?.[2];
    if (change === 'unmount') view.unmount();
    else
      view.rerender(
        <AgentSurface
          {...view.props}
          controller={
            change === 'refusal'
              ? { ...ready, state: { status: 'error', message: 'Refused' } }
              : ready
          }
          {...(change === 'owner'
            ? { viewerId: 'new-owner' }
            : change === 'target'
              ? { agentId: 'agt_other' }
              : change === 'client'
                ? { lifecycleClient: agentClient() }
                : change === 'uncertain'
                  ? { accessUncertain: true }
                  : {})}
        />,
      );
    expect(signal?.aborted).toBe(true);
    await act(async () => old.resolve(connected));
    const field = screen.queryByRole('textbox', { name: 'Message' });
    if (field) {
      fireEvent.change(field, { target: { value: 'Must stay held' } });
      expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    }
    expect(ready.send).not.toHaveBeenCalled();
  },
);

it('drops a previously ready result when access becomes uncertain and requires a new check on return', async () => {
  const view = setup();
  await act(async () => undefined);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep my message' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  view.rerender(<AgentSurface {...view.props} accessUncertain />);
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  const next = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(next.promise);
  view.rerender(<AgentSurface {...view.props} />);
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  await act(async () => next.resolve(connected));
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  expect(field).toHaveValue('Keep my message');
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it('rejects an early result for a different owner after the conversation finishes loading', async () => {
  vi.mocked(connectionClient.check).mockResolvedValue({ ...connected, owner: 'someone-else' });
  const ready = controller();
  const view = setup({ controller: { ...ready, state: { status: 'loading' } } });
  await act(async () => undefined);
  view.rerender(<AgentSurface {...view.props} controller={ready} />);
  expect(
    screen.getByText('Your CMA account changed. Open the Agent again to continue.'),
  ).toBeInTheDocument();
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Not sent' },
  });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
});

it('shows a settled local queued edit with confirmed owner access during connection verification', async () => {
  sessionStorage.clear();
  const owner = 'owner';
  const journal = agentTransportJournal(sessionStorage, owner, 'agt_1');
  journal.select(true);
  const intent = journal.queuedInputs.retain({
    key: 'edit-key',
    responseId: 'response-id',
    commandId: 'command-id',
    revision: 1,
    original: 'Original',
    action: 'edit',
    text: 'My unapplied draft',
  });
  journal.queuedInputs.settle(intent, true);
  const ready = controller();
  if (ready.state.status !== 'ready') throw new Error('Ready fixture');
  const dismiss = vi.fn((key: string) => journal.queuedInputs.dismissUnappliedEdit(key));
  const session = new AgentV2Session(createClient(), journal, owner, {
    storage: sessionStorage,
    currentUser: async () => owner,
  });
  const queuedInputs = {
    items: [],
    outcomes: [],
    managedCommandIds: new Set<string>(),
    unappliedEdits: journal.queuedInputs
      .unappliedEdits()
      .map(({ key, text }) => ({ key, text: text! })),
    dismissUnappliedEdit: dismiss,
    refresh: vi.fn(),
    edit: vi.fn(),
    remove: vi.fn(),
    recover: vi.fn(),
  };
  const pendingConnection = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValue(new Promise(() => {}));
  vi.mocked(connectionClient.check).mockReturnValueOnce(pendingConnection.promise);
  let view = setup({
    controller: {
      ...ready,
      state: { ...ready.state, v2: session },
      queuedInputs,
      controlBaselineReady: true,
    },
  });
  const draft = () => screen.queryByRole('textbox', { name: 'Unapplied edit' });
  expect(draft()).toHaveValue('My unapplied draft');
  expect(draft()).toHaveAttribute('readonly');
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  await act(async () => pendingConnection.resolve({ ...connected, owner: 'another-owner' }));
  expect(draft()).toBeNull();
  expect(
    screen.getByText('Your CMA account changed. Open the Agent again to continue.'),
  ).toBeVisible();
  const retry = deferred<ConnectionReport>();
  vi.mocked(connectionClient.check).mockReturnValueOnce(retry.promise);
  fireEvent.click(screen.getByRole('button', { name: 'Retry connections' }));
  expect(draft()).toBeNull();
  await act(async () => retry.resolve(connected));
  expect(draft()).toHaveValue('My unapplied draft');
  view.rerender(<AgentSurface {...view.props} accessUncertain />);
  expect(draft()).toBeNull();
  view.rerender(<AgentSurface {...view.props} accessAuthorized={false} />);
  expect(draft()).toBeNull();
  view.rerender(<AgentSurface {...view.props} viewerId="another-owner" />);
  expect(draft()).toBeNull();
  view.rerender(<AgentSurface {...view.props} />);
  expect(draft()).toHaveValue('My unapplied draft');
  view.unmount();
  const reopened = agentTransportJournal(sessionStorage, owner, 'agt_1');
  const reopenedSession = new AgentV2Session(createClient(), reopened, owner, {
    storage: sessionStorage,
    currentUser: async () => owner,
  });
  view = setup({
    controller: {
      ...ready,
      state: { ...ready.state, v2: reopenedSession },
      queuedInputs,
      controlBaselineReady: true,
    },
  });
  expect(draft()).toHaveValue('My unapplied draft');
  const user = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  await user.click(screen.getByRole('button', { name: 'Copy edit' }));
  expect(copy).toHaveBeenCalledExactlyOnceWith('My unapplied draft');
  await user.click(screen.getByRole('button', { name: 'Dismiss draft' }));
  expect(dismiss).toHaveBeenCalledExactlyOnceWith('edit-key');
  expect(reopened.queuedInputs.unappliedEdits()).toEqual([]);
  clearAgentTransport(sessionStorage);
  agentTransportJournal(sessionStorage, owner, 'agt_1').select(true);
  view.rerender(<AgentSurface {...view.props} />);
  expect(draft()).toBeNull();
  expect(queuedInputs.edit).not.toHaveBeenCalled();
  expect(queuedInputs.remove).not.toHaveBeenCalled();
  expect(queuedInputs.recover).not.toHaveBeenCalled();
  expect(ready.send).not.toHaveBeenCalled();
});
