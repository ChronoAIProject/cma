import { waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, TransportError } from '@chronoai/cma-sdk';
import type { AgentDetail, AgentUsage, WorkspaceDetail } from '../../api/types';
import { AGENT_DETAIL, AGENT_USAGE } from '../../test/agent';
import { workspaceFixture } from '../../test/hierarchy';
import { harness, item, json, page } from './controller.fixture';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { loadV2Agent } from './load';
import type { AgentLoadLineage } from '../loadAgent';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function setup(lineage?: AgentLoadLineage) {
  const h = harness();
  const v2 = await h.client.agentTransport!.open('agt_1');
  if (!v2) throw new Error('Expected v2 fixture');
  const progress = vi.fn();
  const abort = new AbortController();
  const start = (allowIncomplete = false) =>
    loadV2Agent(h.client, 'agt_1', abort.signal, v2, allowIncomplete, progress, lineage).then(
      (value) => ({ value, error: undefined }),
      (error: unknown) => ({ value: undefined, error }),
    );
  return { ...h, v2, progress, abort, start };
}

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const lineage: AgentLoadLineage = {
  agentId: AGENT_DETAIL.id,
  workspaceId: AGENT_DETAIL.workspace.id,
  sandboxId: AGENT_DETAIL.sandbox.id,
};

it('reads a hinted Workspace with fresh Agent metadata after entry authorization and preserves final authorization', async () => {
  const h = await setup(lineage);
  const entry = deferred<string | undefined>();
  const final = deferred<string | undefined>();
  h.currentUser.mockClear().mockReturnValueOnce(entry.promise).mockReturnValueOnce(final.promise);
  const detail = deferred<AgentDetail>();
  const workspace = deferred<WorkspaceDetail>();
  const detailRead = vi.spyOn(h.client, 'readAgent').mockReturnValue(detail.promise);
  const workspaceRead = vi.spyOn(h.client, 'readWorkspace').mockReturnValue(workspace.promise);
  const settled = vi.fn();
  const result = h.start().then((value) => {
    settled();
    return value;
  });
  expect(detailRead).not.toHaveBeenCalled();
  expect(workspaceRead).not.toHaveBeenCalled();
  entry.resolve('alice');
  await waitFor(() => expect(workspaceRead).toHaveBeenCalledOnce());
  expect(detailRead).toHaveBeenCalledOnce();
  expect(workspaceRead).toHaveBeenCalledWith(lineage.workspaceId, { signal: h.abort.signal });
  expect(h.snapshot).toHaveBeenCalledOnce();
  workspace.resolve(workspaceFixture());
  await Promise.resolve();
  expect(h.currentUser).toHaveBeenCalledOnce();
  expect(settled).not.toHaveBeenCalled();
  detail.resolve(AGENT_DETAIL);
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  expect(settled).not.toHaveBeenCalled();
  expect(h.progress).not.toHaveBeenCalled();
  final.resolve('alice');
  expect(await result).toMatchObject({ value: { detail: AGENT_DETAIL }, error: undefined });
});

it.each(['agentId', 'workspaceId', 'sandboxId'] as const)(
  'rejects a hinted %s mismatch without publishing projections',
  async (field) => {
    const h = await setup({ ...lineage, [field]: 'different-resource' });
    expect(await h.start()).toMatchObject({
      error: { message: 'Agent projections changed identity.' },
    });
    expect(h.progress).not.toHaveBeenCalled();
    expect(h.post).not.toHaveBeenCalled();
  },
);

it.each(['bob', undefined])(
  'rejects an account change to %s after the hinted Workspace resolves before Agent metadata',
  async (owner) => {
    const h = await setup(lineage);
    const detail = deferred<AgentDetail>();
    vi.spyOn(h.client, 'readAgent').mockReturnValue(detail.promise);
    const workspace = vi.spyOn(h.client, 'readWorkspace');
    const result = h.start();
    await waitFor(() => expect(workspace).toHaveBeenCalledOnce());
    h.currentUser.mockResolvedValue(owner);
    detail.resolve(AGENT_DETAIL);
    expect(await result).toMatchObject({ error: { name: 'IdentityChangedError' } });
    expect(h.progress).not.toHaveBeenCalled();
  },
);

