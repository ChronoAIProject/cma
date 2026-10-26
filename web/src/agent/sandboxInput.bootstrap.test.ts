import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { AgentDetail, SandboxDetail } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { BOOTSTRAP_AGENT, bootstrapSandbox } from '../test/bootstrap';
import { useSandboxInput } from './sandboxInput';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
}
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each(['provisioning', 'installing'] as const)(
  'permits only v2 bootstrap input from fresh exact Agent and %s parent evidence',
  async (status) => {
    const parent = deferred<SandboxDetail>();
    const detail = deferred<AgentDetail>();
    const readAgent = vi.fn(() => detail.promise);
    const readSandbox = vi.fn(() => parent.promise);
    const client = agentClient({ readAgent, readSandbox });
    const view = renderHook(() =>
      useSandboxInput(client, 'agt_1', 'sbx_1', true, BOOTSTRAP_AGENT.bootstrap, 'alice'),
    );
    expect(readAgent).toHaveBeenCalledOnce();
    expect(readSandbox).toHaveBeenCalledOnce();
    expect(view.result.current.inputAvailability.allowed).toBe(false);
    await act(async () => detail.resolve(BOOTSTRAP_AGENT));
    expect(view.result.current.inputAvailability.allowed).toBe(false);
    await act(async () => parent.resolve(bootstrapSandbox(status)));
    expect(view.result.current.inputAvailability).toEqual({ allowed: true });
    expect(view.result.current.resourceAvailability.state).toBe('inactive');
    expect(view.result.current.progressObservation).toMatchObject({
      sandbox: { status },
      stale: false,
    });
  },
);

it.each([
  [
    'missing current bootstrap',
    { ...BOOTSTRAP_AGENT, bootstrap: undefined },
    bootstrapSandbox(),
    'alice',
  ],
  [
    'other operation',
    {
      ...BOOTSTRAP_AGENT,
      bootstrap: { ...BOOTSTRAP_AGENT.bootstrap!, operationId: 'another-operation' },
    },
    bootstrapSandbox(),
    'alice',
  ],
  [
    'other bootstrap parent',
    {
      ...BOOTSTRAP_AGENT,
      bootstrap: { ...BOOTSTRAP_AGENT.bootstrap!, sandboxId: 'another-sandbox' },
    },
    bootstrapSandbox(),
    'alice',
  ],
  [
    'other bootstrap workspace',
    {
      ...BOOTSTRAP_AGENT,
      bootstrap: { ...BOOTSTRAP_AGENT.bootstrap!, workspaceId: 'another-workspace' },
    },
    bootstrapSandbox(),
    'alice',
  ],
  ['other Agent', { ...BOOTSTRAP_AGENT, id: 'another-agent' }, bootstrapSandbox(), 'alice'],
  [
    'other Worktree Agent',
    { ...BOOTSTRAP_AGENT, worktree: { ...BOOTSTRAP_AGENT.worktree, agentId: 'another-agent' } },
    bootstrapSandbox(),
    'alice',
  ],
  [
    'other Worktree parent',
    { ...BOOTSTRAP_AGENT, worktree: { ...BOOTSTRAP_AGENT.worktree, sandboxId: 'another-sandbox' } },
    bootstrapSandbox(),
    'alice',
  ],
  [
    'other Worktree workspace',
    {
      ...BOOTSTRAP_AGENT,
      worktree: { ...BOOTSTRAP_AGENT.worktree, workspaceId: 'another-workspace' },
    },
    bootstrapSandbox(),
    'alice',
  ],
  ['wrong parent owner', BOOTSTRAP_AGENT, { ...bootstrapSandbox(), owner: 'bob' }, 'alice'],
  ['unknown parent owner', BOOTSTRAP_AGENT, { ...bootstrapSandbox(), owner: undefined }, 'alice'],
  ['unknown viewer', BOOTSTRAP_AGENT, bootstrapSandbox(), undefined],
  ['missing lifecycle', BOOTSTRAP_AGENT, { ...bootstrapSandbox(), lifecycle: undefined }, 'alice'],
  [
    'unrelated parent transition',
    BOOTSTRAP_AGENT,
    { ...bootstrapSandbox(), status: 'reviving' },
    'alice',
  ],
  [
    'Agent no longer Starting',
    { ...BOOTSTRAP_AGENT, status: 'ready' },
    bootstrapSandbox(),
    'alice',
  ],
  [
    'parent lifecycle operation',
    BOOTSTRAP_AGENT,
    {
      ...bootstrapSandbox(),
      lifecycle: {
        ...bootstrapSandbox().lifecycle!,
        operation: {
          purpose: 'pause',
          phase: 'pause_agents',
          startedAt: BOOTSTRAP_AGENT.createdAt,
        },
      },
    },
    'alice',
  ],
  [
    'parent checkpoint',
    BOOTSTRAP_AGENT,
    {
      ...bootstrapSandbox(),
      lifecycle: {
        ...bootstrapSandbox().lifecycle!,
        checkpoint: { phase: 'capture', epoch: 1, startedAt: BOOTSTRAP_AGENT.createdAt },
      },
    },
    'alice',
  ],
  [
    'checkpoint refusal',
    BOOTSTRAP_AGENT,
    {
      ...bootstrapSandbox(),
      lifecycle: {
        ...bootstrapSandbox().lifecycle!,
        pause: { allowed: false, reason: 'checkpoint_in_progress' },
      },
    },
    'alice',
  ],
] as const)('holds bootstrap input for %s', async (_reason, detail, parent, owner) => {
  const client = agentClient({ readAgent: async () => detail, readSandbox: async () => parent });
  const view = renderHook(() =>
    useSandboxInput(client, 'agt_1', 'sbx_1', true, BOOTSTRAP_AGENT.bootstrap, owner),
  );
  await act(async () => undefined);
  expect(view.result.current.inputAvailability.allowed).toBe(false);
});

