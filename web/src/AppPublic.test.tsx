import { AVAILABLE_CAPACITY } from './test/capacity';
import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { createNyxidAuthProvider } from './auth/nyxid';
import { CmaClientProvider } from './api/context';
import { createRestWebClient } from './api/rest';
import { createProfileViewsClient } from './api/profiles';
import { createCreationClient } from './api/creation';
import { createOperatorClient } from './operator/client';
import { createTriggerPublisherClient } from './trigger-publisher/client';
import { createPublicClient } from './public/client';
import { agentClient } from './test/agent';
import { PUBLIC_PREVIEW, PUBLIC_RECORD, publicClient } from './test/public';

afterEach(() => {
  window.history.replaceState({}, '', '/');
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

it('keeps the public landing outside the auth gate and admits only the explicit sign-in intent', async () => {
  window.history.replaceState({}, '', '/t/public-example/plc_readme');
  vi.stubGlobal(
    'fetch',
    vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            social_providers: [],
            email_signup: true,
            invite_code_required: false,
            register_available: true,
          }),
        ),
    ),
  );
  const visitor = publicClient();
  const provider = createMockAuthProvider();
  const view = render(
    <StrictMode>
      <AuthContextProvider provider={provider}>
        <CmaClientProvider client={agentClient()} publicClient={visitor}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>
    </StrictMode>,
  );
  await screen.findByRole('heading', { name: PUBLIC_PREVIEW.title });
  expect(screen.queryByRole('button', { name: 'Sign in' })).not.toBeInTheDocument();
  expect(visitor.requests.createOrResumeBound).not.toHaveBeenCalled();
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Open in CMA' }));
  await waitFor(() => expect(window.location.pathname).toBe(`/launches/${PUBLIC_RECORD.id}`));
  expect(visitor.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(await screen.findByText('Preparing software')).toBeInTheDocument();
  view.unmount();
});

it.each(
  [true, false].flatMap((visitor) =>
    ['/', '/t/public-example', '/t/public-example/plc_readme'].map((path) => ({
      visitor,
      path,
    })),
  ),
)(
  'creates no default resources on an unseen identity return without click intent: $path, visitor=$visitor',
  async ({ visitor, path }) => {
    const resourceWrites = {
      sandboxes: 0,
      workspaces: 0,
      worktrees: 0,
      agents: 0,
      'sandbox-profiles': 0,
      'workspace-profiles': 0,
      'agent-profiles': 0,
    };
    const rejectedRequests: string[] = [];
    const requests: { method: string; path: string }[] = [];
    const transport = vi.fn<typeof fetch>(async (input, init) => {
      const requestPath = new URL(
        input instanceof Request ? input.url : String(input),
        window.location.origin,
      ).pathname;
      const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
      requests.push({ method, path: requestPath });
      const reply = (body: unknown) =>
        new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } });
      if (method === 'GET' && requestPath === '/auth/session') {
        return reply({
          authenticated: true,
          subject: 'unseen-cma-identity',
          display_name: 'Contract Visitor',
          visitor,
          capabilities: visitor
            ? ['launch_triggers']
            : [
                'launch_triggers',
                'use_own_sandboxes',
                'create_sandbox',
                'manage_own_profiles',
                'publish_triggers',
              ],
        });
      }
      if (method === 'GET' && requestPath === '/api/v1/sandboxes/capacity')
        return Response.json(AVAILABLE_CAPACITY);
      if (method === 'GET' && requestPath === '/api/v1/sandboxes') {
        return reply({ items: [] });
      }
      if (method === 'GET' && requestPath === '/api/v2/ui/task-experience' && !visitor) {
        return reply({
          object: 'cma.ui.task_experience',
          schema_version: 1,
          account_id: 'unseen-cma-identity',
          capabilities: {
            task_home_admission: true,
            suggested_followup_admission: true,
            desktop_enable: true,
            browser_takeover: true,
            replay_presentation: true,
          },
        });
      }
      if (method === 'GET' && requestPath === '/api/v1/organizations' && !visitor) {
        return reply({ items: [] });
      }
      if (method === 'GET' && requestPath === '/openapi.json' && !visitor) {
        return reply({
          openapi: '3.1.0',
          info: { title: 'CMA', version: '1' },
          paths: {},
          'x-cma-task-repository': true,
          components: {
            schemas: {
              AgentLifecycleCreateRequest: {
                properties: { initial_message: { type: 'string' } },
              },
            },
          },
        });
      }
      if (method === 'GET' && requestPath === '/api/v1/triggers/public/public-example') {
        return reply({ ...PUBLIC_PREVIEW, always_review: false });
      }
      if (
        method === 'POST' &&
        requestPath === '/api/v1/triggers/public/public-example/entry-intent'
      ) {
        return reply({ accepted: false });
      }
      for (const resource of Object.keys(resourceWrites) as (keyof typeof resourceWrites)[]) {
        if (method !== 'GET' && requestPath.split('/').includes(resource)) {
          resourceWrites[resource] += 1;
        }
      }
      rejectedRequests.push(`${method} ${requestPath}`);
      throw new Error(`Unexpected post-login request: ${method} ${requestPath}`);
    });
    vi.stubGlobal('fetch', transport);
    const sdk = createClient({ fetch: transport });

    // Complement the real callback contract with its post-redirect application effects.
    for (let mount = 0; mount < 2; mount += 1) {
      window.history.replaceState({}, '', path);
      const before = requests.length;
      const view = render(
        <StrictMode>
          <AuthContextProvider provider={createNyxidAuthProvider()}>
            <CmaClientProvider
              client={createRestWebClient(sdk)}
              profiles={createProfileViewsClient(sdk)}
              creation={createCreationClient(sdk)}
              operator={createOperatorClient(sdk)}
              publisher={createTriggerPublisherClient(sdk)}
              publicClient={createPublicClient(sdk)}
            >
              <App />
            </CmaClientProvider>
          </AuthContextProvider>
        </StrictMode>,
      );
      await screen.findByRole('heading', {
        name:
          path === '/'
            ? visitor
              ? 'Welcome to CMA'
              : 'What should your Agent work on?'
            : PUBLIC_PREVIEW.title,
      });
      await waitFor(() =>
        expect(requests.slice(before)).toContainEqual(
          path === '/'
            ? { method: 'GET', path: '/auth/session' }
            : { method: 'POST', path: '/api/v1/triggers/public/public-example/entry-intent' },
        ),
      );
      await act(async () => undefined);
      expect(requests.slice(before)).toContainEqual({ method: 'GET', path: '/auth/session' });
      expect(window.location.pathname).toBe(path === '/' ? '/agent-manager' : path);
      expect(Object.fromEntries(Object.entries(sessionStorage))).toEqual({
        'cma:task-viewer': 'unseen-cma-identity',
        'cma:agent-draft-owner': 'unseen-cma-identity',
        'cma:preparation-draft-owner': 'unseen-cma-identity',
      });
      if (path === '/' && !visitor) {
        expect(screen.getByRole('textbox', { name: 'Your task' })).toHaveValue('');
        expect(screen.getByRole('button', { name: 'Start task' })).toBeDisabled();
      }
      expect(rejectedRequests).toEqual([]);
      expect(resourceWrites).toEqual({
        sandboxes: 0,
        workspaces: 0,
        worktrees: 0,
        agents: 0,
        'sandbox-profiles': 0,
        'workspace-profiles': 0,
        'agent-profiles': 0,
      });
      expect(requests.filter(({ path: requestPath }) => requestPath.includes('/launches'))).toEqual(
        [],
      );
      view.unmount();
    }
  },
);