it('consumes a late parallel Workspace rejection after Agent metadata has already failed', async () => {
  const h = await setup(lineage);
  const detail = deferred<AgentDetail>();
  const workspace = deferred<WorkspaceDetail>();
  vi.spyOn(h.client, 'readAgent').mockReturnValue(detail.promise);
  vi.spyOn(h.client, 'readWorkspace').mockReturnValue(workspace.promise);
  const result = h.start();
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  detail.reject(new Error('Agent refused'));
  expect(await result).toMatchObject({ error: { message: 'Agent refused' } });
  workspace.reject(new Error('Later Workspace refusal'));
  await Promise.allSettled([detail.promise, workspace.promise]);
  expect(h.progress).not.toHaveBeenCalled();
});

it('overlaps equally slow Agent and Workspace reads without shortening either identity check', async () => {
  const h = await setup(lineage);
  vi.useFakeTimers();
  const delay = <Value>(ms: number, value: Value) =>
    new Promise<Value>((resolve) => setTimeout(() => resolve(value), ms));
  h.currentUser.mockClear().mockImplementation(() => delay(250, 'alice'));
  vi.spyOn(h.client, 'readAgent').mockImplementation(() => delay(300, AGENT_DETAIL));
  const workspace = vi
    .spyOn(h.client, 'readWorkspace')
    .mockImplementation(() => delay(300, workspaceFixture()));
  h.snapshot.mockImplementation(() => delay(100, json(page([], 0))));
  const settled = vi.fn();
  const result = h.start().then((value) => {
    settled();
    return value;
  });
  await vi.advanceTimersByTimeAsync(249);
  expect(workspace).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(workspace).toHaveBeenCalledOnce();
  expect(h.currentUser).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(300);
  expect(h.currentUser).toHaveBeenCalledTimes(2);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(249);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(await result).toMatchObject({ value: { detail: AGENT_DETAIL }, error: undefined });
});

it('overlaps the first snapshot and usage with hierarchy reads, then waits for fresh account authority', async () => {
  const h = await setup();
  h.currentUser.mockClear();
  const detail = deferred<AgentDetail>();
  const usage = deferred<AgentUsage>();
  const workspace = deferred<WorkspaceDetail>();
  vi.spyOn(h.client, 'readAgent').mockReturnValue(detail.promise);
  const usageRead = vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
  const workspaceRead = vi.spyOn(h.client, 'readWorkspace').mockReturnValue(workspace.promise);
  const settled = vi.fn();
  const result = h.start().then((value) => {
    settled();
    return value;
  });
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  expect(h.currentUser).toHaveBeenCalledOnce();
  expect(usageRead).toHaveBeenCalledOnce();
  expect(workspaceRead).not.toHaveBeenCalled();
  expect(settled).not.toHaveBeenCalled();
  detail.resolve(AGENT_DETAIL);
  await waitFor(() => expect(workspaceRead).toHaveBeenCalledOnce());
  expect(settled).not.toHaveBeenCalled();
  const identity = deferred<string | undefined>();
  const finalCheck = vi.fn(() => identity.promise);
  h.currentUser.mockImplementationOnce(finalCheck);
  workspace.resolve(workspaceFixture());
  usage.resolve(AGENT_USAGE);
  await waitFor(() => expect(finalCheck).toHaveBeenCalledOnce());
  expect(settled).not.toHaveBeenCalled();
  expect(h.progress).not.toHaveBeenCalled();
  identity.resolve('alice');
  expect(await result).toMatchObject({
    value: { detail: AGENT_DETAIL, workspace: workspaceFixture(), historyUnavailable: false },
    error: undefined,
  });
  expect(h.post).not.toHaveBeenCalled();
});

