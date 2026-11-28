import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../../api/context';
import { connectionClient } from '../../connections/client';
import { AGENT_DETAIL } from '../../test/agent';
import { accessRequests } from '../accessRequests';
import { AgentSurface } from '../AgentSurface';
import { useAgentController } from '../controller';
import { binding, cursor, harness, incarnation, item, json, page } from './controller.fixture';
import { observation } from './controls.fixture';
import { deferred } from './load.test.fixture';

const agentId = 'agt_1';
const stop = () => screen.getByRole('button', { name: 'Stop response' });
const escape = () =>
  fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message' }), { key: 'Escape' });
const firstTurn = { type: 'turn_started', turn_sequence: 1 };

function openFirstReply(purpose: 'setup' | 'task' = 'task', desktop = false) {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: AGENT_DETAIL.createdAt,
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const transport = harness();
  const { client } = transport;
  vi.spyOn(client, 'readAgent').mockResolvedValue({
    ...AGENT_DETAIL,
    desktop,
    status: 'working',
    firstMessage: {
      purpose,
      state: 'write_confirmed',
      queuedAt: AGENT_DETAIL.createdAt,
      turnSequence: 1,
    },
  });
  let interrupt!: () => Promise<void>;
  function Agent() {
    const controller = useAgentController(client, agentId, { viewerId: 'alice' });
    interrupt = controller.interrupt;
    return (
      <CmaClientProvider client={client}>
        <AgentSurface
          agentId={agentId}
          viewerId="alice"
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => undefined}
          onNavigate={() => undefined}
        />
      </CmaClientProvider>
    );
  }
  return { transport, mount: () => render(<Agent />), captureStop: () => interrupt };
}

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it.each([
  ['task', false],
  ['setup', true],
] as const)(
  'stops the exact inline %s reply after a live binding on desktop=%s',
  async (purpose, desktop) => {
    const { transport, mount, captureStop } = openFirstReply(purpose, desktop);
    const availability = deferred<Response>();
    const admission = deferred<Response>();
    transport.observe.mockReturnValueOnce(availability.promise);
    transport.control.mockReturnValueOnce(admission.promise);
    mount();
    await waitFor(() => expect(transport.streaming).toBe(true));
    expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
    await act(async () => transport.durable(1, firstTurn, binding));
    expect(stop()).toBeDisabled();
    expect(stop()).toHaveAccessibleDescription('Checking response controls…');
    escape();
    fireEvent.click(stop());
    expect(transport.control).not.toHaveBeenCalled();
    await act(async () => availability.resolve(json(observation())));
    expect(stop()).toBeEnabled();
    const staleStop = captureStop();
    fireEvent.click(stop());
    escape();
    fireEvent.click(screen.getByRole('button', { name: 'Stopping response' }));
    await waitFor(() => expect(transport.control).toHaveBeenCalledOnce());
    const [responseId, request] = transport.control.mock.lastCall!;
    expect(responseId).toBe('resp_1');
    expect(JSON.parse(String(request?.body))).toEqual({
      command: { type: 'cancel' },
      expected_agent_revision: 10,
      turn_sequence: 1,
    });
    expect(new Headers(request?.headers).get('idempotency-key')).toBeTruthy();
    const originalReceipt = await transport.control.getMockImplementation()!(responseId, request);
    await act(async () => admission.resolve(originalReceipt));
    expect(screen.getByRole('button', { name: 'Stopping response' })).toBeDisabled();
    expect(screen.queryByText('Response interrupted', { exact: true })).not.toBeInTheDocument();
    await act(async () => {
      transport.durable(2, { type: 'turn_cancelled', turn_sequence: 1 });
      transport.durable(
        3,
        { type: 'turn_started', turn_sequence: 3 },
        {
          response_id: 'resp_successor',
          command_id: 'cmd_successor',
          turn_sequence: 3,
        },
      );
    });
    expect(stop()).toBeEnabled();
    await act(staleStop);
    expect(transport.control).toHaveBeenCalledOnce();
    expect(transport.post).not.toHaveBeenCalled();
  },
);

it.each([
  ['task', false],
  ['setup', true],
] as const)(
  'enables a bound %s reply from its head and clears Stop on completion on desktop=%s',
  async (purpose, desktop) => {
    const { transport, mount, captureStop } = openFirstReply(purpose, desktop);
    transport.head.mockResolvedValueOnce(
      json({
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: agentId,
        incarnation,
        through_cursor: cursor(1),
        active_turn: item(1, firstTurn, binding),
      }),
    );
    transport.snapshot.mockResolvedValueOnce(json(page([item(1, firstTurn, binding)], 1)));
    mount();
    await waitFor(() => expect(stop()).toBeEnabled());
    const staleStop = captureStop();
    await act(async () => transport.durable(2, { type: 'turn_completed', turn_sequence: 1 }));
    expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
    await act(staleStop);
    expect(transport.control).not.toHaveBeenCalled();
  },
);

it('recovers the original Stop after a lost acknowledgement while the reply stays active', async () => {
  const { transport, mount } = openFirstReply();
  transport.snapshot.mockResolvedValueOnce(json(page([item(1, firstTurn, binding)], 1)));
  transport.control.mockRejectedValueOnce(new TypeError('lost control receipt'));
  mount();
  await waitFor(() => expect(stop()).toBeEnabled());
  fireEvent.click(stop());
  const recover = await screen.findByRole('button', { name: 'Recover pending request' });
  expect(stop()).toBeDisabled();
  expect(stop()).toHaveAccessibleDescription('Checking Stop request…');
  escape();
  expect(transport.control).toHaveBeenCalledOnce();
  fireEvent.click(recover);
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Stopping response' })).toBeDisabled(),
  );
  expect(transport.control).toHaveBeenCalledTimes(2);
  const [originalTarget, original] = transport.control.mock.calls[0]!;
  const [replayedTarget, replayed] = transport.control.mock.calls[1]!;
  expect(replayedTarget).toBe(originalTarget);
  expect(replayed?.body).toBe(original?.body);
  expect(new Headers(replayed?.headers).get('idempotency-key')).toBe(
    new Headers(original?.headers).get('idempotency-key'),
  );
});

it.each([
  [
    'legacy unbound',
    null,
    undefined,
    'Stop is unavailable because this turn has no bound response.',
  ],
  [
    'unsupported',
    binding,
    json({
      ...observation(),
      runtime_support: { cancel: false, steer: true, resolve_approval: true },
    }),
    'This Agent’s runtime does not support stopping this response.',
  ],
  [
    'denied',
    binding,
    json(
      { error: { type: 'permission_error', code: 'forbidden', message: 'Denied', param: null } },
      403,
    ),
    'Only the owner can stop this response.',
  ],
  [
    'inactive',
    binding,
    json({ ...observation(), target_active: false }),
    'This response is no longer active.',
  ],
] as const)(
  'explains why Stop is unavailable for a %s first reply',
  async (_, target, availability, reason) => {
    const { transport, mount } = openFirstReply();
    transport.snapshot.mockResolvedValueOnce(json(page([item(1, firstTurn, target)], 1)));
    if (availability) transport.observe.mockResolvedValueOnce(availability);
    mount();
    await waitFor(() => expect(stop()).toHaveAccessibleDescription(reason));
    expect(stop()).toBeDisabled();
    escape();
    expect(transport.control).not.toHaveBeenCalled();
  },
);
