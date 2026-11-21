import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { ApiError, createClient } from '@cma/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { accessRequests } from './accessRequests';
import { controller } from './AgentSurface.fixture';
import { AgentSurface } from './AgentSurface';
import { conversationFromTranscript } from './conversation';
import type { AgentResourceState } from './controllerTypes';
import { AgentV2Session } from './transport/client';
import { agentTransportJournal } from './transport/journal';

const flag = 'VITE_FIRST_TEXT_SEND_INTENT_ENABLED';
const connected: ConnectionReport = {
  owner: 'owner',
  ready: true,
  checked_at: '2026-09-17T00:00:00Z',
  services: [],
};
const missing: ConnectionReport = {
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
};
const draft = '  Keep this message  ';

beforeEach(() => {
  vi.stubEnv(flag, 'true');
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  vi.spyOn(connectionClient, 'check').mockResolvedValue(connected);
  sessionStorage.clear();
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

function holdCheck() {
  let resolve!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockReturnValueOnce(
    new Promise((complete) => (resolve = complete)),
  );
  return resolve;
}

function compose() {
  const client = agentClient();
  const v2 = new AgentV2Session(
    createClient(),
    agentTransportJournal(sessionStorage, 'owner', AGENT_DETAIL.id),
    'owner',
    { storage: sessionStorage, currentUser: async () => 'owner' },
  );
  const state: Extract<AgentResourceState, { status: 'ready' }> = {
    status: 'ready',
    detail: AGENT_DETAIL,
    workspace: workspaceFixture(),
    conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
    v2,
  };
  const send = vi.fn().mockResolvedValue(undefined);
  const ready = controller(state, undefined, { send });
  const draw = (overrides: Partial<Parameters<typeof AgentSurface>[0]> = {}) => (
    <AgentSurface
      agentId={AGENT_DETAIL.id}
      controller={ready}
      viewerId="owner"
      lifecycleClient={client}
      accessAuthorized
      firstSendContext={{ authEpoch: 1, visible: true }}
      detailsOpen={false}
      onDetailsOpenChange={vi.fn()}
      onNavigate={vi.fn()}
      {...overrides}
    />
  );
  const view = render(draw());
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: draft } });
  return { ...view, draw, field, ready, send, state };
}

const sendButton = () => screen.getByRole('button', { name: 'Send message' });
const cancelButton = () => screen.getByRole('button', { name: 'Cancel send' });

