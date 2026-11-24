import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient, type AccessibleAgent } from '@cma/client';
import { agent as parseAgent } from './api-client/agents/types';
import { agentDetail } from './api/rest/hierarchy';
import { agentClient } from './test/agent';
import { WIRE_AGENT } from './test/rest';
import type { DeletionRequest } from './deletion';
import { forgetDeletion, persistDeletion, restoreDeletions } from './deletionRecovery';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';

afterEach(() => vi.restoreAllMocks());

const personal = parseAgent({
  ...WIRE_AGENT,
  id: 'agt_1',
  workspace: { ...WIRE_AGENT.workspace, id: 'wsp_1' },
  sandbox: { ...WIRE_AGENT.sandbox, id: 'sbx_1', status: 'retired' },
});
const shared: AccessibleAgent = {
  access: 'shared',
  id: 'agt_1',
  status: 'ready',
  created_at: personal.created_at,
  updated_at: personal.updated_at,
};
const request: DeletionRequest = {
  key: 'accepted-parent',
  accepted: true,
  action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
};

function fixture() {
  const fetch = vi.fn<typeof globalThis.fetch>(async () => Response.json(personal));
  const sdk = createClient({ baseUrl: 'https://cma.test', fetch });
  const readAccessible = vi.spyOn(sdk.agents, 'readAccessible');
  const readAgent = vi.fn(async () => agentDetail(personal));
  const source = agentClient({ resourceViews: sdk, readAgent });
  return { fetch, sdk, readAccessible, readAgent, source };
}

it('observes the existing personal access read, preserving its result/options and settling only its exact parent', async () => {
  const h = fixture();
  const sibling: DeletionRequest = {
    ...request,
    key: 'sibling',
    action: { kind: 'delete_sandbox', sandboxId: 'sbx_2' },
  };
  const unknown: DeletionRequest = {
    ...request,
    key: 'unknown-agent',
    accepted: false,
    action: { kind: 'delete_agent', agentId: 'agt_1' },
  };
  for (const item of [request, sibling, unknown]) persistDeletion('owner', item);
  persistDeletion('another-owner', request);
  const view = renderHook(() =>
    useDeletionNoticeReconciliation(h.source, 'owner', 7, true, 'agt_1'),
  );
  expect(h.fetch).not.toHaveBeenCalled();
  const options = { signal: new AbortController().signal, timeoutMs: 3000, requestId: 'fixture' };
  await act(async () => {
    const result = await view.result.current.client.resourceViews!.agents.readAccessible(
      'agt_1',
      options,
    );
    expect(result).toEqual(personal);
  });
  expect(view.result.current.selectedRetired).toEqual({
    detail: agentDetail(personal),
    viewerId: 'owner',
    authEpoch: 7,
  });
  expect(restoreDeletions('owner')).toEqual([sibling, unknown]);
  expect(restoreDeletions('another-owner')).toEqual([request]);
  expect(view.result.current.settlements?.requests).toEqual([request]);
  expect(h.readAccessible).toHaveBeenCalledExactlyOnceWith('agt_1', options);
  expect(h.readAccessible.mock.calls[0]?.[1]).toBe(options);
  expect(h.fetch).toHaveBeenCalledOnce();
  expect(h.readAgent).not.toHaveBeenCalled();
});

