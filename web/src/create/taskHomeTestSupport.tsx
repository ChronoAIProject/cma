import { fireEvent, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import type { CmaWebClient } from '../api/client';
import { useAuth } from '../auth/context';
import type { AuthProvider } from '../auth/types';
import { AVAILABLE_CAPACITY } from '../test/capacity';
import { taskExperience } from '../test/taskExperience';
import { TaskHome } from './TaskHome';

export function taskClient(transport: typeof fetch, repositorySupport = true) {
  const boundary: typeof fetch = async (input, options) =>
    String(input).endsWith('/openapi.json')
      ? Response.json({
          openapi: '3.1.0',
          info: { title: 'CMA', version: '1' },
          paths: {},
          'x-cma-task-repository': repositorySupport,
          components: {
            schemas: {
              AgentLifecycleCreateRequest: {
                properties: { name: { type: 'string' }, initial_message: { type: 'string' } },
              },
            },
          },
        })
      : transport(input, options);
  vi.stubGlobal('fetch', boundary);
  return {
    ...createRestWebClient(createClient({ fetch: boundary })),
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    taskExperience: taskExperience('owner'),
  };
}

export function AuthenticatedTask({ client }: { readonly client: CmaWebClient }) {
  const { state, signOut } = useAuth();
  return state.status === 'authenticated' ? (
    <>
      <button onClick={() => void signOut()}>Sign out</button>
      <TaskHome client={client} viewerId={state.user.id} canCreateSandbox onCreated={() => {}} />
    </>
  ) : (
    <p>Signed out</p>
  );
}

export function identity(owner: string): AuthProvider {
  return {
    current: async () => ({
      status: 'authenticated',
      user: {
        id: owner,
        handle: owner,
        visitor: false,
        capabilities: ['use_own_sandboxes', 'create_sandbox'],
      },
    }),
    signIn: async () => {},
    signOut: async () => {},
  };
}

export function computerOption(name: string) {
  if (!screen.queryByRole('menu', { name: 'Run this task on' }))
    fireEvent.click(screen.getByRole('button', { name: /^Cloud computer/ }));
  return screen.getByRole('menuitemcheckbox', { name });
}