it('starts the first snapshot with the authorized batch and still waits for a new final account read', async () => {
  const h = await setup();
  vi.useFakeTimers();
  const delay = <Value>(ms: number, value: Value) =>
    new Promise<Value>((resolve) => setTimeout(() => resolve(value), ms));
  h.currentUser.mockClear().mockImplementation(() => delay(250, 'alice'));
  vi.spyOn(h.client, 'readAgent').mockImplementation(() => delay(300, AGENT_DETAIL));
  vi.spyOn(h.client, 'readAgentUsage').mockImplementation(() => delay(300, AGENT_USAGE));
  vi.spyOn(h.client, 'readWorkspace').mockImplementation(() => delay(300, workspaceFixture()));
  h.snapshot.mockImplementation(() => delay(600, json(page([], 0))));
  const settled = vi.fn();
  const result = h.start().then((value) => {
    settled();
    return value;
  });
  await vi.advanceTimersByTimeAsync(249);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.client.readAgent).not.toHaveBeenCalled();
  expect(h.client.readAgentUsage).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.currentUser).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(600);
  expect(h.currentUser).toHaveBeenCalledTimes(2);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(249);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(await result).toMatchObject({ value: { detail: AGENT_DETAIL }, error: undefined });
  expect(h.progress).not.toHaveBeenCalled();
});

it.each(['cancel', 'new epoch'] as const)(
  'refuses the first snapshot if a synchronous projection reader causes %s',
  async (change) => {
    const h = await setup();
    vi.spyOn(h.client, 'readAgentUsage').mockImplementation(() => {
      if (change === 'cancel') h.abort.abort();
      else {
        clearAgentTransport(sessionStorage);
        agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
      }
      return Promise.resolve(AGENT_USAGE);
    });
    expect(await h.start()).toMatchObject({
      value: undefined,
      error: { name: change === 'cancel' ? 'AbortError' : 'IdentityChangedError' },
    });
    expect(h.snapshot).not.toHaveBeenCalled();
    expect(h.progress).not.toHaveBeenCalled();
  },
);

it.each(['bob', undefined])(
  'refuses the next snapshot when a progress callback changes the account to %s',
  async (owner) => {
    const h = await setup();
    h.snapshot.mockResolvedValueOnce(
      json(page([item(1, { type: 'turn_started', turn_sequence: 1 })], 2, 1)),
    );
    h.progress.mockImplementation(() => h.currentUser.mockResolvedValue(owner));
    expect(await h.start()).toMatchObject({
      value: undefined,
      error: { name: 'IdentityChangedError' },
    });
    expect(h.snapshot).toHaveBeenCalledOnce();
    expect(h.progress).toHaveBeenCalledOnce();
    expect(h.progress.mock.calls[0]![0].conversation.messages).toEqual([]);
  },
);

it.each([
  ['account switch', 'bob'],
  ['expired sign-in', undefined],
] as const)(
  'rejects an %s after an early snapshot while required metadata is still pending',
  async (_name, user) => {
    const h = await setup();
    const usage = deferred<AgentUsage>();
    vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
    h.snapshot.mockResolvedValueOnce(
      json(
        page([item(1, { type: 'message', role: 'assistant', content: 'Private history' })], 2, 1),
      ),
    );
    const result = h.start();
    await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
    h.currentUser.mockResolvedValue(user);
    usage.resolve(AGENT_USAGE);
    expect(await result).toMatchObject({ error: { name: 'IdentityChangedError' } });
    expect(h.progress).not.toHaveBeenCalled();
    expect(h.snapshot).toHaveBeenCalledOnce();
    expect(h.post).not.toHaveBeenCalled();
  },
);