it('preserves SDK prototypes, private method receivers and stable resource identity across authority rechecks', async () => {
  const h = fixture();
  const view = renderHook(
    ({ epoch, authorized }) =>
      useDeletionNoticeReconciliation(h.source, 'owner', epoch, authorized, 'agt_1'),
    { initialProps: { epoch: 7, authorized: true } },
  );
  const wrapped = view.result.current.client.resourceViews as typeof h.sdk;
  expect(Object.getPrototypeOf(wrapped)).toBe(Object.getPrototypeOf(h.sdk));
  expect(Object.getPrototypeOf(wrapped.agents)).toBe(Object.getPrototypeOf(h.sdk.agents));
  expect(wrapped.resourceAccess).toBe(h.sdk.resourceAccess);
  expect(wrapped.sandboxes).toBe(h.sdk.sandboxes);
  expect(wrapped.workspaces).toBe(h.sdk.workspaces);
  expect(h.sdk.agents.readAccessible).toBe(h.readAccessible);
  const read = wrapped.agents.read;
  expect(wrapped.agents.read).toBe(read);
  expect(await wrapped.agents.read('agt_1')).toEqual(personal);
  const document = { openapi: '3.1.0', info: { title: 'Fixture', version: '1' }, paths: {} };
  h.fetch.mockResolvedValueOnce(Response.json(document));
  const readOpenApi = wrapped.readOpenApi;
  expect(await wrapped.readOpenApi()).toEqual(document);
  view.rerender({ epoch: 8, authorized: false });
  expect(view.result.current.client.resourceViews).toBe(wrapped);
  view.rerender({ epoch: 9, authorized: true });
  expect(view.result.current.client.resourceViews).toBe(wrapped);
  expect(wrapped.readOpenApi).toBe(readOpenApi);
});

