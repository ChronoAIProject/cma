import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import type { AgentDetail, SandboxDetail } from '../api/types';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';

const starting: AgentDetail = {
  ...AGENT_DETAIL,
  status: 'starting',
  firstMessage: { state: 'queued', queuedAt: AGENT_DETAIL.createdAt },
};
const active: SandboxDetail = { ...sandboxFixture(), owner: 'owner' };
beforeEach(() => {
  vi.useFakeTimers();
  sessionStorage.clear();
});
afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
});
function retain(workspaceId?: string, sandboxId?: string) {
  sessionStorage.setItem(
    lifecycleStorageKey('owner', workspaceId, sandboxId),
    JSON.stringify({
      version: 1,
      intent: {
        key: 'same-key',
        body: { name: 'Created agent', workspace_id: workspaceId, sandbox_id: sandboxId },
        receipt: { id: 'operation-one' },
      },
    }),
  );
}
function clientFor(overrides: Partial<CmaWebClient> = {}) {
  return agentClient({
    createAgentLifecycle: vi.fn(),
    readAgentLifecycle: vi.fn(async () => ({
      id: 'operation-one',
      status: 'running',
      phase: 'agent_startup',
      agent_id: 'agt_1',
      events: [],
    })),
    readAgent: vi.fn(async () => starting),
    readSandbox: vi.fn(async () => active),
    ...overrides,
  });
}
function show(client: CmaWebClient, workspaceId?: string, sandboxId?: string) {
  retain(workspaceId, sandboxId);
  const onCreated = vi.fn();
  const props = { client, workspaceId, sandboxId, viewerId: 'owner', onClose: vi.fn(), onCreated };
  return { ...render(<AgentLifecycleCard {...props} />), props, onCreated };
}
const advance = async (ms = 0) => act(async () => vi.advanceTimersByTimeAsync(ms));

it('opens the real Starting Agent once before lifecycle success without recreating or claiming Ready', async () => {
  const client = clientFor();
  const { onCreated, unmount } = show(client, 'wsp_1', 'sbx_1');
  await advance();
  expect(onCreated).toHaveBeenCalledExactlyOnceWith({ id: 'agt_1', status: 'starting' });
  expect(client.readAgent).toHaveBeenCalledExactlyOnceWith('agt_1', expect.anything());
  expect(client.readSandbox).toHaveBeenCalledExactlyOnceWith('sbx_1', expect.anything());
  expect(
    JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_1', 'sbx_1'))!).intent,
  ).toMatchObject({ key: 'same-key', agentId: 'agt_1', receipt: { id: 'operation-one' } });
  unmount();
  await advance(60_000);
  expect(onCreated).toHaveBeenCalledOnce();
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(client.readAgentLifecycle).toHaveBeenCalledOnce();
});

it.each([
  ['Agent identity', { ...starting, id: 'other' }, active],
  ['Workspace scope', { ...starting, workspace: { ...starting.workspace, id: 'other' } }, active],
  ['Sandbox scope', { ...starting, sandbox: { ...starting.sandbox, id: 'other' } }, active],
  [
    'Worktree lineage',
    { ...starting, worktree: { ...starting.worktree, agentId: 'other' } },
    active,
  ],
  ['parent identity', starting, { ...active, id: 'other' }],
  ['parent owner', starting, { ...active, owner: 'another-owner' }],
  ['paused parent', starting, { ...active, status: 'paused' }],
  ['missing lifecycle', starting, { ...active, lifecycle: undefined }],
  [
    'active operation',
    starting,
    {
      ...active,
      lifecycle: {
        ...active.lifecycle!,
        operation: { purpose: 'pause', phase: 'pause_agents', startedAt: starting.createdAt },
      },
    },
  ],
] as const)('keeps the retained creation when %s is unverified', async (_name, agent, sandbox) => {
  const { onCreated } = show(
    clientFor({ readAgent: async () => agent, readSandbox: async () => sandbox }),
    'wsp_1',
    'sbx_1',
  );
  await advance(1000);
  expect(onCreated).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_1', 'sbx_1'))).toContain(
    'same-key',
  );
});

it.each(['viewer', 'client', 'workspace', 'sandbox', 'unmount'] as const)(
  'ignores a delayed Agent observation after a %s change',
  async (change) => {
    let resolve!: (value: AgentDetail) => void;
    const readAgent = vi.fn<CmaWebClient['readAgent']>(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const client = clientFor({ readAgent });
    const view = show(client);
    await advance();
    expect(readAgent).toHaveBeenCalledOnce();
    if (change === 'unmount') view.unmount();
    else
      view.rerender(
        <AgentLifecycleCard
          {...view.props}
          {...(change === 'viewer'
            ? { viewerId: 'other-owner' }
            : change === 'client'
              ? {
                  client: clientFor({
                    readAgent: async () => {
                      throw new Error('unavailable');
                    },
                  }),
                }
              : change === 'workspace'
                ? { workspaceId: 'other-workspace' }
                : { sandboxId: 'other-sandbox' })}
        />,
      );
    await act(async () => resolve(starting));
    await advance();
    expect(readAgent.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    expect(view.onCreated).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('same-key');
  },
);

it('bounds a hung Agent read, retries observation, and ignores its late result', async () => {
  let resolve!: (value: AgentDetail) => void;
  const readAgent = vi
    .fn<CmaWebClient['readAgent']>()
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    )
    .mockResolvedValue(starting);
  const { onCreated } = show(clientFor({ readAgent }));
  await advance();
  await advance(4999);
  expect(readAgent).toHaveBeenCalledOnce();
  expect(onCreated).not.toHaveBeenCalled();
  await advance(501);
  expect(readAgent.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
  expect(onCreated).toHaveBeenCalledExactlyOnceWith({ id: 'agt_1', status: 'starting' });
  await act(async () => resolve({ ...starting, status: 'ready' }));
  expect(onCreated).toHaveBeenCalledOnce();
});
