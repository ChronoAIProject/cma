import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentDetail, SandboxDetail } from '../api/types';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { revivalInputAllowed } from './bootstrap';
import { useSandboxInput } from './sandboxInput';
import { useAgentController } from './controller';
import { harness } from './transport/controller.fixture';
import { agentTransportJournal } from './transport/journal';

const agent: AgentDetail = { ...AGENT_DETAIL, status: 'paused' };
const base = sandboxFixture('reviving');
const parent: SandboxDetail = {
  ...base,
  owner: 'alice',
  lifecycle: {
    ...base.lifecycle!,
    operation: {
      purpose: 'revive',
      phase: 'runtime_provisioning',
      startedAt: AGENT_DETAIL.createdAt,
    },
  },
};
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  sessionStorage.clear();
});

it.each([
  ['unknown owner', agent, parent, undefined],
  ['another owner', agent, parent, 'bob'],
  ['another Agent', { ...agent, id: 'agt_other' }, parent, 'alice'],
  ['another parent', { ...agent, sandbox: { ...agent.sandbox, id: 'sbx_other' } }, parent, 'alice'],
  ['stopped Agent', { ...agent, status: 'stopped' }, parent, 'alice'],
  ['stop intent', { ...agent, stopReason: 'owner_stop' }, parent, 'alice'],
  ['pausing', agent, { ...parent, status: 'pausing' }, 'alice'],
  [
    'no revival',
    agent,
    { ...parent, lifecycle: { ...parent.lifecycle!, operation: undefined } },
    'alice',
  ],
  [
    'rotation',
    agent,
    {
      ...parent,
      lifecycle: {
        ...parent.lifecycle!,
        operation: { ...parent.lifecycle!.operation!, purpose: 'image_rotation' },
      },
    },
    'alice',
  ],
  [
    'checkpoint',
    agent,
    {
      ...parent,
      lifecycle: {
        ...parent.lifecycle!,
        checkpoint: { phase: 'capture', epoch: 1, startedAt: AGENT_DETAIL.createdAt },
      },
    },
    'alice',
  ],
] as const)('refuses queued revival input with %s', (_name, detail, sandbox, owner) => {
  expect(revivalInputAllowed(detail, sandbox, owner)).toBe(false);
});

it('requires a fresh Agent and parent, keeps resources inactive, and withdraws stale input', async () => {
  vi.useFakeTimers();
  const readAgent = vi.fn().mockResolvedValue(agent);
  const readSandbox = vi.fn().mockResolvedValue(parent);
  const client = agentClient({ readAgent, readSandbox });
  const view = renderHook(() =>
    useSandboxInput(client, agent.id, parent.id, false, undefined, 'alice', true),
  );
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(async () => {});
  expect(view.result.current.inputAvailability).toEqual({ allowed: true });
  expect(view.result.current.resourceAvailability.state).toBe('inactive');
  readAgent.mockImplementation(() => new Promise(() => {}));
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  view.unmount();
});

it('keeps the existing non-v2 transport held during revival', async () => {
  const client = agentClient({ readAgent: async () => agent, readSandbox: async () => parent });
  const view = renderHook(() =>
    useSandboxInput(client, agent.id, parent.id, false, undefined, 'alice'),
  );
  await act(async () => {});
  expect(view.result.current.inputAvailability.allowed).toBe(false);
});

it('does not add Agent reads to ordinary settled availability polls', async () => {
  const readAgent = vi.fn(async () => agent);
  const settled = { ...sandboxFixture(), owner: 'alice' };
  const client = agentClient({ readAgent, readSandbox: async () => settled });
  const view = renderHook(() =>
    useSandboxInput(client, agent.id, settled.id, false, undefined, 'alice', true),
  );
  await act(async () => {});
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  expect(readAgent).not.toHaveBeenCalled();
});

it('durably accepts one v2 message while resuming without claiming the runtime is Ready', async () => {
  sessionStorage.clear();
  const h = harness();
  vi.spyOn(h.client, 'readAgent').mockResolvedValue(agent);
  vi.spyOn(h.client, 'readSandbox').mockResolvedValue(parent);
  const view = renderHook(() => useAgentController(h.client, agent.id, { viewerId: 'alice' }));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  const accepted = vi.fn();
  await act(() => view.result.current.send('Continue after this home resumes', accepted));
  expect(h.post).toHaveBeenCalledOnce();
  expect(accepted).toHaveBeenCalledOnce();
  expect(h.client.runAgent).not.toHaveBeenCalled();
  expect(agentTransportJournal(sessionStorage, 'alice', agent.id).read()).toMatchObject({
    state: 'accepted',
    input: 'Continue after this home resumes',
  });
  expect(view.result.current.resourceAvailability.state).toBe('inactive');
  expect(view.result.current.progressObservation?.sandbox.status).toBe('reviving');
  view.unmount();
});
