import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { AuthContextProvider, useAuth } from './auth/context';
import type { AuthProvider } from './auth/types';
import { unavailableClient, type CmaWebClient } from './api/client';
import { useTaskExperience } from './taskExperience';

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