it.each(['shared', 403, 404, 503] as const)(
  'clears the selected witness on a fresh %s access result without settling deletion',
  async (response) => {
    const h = fixture();
    const view = renderHook(() =>
      useDeletionNoticeReconciliation(h.source, 'owner', 7, true, 'agt_1'),
    );
    const read = () => view.result.current.client.resourceViews!.agents.readAccessible('agt_1');
    await act(async () => {
      await read();
    });
    expect(view.result.current.selectedRetired).toBeDefined();
    persistDeletion('owner', request);
    if (response === 'shared') h.readAccessible.mockResolvedValueOnce(shared);
    else h.readAccessible.mockRejectedValueOnce(new ApiError(response, 'Access unavailable'));
    await act(async () => {
      if (response === 'shared') expect(await read()).toBe(shared);
      else await expect(read()).rejects.toBeInstanceOf(ApiError);
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
    expect(restoreDeletions('owner')).toEqual([request]);
    expect(view.result.current.revision).toBe(0);
  },
);

it.each(['wrong-agent', 'retiring'] as const)('refuses a %s access witness', async (mode) => {
  const h = fixture();
  persistDeletion('owner', request);
  h.readAccessible.mockResolvedValueOnce({
    ...personal,
    id: mode === 'wrong-agent' ? 'agt_other' : personal.id,
    sandbox: { ...personal.sandbox, status: mode === 'retiring' ? 'retiring' : 'retired' },
  });
  const view = renderHook(() =>
    useDeletionNoticeReconciliation(h.source, 'owner', 7, true, 'agt_1'),
  );
  await act(async () => {
    await view.result.current.client.resourceViews!.agents.readAccessible('agt_1');
  });
  expect(view.result.current.selectedRetired).toBeUndefined();
  expect(restoreDeletions('owner')).toEqual([request]);
});

it('does not settle a different parent or select a different Agent from an access read', async () => {
  const h = fixture();
  persistDeletion('owner', request);
  h.readAccessible.mockResolvedValueOnce({
    ...personal,
    id: 'agt_other',
    sandbox: { ...personal.sandbox, id: 'sbx_other' },
  });
  const view = renderHook(() =>
    useDeletionNoticeReconciliation(h.source, 'owner', 7, true, 'agt_1'),
  );
  await act(async () => {
    await view.result.current.client.resourceViews!.agents.readAccessible('agt_other');
  });
  expect(view.result.current.selectedRetired).toBeUndefined();
  expect(restoreDeletions('owner')).toEqual([request]);
});

it.each(['owner', 'client', 'epoch', 'uncertain', 'route', 'unmount', 'abort'] as const)(
  'ignores a late personal access result across %s changes even if the reader ignores cancellation',
  async (mode) => {
    const h = fixture();
    persistDeletion('owner', request);
    let finish!: (value: AccessibleAgent) => void;
    h.readAccessible.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const initialProps = {
      source: h.source,
      owner: 'owner',
      epoch: 7,
      authorized: true,
      id: 'agt_1',
    };
    const view = renderHook(
      ({ source, owner, epoch, authorized, id }) =>
        useDeletionNoticeReconciliation(source, owner, epoch, authorized, id),
      { initialProps },
    );
    const abort = new AbortController();
    const read = view.result.current.client.resourceViews!.agents.readAccessible('agt_1', {
      signal: abort.signal,
    });
    if (mode === 'unmount') view.unmount();
    else if (mode === 'abort') abort.abort();
    else
      view.rerender({
        source: mode === 'client' ? agentClient() : h.source,
        owner: mode === 'owner' ? 'another-owner' : 'owner',
        epoch: mode === 'epoch' ? 8 : 7,
        authorized: mode !== 'uncertain',
        id: mode === 'route' ? 'agt_other' : 'agt_1',
      });
    await act(async () => {
      finish(personal);
      await read;
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
    expect(restoreDeletions('owner')).toEqual([request]);
    expect(h.readAccessible).toHaveBeenCalledOnce();
    expect(h.readAgent).not.toHaveBeenCalled();
  },
);

it.each(['unknown', 'ambiguous', 'replacement', 'late-acceptance'] as const)(
  'preserves %s responsibility despite current personal retirement evidence',
  async (mode) => {
    const h = fixture();
    persistDeletion('owner', {
      ...request,
      accepted: mode !== 'unknown' && mode !== 'late-acceptance',
    });
    if (mode === 'ambiguous') persistDeletion('owner', { ...request, key: 'second-key' });
    let finish!: (value: AccessibleAgent) => void;
    h.readAccessible.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const view = renderHook(() =>
      useDeletionNoticeReconciliation(h.source, 'owner', 7, true, 'agt_1'),
    );
    const read = view.result.current.client.resourceViews!.agents.readAccessible('agt_1');
    if (mode === 'replacement') {
      forgetDeletion('owner', request.key);
      persistDeletion('owner', { ...request, key: 'replacement' });
    } else if (mode === 'late-acceptance') persistDeletion('owner', request);
    const before = restoreDeletions('owner');
    await act(async () => {
      finish(personal);
      await read;
    });
    expect(view.result.current.selectedRetired?.detail).toEqual(agentDetail(personal));
    expect(restoreDeletions('owner')).toEqual(before);
    expect(view.result.current.revision).toBe(0);
  },
);

it.each([
  ['legacy', 'shared'],
  ['legacy', 'error'],
  ['accessible', 'shared'],
  ['accessible', 'error'],
] as const)(
  'does not let an older %s read override a newer %s access result',
  async (reader, result) => {
    const h = fixture();
    const view = renderHook(() =>
      useDeletionNoticeReconciliation(h.source, 'owner', 7, true, 'agt_1'),
    );
    const access = () => view.result.current.client.resourceViews!.agents.readAccessible('agt_1');
    await act(async () => {
      await access();
    });
    expect(view.result.current.selectedRetired).toBeDefined();
    persistDeletion('owner', request);
    let finish!: () => void;
    if (reader === 'legacy')
      h.readAgent.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = () => resolve(agentDetail(personal));
          }),
      );
    else
      h.readAccessible.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = () => resolve(personal);
          }),
      );
    const pending = reader === 'legacy' ? view.result.current.client.readAgent('agt_1') : access();
    await act(async () => {});
    if (result === 'shared') h.readAccessible.mockResolvedValueOnce(shared);
    else h.readAccessible.mockRejectedValueOnce(new ApiError(403, 'Access denied'));
    await act(async () => {
      if (result === 'shared') await access();
      else await expect(access()).rejects.toBeInstanceOf(ApiError);
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
    await act(async () => {
      finish();
      await pending;
    });
    expect(view.result.current.selectedRetired).toBeUndefined();
    expect(restoreDeletions('owner')).toEqual([request]);
    expect(view.result.current.revision).toBe(0);
  },
);