it('rejects an account switch while the snapshot is the last outstanding read', async () => {
  const h = await setup();
  const snapshot = deferred<Response>();
  h.snapshot.mockReturnValueOnce(snapshot.promise);
  const workspaceRead = vi.spyOn(h.client, 'readWorkspace');
  const result = h.start();
  await waitFor(() => expect(workspaceRead).toHaveBeenCalledOnce());
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  h.currentUser.mockResolvedValue('bob');
  snapshot.resolve(json(page([], 0)));
  expect(await result).toMatchObject({ error: { name: 'IdentityChangedError' } });
  expect(h.progress).not.toHaveBeenCalled();
});

it('refuses all projection reads when the initial account check fails', async () => {
  const h = await setup();
  const detailRead = vi.spyOn(h.client, 'readAgent');
  const usageRead = vi.spyOn(h.client, 'readAgentUsage');
  h.currentUser.mockResolvedValue(undefined);
  expect(await h.start()).toMatchObject({ error: { name: 'IdentityChangedError' } });
  expect(detailRead).not.toHaveBeenCalled();
  expect(usageRead).not.toHaveBeenCalled();
  expect(h.snapshot).not.toHaveBeenCalled();
});

it.each(['detail', 'usage', 'workspace'] as const)(
  'never treats a required %s failure as optional incomplete history',
  async (branch) => {
    const h = await setup();
    const error = new ApiError(503, 'Projection unavailable');
    vi.spyOn(
      h.client,
      branch === 'detail' ? 'readAgent' : branch === 'usage' ? 'readAgentUsage' : 'readWorkspace',
    ).mockRejectedValue(error);
    expect(await h.start(true)).toMatchObject({ value: undefined, error });
    expect(h.progress).not.toHaveBeenCalled();
  },
);

it.each(['detail', 'usage', 'workspace', 'parent'] as const)(
  'fails closed when the %s projection identity differs',
  async (branch) => {
    const h = await setup();
    if (branch === 'detail')
      vi.spyOn(h.client, 'readAgent').mockResolvedValue({ ...AGENT_DETAIL, id: 'agt_other' });
    else if (branch === 'usage')
      vi.spyOn(h.client, 'readAgentUsage').mockResolvedValue({
        ...AGENT_USAGE,
        agentId: 'agt_other',
      });
    else
      vi.spyOn(h.client, 'readWorkspace').mockResolvedValue({
        ...workspaceFixture(),
        ...(branch === 'workspace' ? { id: 'wsp_other' } : { sandboxId: 'sbx_other' }),
      });
    expect(await h.start(true)).toMatchObject({
      value: undefined,
      error: { message: 'Agent projections changed identity.' },
    });
    expect(h.progress).not.toHaveBeenCalled();
  },
);

it.each([401, 403, 404])(
  'does not mask a snapshot refusal %s as partial history',
  async (status) => {
    const h = await setup();
    h.snapshot.mockResolvedValueOnce(
      json(
        {
          error: {
            type: 'invalid_request_error',
            code: 'refused',
            message: 'Refused',
            param: null,
          },
        },
        status,
      ),
    );
    expect(await h.start(true)).toMatchObject({ value: undefined, error: { status } });
    expect(h.progress).not.toHaveBeenCalled();
  },
);

it('rejects a malformed snapshot identity even when incomplete history is allowed', async () => {
  const h = await setup();
  h.snapshot.mockResolvedValueOnce(json({ ...page([], 0), agent_id: 'agt_other' }));
  expect(await h.start(true)).toMatchObject({ value: undefined, error: { name: 'ProtocolError' } });
  expect(h.progress).not.toHaveBeenCalled();
});

it('allows only a transient snapshot failure to produce an authorized incomplete view', async () => {
  const h = await setup();
  h.snapshot.mockRejectedValueOnce(new TransportError('History temporarily unavailable'));
  const usage = deferred<AgentUsage>();
  vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
  const result = h.start(true);
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  const finalCheck = vi.fn(async () => 'alice');
  h.currentUser.mockImplementationOnce(finalCheck);
  usage.resolve(AGENT_USAGE);
  expect(await result).toMatchObject({
    value: { historyUnavailable: true, historyLoading: false },
    error: undefined,
  });
  expect(finalCheck).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
});