it('retains the local draft and releases one send only after the newer action check', async () => {
  const completePassive = holdCheck(),
    completeAction = holdCheck();
  const { field, send } = compose();
  expect(sendButton()).toBeEnabled();
  expect(connectionClient.check).toHaveBeenCalledTimes(1);
  fireEvent.click(sendButton());
  fireEvent.click(sendButton());
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(send).not.toHaveBeenCalled();
  expect(field).toHaveValue(draft);
  expect(cancelButton()).toBeVisible();
  await act(async () => completePassive(connected));
  expect(send).not.toHaveBeenCalled();
  expect(field).toHaveValue(draft);
  await act(async () => completeAction(connected));
  expect(send).toHaveBeenCalledExactlyOnceWith(
    'Keep this message',
    expect.any(Function),
    expect.any(Function),
    expect.any(Function),
  );
  expect(screen.queryByRole('button', { name: 'Cancel send' })).not.toBeInTheDocument();
  fireEvent.change(field, { target: { value: 'Second ordinary message' } });
  await act(async () => fireEvent.click(sendButton()));
  expect(send).toHaveBeenLastCalledWith(
    'Second ordinary message',
    expect.any(Function),
    expect.any(Function),
    expect.any(Function),
  );
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it.each(['cancel', 'draft', 'attached draft', 'running', 'view', 'login', 'revocation'] as const)(
  'withdraws a pending first text Send after %s',
  async (change) => {
    const completePassive = holdCheck(),
      completeAction = holdCheck();
    const view = compose();
    fireEvent.click(sendButton());
    const signal = vi.mocked(connectionClient.check).mock.calls[1]?.[2];
    expect(signal?.aborted).toBe(false);
    if (change === 'cancel') fireEvent.click(cancelButton());
    if (change === 'draft') fireEvent.change(view.field, { target: { value: 'My revised text' } });
    if (change === 'attached draft')
      view.rerender(
        view.draw({
          draft: {
            content: draft,
            setContent: vi.fn(),
            setAttachments: vi.fn(),
            attachments: [
              {
                key: 'photo',
                status: 'reselect',
                description: { name: 'photo.png', size: 16, mediaType: 'image/png' },
              },
            ],
          },
        }),
      );
    if (change === 'view')
      view.rerender(view.draw({ firstSendContext: { authEpoch: 1, visible: false } }));
    if (change === 'login')
      view.rerender(view.draw({ firstSendContext: { authEpoch: 2, visible: true } }));
    if (change === 'revocation')
      view.rerender(view.draw({ accessUncertain: true, accessAuthorized: false }));
    if (change === 'running')
      view.rerender(
        view.draw({
          controller: {
            ...view.ready,
            state: {
              ...view.state,
              conversation: { ...view.state.conversation, activeRunId: 'active-turn' },
            },
          },
        }),
      );
    expect(signal?.aborted).toBe(true);
    await act(async () => {
      completeAction(connected);
      completePassive(connected);
    });
    expect(view.send).not.toHaveBeenCalled();
    expect(view.field).toHaveValue(change === 'draft' ? 'My revised text' : draft);
    expect(screen.queryByRole('button', { name: 'Cancel send' })).not.toBeInTheDocument();
  },
);

it('lets the owner cancel required account setup without sending or losing the draft', async () => {
  const completePassive = holdCheck();
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  const { field, send } = compose();
  fireEvent.click(sendButton());
  const wizard = await screen.findByRole('dialog', { name: 'Connect your accounts' });
  await act(async () => completePassive(missing));
  expect(send).not.toHaveBeenCalled();
  fireEvent.click(within(wizard).getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(field).toHaveValue(draft);
  expect(send).not.toHaveBeenCalled();
});

it('lets a slash-command draft open and cancel account setup after passive refusal', async () => {
  const completePassive = holdCheck();
  vi.mocked(connectionClient.check).mockResolvedValue(missing);
  const { field, send } = compose();
  fireEvent.change(field, { target: { value: '/status' } });
  await act(async () => completePassive(missing));
  expect(sendButton()).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Connect accounts' }));
  const wizard = await screen.findByRole('dialog', { name: 'Connect your accounts' });
  fireEvent.click(within(wizard).getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Connect accounts' })).toBeEnabled();
  expect(field).toHaveValue('/status');
  expect(send).not.toHaveBeenCalled();
});

it('uses the same cooldown after Retry-After and needs a second explicit Send after refusal', async () => {
  const completePassive = holdCheck();
  vi.mocked(connectionClient.check).mockRejectedValueOnce(
    new ApiError(429, 'Retry later', undefined, undefined, 60_000),
  );
  const { field, send } = compose();
  await act(async () => fireEvent.click(sendButton()));
  expect(screen.getByRole('alert')).toHaveTextContent('Retry later');
  await act(async () => {
    completePassive(connected);
    fireEvent.click(sendButton());
  });
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(send).not.toHaveBeenCalled();
  expect(field).toHaveValue(draft);
});

it.each([
  { ...connected, owner: 'someone-else' },
  { ...connected, ready: false },
  { ...connected, ready: 'true' },
  { ...missing, ready: true, services: [{ ...missing.services[0]!, state: 'unknown' }] },
])(
  'keeps the draft when the action check cannot establish current authority (%j)',
  async (report) => {
    const completePassive = holdCheck();
    vi.mocked(connectionClient.check).mockResolvedValueOnce(report as ConnectionReport);
    const { field, send } = compose();
    await act(async () => fireEvent.click(sendButton()));
    await act(async () => completePassive(connected));
    expect(send).not.toHaveBeenCalled();
    expect(field).toHaveValue(draft);
  },
);

it('keeps the established gate when the first Send flag is unset or the draft is a command', async () => {
  const completePassive = holdCheck();
  vi.stubEnv(flag, undefined);
  const view = compose();
  expect(sendButton()).toBeDisabled();
  vi.stubEnv(flag, 'true');
  view.rerender(view.draw());
  fireEvent.change(view.field, { target: { value: '/status' } });
  expect(sendButton()).toBeDisabled();
  expect(view.send).not.toHaveBeenCalled();
  await act(async () => completePassive(connected));
});
