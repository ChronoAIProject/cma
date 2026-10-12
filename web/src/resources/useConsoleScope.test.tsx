import { act, renderHook, waitFor } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import type { AppRoute } from '../routing/routes';
import { useConsoleScope } from './useConsoleScope';

function fixture() {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    expect(new URL(String(input)).pathname).toContain('/resource-access/');
    return Response.json({
      owner: { kind: 'organization', organization_id: 'org_research' },
      capabilities: ['view_details'],
      policy_revision: 1,
      revalidate_at: null,
    });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  return { fetch, client };
}

it.each([
  { kind: 'agent', agentId: 'agt_org' },
  { kind: 'workspace', workspaceId: 'wks_org' },
  { kind: 'sandbox', sandboxId: 'sbx_org' },
] as const)(
  'restores organization scope from authorized $kind deep links without parent reads',
  async (route) => {
    const test = fixture();
    const first = renderHook(() => useConsoleScope(test.client, 'person', route));
    expect(first.result.current.ready).toBe(false);
    await waitFor(() => expect(first.result.current.value).toBe('organization:org_research'));
    expect(first.result.current.ready).toBe(true);
    first.unmount();
    const reloaded = renderHook(() => useConsoleScope(test.client, 'person', route));
    await waitFor(() => expect(reloaded.result.current.value).toBe('organization:org_research'));
    expect(test.fetch).toHaveBeenCalledTimes(2);
    expect(test.fetch.mock.calls.every(([url]) => String(url).includes('/resource-access/'))).toBe(
      true,
    );
  },
);

it('respects an explicit sidebar choice when a pending resource authorization completes', async () => {
  const test = fixture();
  let finish: ((value: Response) => void) | undefined;
  test.fetch.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const hook = renderHook(() =>
    useConsoleScope(test.client, 'person', { kind: 'agent', agentId: 'agt_org' }),
  );
  act(() => hook.result.current.select(''));
  await act(async () => {
    finish?.(
      Response.json({
        owner: { kind: 'organization', organization_id: 'org_research' },
        capabilities: ['view_details'],
        policy_revision: 1,
        revalidate_at: null,
      }),
    );
  });
  expect(hook.result.current.value).toBe('');
  expect(hook.result.current.ready).toBe(true);
});

it('keeps an explicit choice on the current conversation and resolves a newly opened resource', async () => {
  const test = fixture();
  const hook = renderHook(
    ({ route }: { route: AppRoute }) => useConsoleScope(test.client, 'person', route),
    {
      initialProps: { route: { kind: 'agent', agentId: 'agt_org' } as AppRoute },
    },
  );
  await waitFor(() => expect(hook.result.current.value).toBe('organization:org_research'));
  act(() => hook.result.current.select('shared'));
  hook.rerender({ route: { kind: 'agent', agentId: 'agt_org' } });
  expect(hook.result.current.value).toBe('shared');
  hook.rerender({ route: { kind: 'workspace', workspaceId: 'wks_org' } });
  await waitFor(() => expect(hook.result.current.value).toBe('organization:org_research'));
});

it('does not infer scope or allow creation from denied owner metadata', async () => {
  const test = fixture();
  test.fetch.mockResolvedValue(
    Response.json({
      owner: { kind: 'organization', organization_id: 'private_org' },
      capabilities: [],
      policy_revision: 1,
      revalidate_at: null,
    }),
  );
  const hook = renderHook(() =>
    useConsoleScope(test.client, 'person', { kind: 'agent', agentId: 'agt_private' }),
  );
  await waitFor(() => expect(test.fetch).toHaveBeenCalledOnce());
  expect(hook.result.current.value).toBe('');
  expect(hook.result.current.ready).toBe(false);
});

it('selects incoming personal shares from current ownership and clears scope across viewers', async () => {
  const test = fixture();
  test.fetch.mockResolvedValue(
    Response.json({
      owner: { kind: 'person', is_current_person: false },
      capabilities: ['view_details'],
      policy_revision: 1,
      revalidate_at: null,
    }),
  );
  const hook = renderHook(
    ({ viewer, route }: { viewer: string; route: AppRoute }) =>
      useConsoleScope(test.client, viewer, route),
    {
      initialProps: {
        viewer: 'person',
        route: { kind: 'agent', agentId: 'agt_shared' } as AppRoute,
      },
    },
  );
  await waitFor(() => expect(hook.result.current.value).toBe('shared'));
  hook.rerender({ viewer: 'another-person', route: { kind: 'home' } });
  expect(hook.result.current.value).toBe('');
});