it('still checks expiry after a transient snapshot failure and the final metadata result', async () => {
  const h = await setup();
  h.snapshot.mockRejectedValueOnce(new TransportError('History temporarily unavailable'));
  const usage = deferred<AgentUsage>();
  vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
  const result = h.start(true);
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  h.currentUser.mockResolvedValue(undefined);
  usage.resolve(AGENT_USAGE);
  expect(await result).toMatchObject({ value: undefined, error: { name: 'IdentityChangedError' } });
  expect(h.progress).not.toHaveBeenCalled();
});

it.each(['projection first', 'snapshot first'] as const)(
  'consumes every concurrent rejection when %s fails',
  async (order) => {
    const h = await setup();
    const detail = deferred<AgentDetail>();
    const usage = deferred<AgentUsage>();
    const snapshot = deferred<Response>();
    vi.spyOn(h.client, 'readAgent').mockReturnValue(detail.promise);
    vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
    h.snapshot.mockReturnValueOnce(snapshot.promise);
    const result = h.start();
    await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
    const first = new ApiError(403, 'First refusal');
    if (order === 'projection first') detail.reject(first);
    else snapshot.reject(first);
    expect(await result).toMatchObject({ value: undefined, error: first });
    usage.reject(new Error('Later usage failure'));
    if (order === 'projection first') snapshot.reject(new Error('Later snapshot failure'));
    else detail.reject(new Error('Later hierarchy failure'));
    // Vitest reports any unhandled rejection, including after the first refusal.
    await Promise.allSettled([detail.promise, usage.promise, snapshot.promise]);
    expect(h.progress).not.toHaveBeenCalled();
  },
);

it('contains synchronous reader failure while safely consuming the other branches', async () => {
  const h = await setup();
  const usage = deferred<AgentUsage>();
  const snapshot = deferred<Response>();
  const error = new Error('Synchronous detail refusal');
  vi.spyOn(h.client, 'readAgent').mockImplementation(() => {
    throw error;
  });
  vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
  h.snapshot.mockReturnValueOnce(snapshot.promise);
  expect(await h.start()).toMatchObject({ value: undefined, error });
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  usage.reject(new Error('Later usage failure'));
  snapshot.reject(new Error('Later snapshot failure'));
  await Promise.allSettled([usage.promise, snapshot.promise]);
});

it('does not read the dependent Workspace or publish progress after cancellation', async () => {
  const h = await setup();
  const detail = deferred<AgentDetail>();
  vi.spyOn(h.client, 'readAgent').mockReturnValue(detail.promise);
  const workspaceRead = vi.spyOn(h.client, 'readWorkspace');
  const result = h.start();
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  h.abort.abort();
  detail.resolve(AGENT_DETAIL);
  const outcome = await result;
  expect(outcome.value).toBeUndefined();
  expect(outcome.error).toBeInstanceOf(Error);
  expect(workspaceRead).not.toHaveBeenCalled();
  expect(h.progress).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
});

it('refuses a completed read when cancellation arrives during the final account check', async () => {
  const h = await setup();
  const usage = deferred<AgentUsage>();
  vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(usage.promise);
  const result = h.start();
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  const identity = deferred<string | undefined>();
  const finalCheck = vi.fn(() => identity.promise);
  h.currentUser.mockImplementationOnce(finalCheck);
  usage.resolve(AGENT_USAGE);
  await waitFor(() => expect(finalCheck).toHaveBeenCalledOnce());
  h.abort.abort();
  identity.resolve('alice');
  const outcome = await result;
  expect(outcome.value).toBeUndefined();
  expect(outcome.error).toBeInstanceOf(Error);
  expect(h.progress).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
});
