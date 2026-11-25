import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { controller } from './AgentSurface.fixture';
import type { AgentController } from './controllerTypes';
import { turnTimeoutFixture } from './turnTimeout.fixture';
import { turnRetryAvailable } from './turnRetryAvailability';

const ready = () =>
  controller({
    status: 'ready',
    detail: AGENT_DETAIL,
    workspace: workspaceFixture(),
    conversation: turnTimeoutFixture(),
  });
const queuedInputs = {
  items: [
    { responseId: 'resp_queued', commandId: 'cmd_queued', revision: 1, input: 'Keep queued' },
  ],
  outcomes: [],
  managedCommandIds: new Set(['cmd_queued']),
  edit: vi.fn(),
  remove: vi.fn(),
  recover: vi.fn(),
  refresh: vi.fn(),
};

it('allows local preparation only with a ready authorized history and usable draft', () => {
  expect(turnRetryAvailable(ready(), true)).toBe(true);
  expect(turnRetryAvailable(ready(), false)).toBe(false);
  expect(
    turnRetryAvailable(ready(), true, {
      content: '',
      attachments: [],
      setContent: vi.fn(),
      setAttachments: vi.fn(),
      recoveryBlocked: true,
    }),
  ).toBe(false);
  expect(turnRetryAvailable(ready(), true, undefined, true)).toBe(false);
});

it.each([
  { pending: 'send' },
  { pendingMessage: 'Unconfirmed input' },
  { checkingInputReceipt: true },
  { pendingControl: true },
  { acceptedMessage: 'Accepted original' },
  { acceptedSteeringMessage: 'Accepted steering' },
  { acceptedInterruptionMessage: 'Accepted interruption' },
  { controlBaselineReady: false },
  { inputAvailability: { allowed: false, notice: 'The previous turn is not safe to reuse.' } },
  { queuedInputs },
  { queuedInputs: { ...queuedInputs, items: [], error: 'Queue status unknown' } },
] satisfies Partial<AgentController>[])(
  'does not prepare over uncertain/admitted work or a server gate %#',
  (overrides) => {
    const state = { ...ready(), ...overrides };
    const snapshot = state.queuedInputs?.items;
    expect(turnRetryAvailable(state, true)).toBe(false);
    expect(state.queuedInputs?.items).toBe(snapshot);
    expect(state.send).not.toHaveBeenCalled();
    for (const action of [queuedInputs.edit, queuedInputs.remove, queuedInputs.recover])
      expect(action).not.toHaveBeenCalled();
  },
);

it.each(['loading', 'unavailable'] as const)('waits for complete history when %s', (history) => {
  const original = ready();
  if (original.state.status !== 'ready') throw new Error('Fixture must be ready');
  const state = {
    ...original,
    state: {
      ...original.state,
      historyLoading: history === 'loading',
      historyUnavailable: history === 'unavailable',
    },
  };
  expect(turnRetryAvailable(state, true)).toBe(false);
});

it('allows local review after terminal removes the active response control binding', () => {
  expect(
    turnRetryAvailable(
      { ...ready(), controlsUnavailable: true, responseControls: 'unbound' },
      true,
    ),
  ).toBe(true);
});