it.each([false, true])(
  'holds generic Starting Agents without bootstrap evidence (v2=%s)',
  async (queue) => {
    const readAgent = vi.fn(async () => BOOTSTRAP_AGENT);
    const client = agentClient({ readAgent, readSandbox: async () => bootstrapSandbox() });
    const view = renderHook(() =>
      useSandboxInput(
        client,
        'agt_1',
        'sbx_1',
        queue,
        queue ? undefined : BOOTSTRAP_AGENT.bootstrap,
        'alice',
      ),
    );
    await act(async () => undefined);
    expect(view.result.current.inputAvailability.allowed).toBe(false);
    expect(readAgent).not.toHaveBeenCalled();
  },
);

it('requires fresh bootstrap proof on refresh and expires both evidence reads together', async () => {
  vi.useFakeTimers();
  const readAgent = vi
    .fn()
    .mockResolvedValueOnce(BOOTSTRAP_AGENT)
    .mockResolvedValue({ ...BOOTSTRAP_AGENT, bootstrap: undefined });
  const client = agentClient({ readAgent, readSandbox: async () => bootstrapSandbox() });
  const view = renderHook(() =>
    useSandboxInput(client, 'agt_1', 'sbx_1', true, BOOTSTRAP_AGENT.bootstrap, 'alice'),
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  readAgent.mockImplementation(() => new Promise(() => {}));
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  expect(view.result.current.progressObservation?.stale).toBe(true);
});

it.each([401, 403, 404])(
  'clears bootstrap authority when the fresh Agent read is refused with %s',
  async (status) => {
    const client = agentClient({
      readAgent: async () => {
        throw new ApiError(status, 'Refused');
      },
      readSandbox: async () => bootstrapSandbox(),
    });
    const view = renderHook(() =>
      useSandboxInput(client, 'agt_1', 'sbx_1', true, BOOTSTRAP_AGENT.bootstrap, 'alice'),
    );
    await act(async () => undefined);
    expect(view.result.current.inputAvailability.allowed).toBe(false);
    expect(view.result.current.progressObservation).toBeUndefined();
  },
);

it('switches to normal Active eligibility when bootstrap evidence disappears after provisioning', async () => {
  vi.useFakeTimers();
  const readAgent = vi
    .fn()
    .mockResolvedValueOnce(BOOTSTRAP_AGENT)
    .mockResolvedValue({ ...BOOTSTRAP_AGENT, bootstrap: undefined });
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(bootstrapSandbox())
    .mockResolvedValue({ ...sandboxFixture(), owner: 'alice' });
  const client = agentClient({ readAgent, readSandbox });
  const view = renderHook(() =>
    useSandboxInput(client, 'agt_1', 'sbx_1', true, BOOTSTRAP_AGENT.bootstrap, 'alice'),
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  expect(view.result.current.resourceAvailability.state).toBe('live');
});

it.each(['owner', 'client', 'operation', 'Agent', 'unmount'] as const)(
  'cancels bootstrap reads and ignores stale results after a %s change',
  async (change) => {
    const old = deferred<AgentDetail>();
    const readAgent = vi.fn<CmaWebClient['readAgent']>(() => old.promise);
    const client = agentClient({ readAgent, readSandbox: async () => bootstrapSandbox() });
    const original = {
      client,
      owner: 'alice',
      agentId: 'agt_1',
      bootstrap: BOOTSTRAP_AGENT.bootstrap,
    };
    const view = renderHook(
      ({ client, owner, agentId, bootstrap }) =>
        useSandboxInput(client, agentId, 'sbx_1', true, bootstrap, owner),
      { initialProps: original },
    );
    if (change === 'unmount') view.unmount();
    else
      view.rerender({
        ...original,
        ...(change === 'owner'
          ? { owner: 'bob' }
          : change === 'client'
            ? {
                client: agentClient({
                  readAgent: () => new Promise(() => {}),
                  readSandbox: async () => bootstrapSandbox(),
                }),
              }
            : change === 'Agent'
              ? { agentId: 'another-agent' }
              : { bootstrap: { ...BOOTSTRAP_AGENT.bootstrap!, operationId: 'another-operation' } }),
      });
    expect(readAgent.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    await act(async () => old.resolve(BOOTSTRAP_AGENT));
    expect(view.result.current.inputAvailability.allowed).toBe(false);
  },
);

it('does not expose a stale allowed result when the client changes after successful bootstrap verification', async () => {
  const client = agentClient({
    readAgent: async () => BOOTSTRAP_AGENT,
    readSandbox: async () => bootstrapSandbox(),
  });
  const view = renderHook(
    ({ current }) =>
      useSandboxInput(current, 'agt_1', 'sbx_1', true, BOOTSTRAP_AGENT.bootstrap, 'alice'),
    { initialProps: { current: client } },
  );
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  view.rerender({ current: agentClient({ readSandbox: () => new Promise(() => {}) }) });
  expect(view.result.current.inputAvailability.allowed).toBe(false);
});
