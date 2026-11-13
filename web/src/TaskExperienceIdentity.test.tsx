import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { AuthContextProvider, useAuth } from './auth/context';
import type { AuthProvider } from './auth/types';
import { unavailableClient, type CmaWebClient } from './api/client';
import { useTaskExperience } from './taskExperience';
import { useEffect } from 'react';

function Probe({ client }: { client: CmaWebClient }) {
  const { state } = useAuth();
  const account = state.status === 'authenticated' ? state.user.id : undefined;
  const policy = useTaskExperience(client, account);
  return (
    <p>
      {account ?? 'unknown'}:{policy.task_home_admission ? 'enabled' : 'disabled'}
    </p>
  );
}
afterEach(() => sessionStorage.clear());

it('refreshes current identity on focus and fences policy to that exact account', async () => {
  let account = 'owner-a';
  const provider: AuthProvider = {
    current: async () => ({
      status: 'authenticated',
      user: { id: account, handle: account, visitor: false, capabilities: ['use_own_sandboxes'] },
    }),
    signIn: async () => {},
    signOut: async () => {},
  };
  const sdk = createClient({
    fetch: async () =>
      Response.json({
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: account,
        capabilities: {
          task_home_admission: true,
          suggested_followup_admission: false,
          desktop_enable: false,
          browser_takeover: false,
          replay_presentation: false,
        },
      }),
  });
  const client = { ...unavailableClient, taskExperience: sdk.taskExperience };
  render(
    <AuthContextProvider provider={provider}>
      <Probe client={client} />
    </AuthContextProvider>,
  );
  await screen.findByText('owner-a:enabled');
  account = 'owner-b';
  fireEvent(window, new Event('focus'));
  await screen.findByText('owner-b:enabled');
  await act(async () => {});
  await waitFor(() => expect(screen.queryByText('owner-a:enabled')).not.toBeInTheDocument());
});

it('keeps policy-dependent effects stable across unrelated renders and pending same-scope refreshes', async () => {
  let settle!: (
    value: Awaited<ReturnType<NonNullable<CmaWebClient['taskExperience']>['read']>>,
  ) => void;
  const read = vi.fn(
    () =>
      new Promise<Awaited<ReturnType<NonNullable<CmaWebClient['taskExperience']>['read']>>>(
        (resolve) => {
          settle = resolve;
        },
      ),
  );
  const client = { ...unavailableClient, taskExperience: { read } };
  const changed = vi.fn();
  const { result, rerender } = renderHook(() => {
    const policy = useTaskExperience(client, 'owner');
    useEffect(() => {
      changed(policy);
    }, [policy]);
    return policy;
  });
  const initial = result.current;
  expect(initial.settled).toBe(false);
  rerender();
  expect(result.current).toBe(initial);
  expect(changed).toHaveBeenCalledTimes(1);
  const capabilities = {
    task_home_admission: true,
    suggested_followup_admission: true,
    desktop_enable: true,
    browser_takeover: true,
    replay_presentation: true,
  };
  await act(async () =>
    settle({
      object: 'cma.ui.task_experience',
      schema_version: 1,
      account_id: 'owner',
      capabilities,
    }),
  );
  const settled = result.current;
  expect(settled.settled).toBe(true);
  expect(settled.task_home_admission).toBe(true);
  expect(changed).toHaveBeenCalledTimes(2);
  fireEvent(window, new Event('focus'));
  rerender();
  expect(read).toHaveBeenCalledTimes(2);
  expect(result.current).toBe(settled);
  expect(changed).toHaveBeenCalledTimes(2);
  await act(async () =>
    settle({
      object: 'cma.ui.task_experience',
      schema_version: 1,
      account_id: 'owner',
      capabilities: { ...capabilities, browser_takeover: false },
    }),
  );
  expect(result.current.browser_takeover).toBe(false);
  expect(changed).toHaveBeenCalledTimes(3);
});
