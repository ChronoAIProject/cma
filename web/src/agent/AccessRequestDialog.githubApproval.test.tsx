import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { accessRequests, type AgentAccessRequest } from './accessRequests';
import { AgentAccessRequests } from './AccessRequestDialog';
import { connectionClient } from '../connections/client';

const catalog = vi.hoisted(() => ({ listServices: vi.fn() }));
vi.mock('../api/context', () => ({ useProfileViewsClient: () => ({ authoring: catalog }) }));
const request: AgentAccessRequest = {
  id: 'access-one',
  agent_id: 'agent-one',
  status: 'pending',
  services: ['api-github'],
  reason: 'Clone the repository and create a pull request.',
};
beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  });
  catalog.listServices.mockResolvedValue([
    { id: 'api-github', name: 'GitHub' },
    { id: 'storage', name: 'Storage' },
  ]);
  vi.spyOn(accessRequests, 'list').mockResolvedValue([request]);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
function show() {
  return render(
    <AgentAccessRequests
      agentId="agent-one"
      owner="owner"
      granted={['chrono-llm-public', 'ornn-api']}
      enabled
    />,
  );
}
it('requires the owner to choose a repo-capable connection and replays the exact choice', async () => {
  const scope = {
    sandbox_id: 'sandbox-one',
    sandbox_name: 'Shared workspace',
    scope_revision: 3,
    credential_generation: 7,
  };
  vi.mocked(accessRequests.list).mockResolvedValue([
    {
      ...request,
      github_approval: {
        ...scope,
        other_agents: ['agent-two'],
        current_user_service_id: 'profile-only',
      },
    },
  ]);
  const decide = vi
    .spyOn(accessRequests, 'decide')
    .mockRejectedValueOnce(new Error('Lost selected approval response'))
    .mockResolvedValue({ ...request, status: 'applying' });
  vi.stubGlobal(
    'fetch',
    vi.fn(async () =>
      Response.json({
        owner: 'owner',
        github: {
          manage_url: 'https://nyx.example/keys',
          connections: [
            {
              id: 'profile-only',
              label: 'Profile access',
              granted_scopes: ['read:user'],
              manage_url: 'https://nyx.example/keys/profile-only',
            },
            {
              id: 'stale-repository-access',
              label: 'Stale repository access',
              granted_scopes: ['repo'],
              account: { login: 'stale-owner', observed_scopes: ['read:user'] },
              manage_url: 'https://nyx.example/keys/stale-repository-access',
            },
            {
              id: 'repository-access',
              label: 'Repository access',
              granted_scopes: ['repo'],
              account: { login: 'repository-owner', observed_scopes: ['repo', 'read:user'] },
              manage_url: 'https://nyx.example/keys/repository-access',
            },
          ],
        },
      }),
    ),
  );
  const first = show();
  expect(await screen.findByText(/Shared workspace/)).toBeVisible();
  expect(screen.getByText('agent-two')).toBeVisible();
  expect(screen.getByText('Currently selected connection: profile-only')).toBeVisible();
  expect(await screen.findByText('GitHub account @repository-owner')).toBeVisible();
  expect(screen.getByText('Live scopes — repo, read:user')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Approve' })).toBeDisabled();
  expect(screen.queryByRole('button', { name: /Select Profile access/ })).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: /Select Stale repository access/ }),
  ).not.toBeInTheDocument();
  await userEvent.click(await screen.findByRole('button', { name: /Select Repository access/ }));
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  expect(await screen.findByText('Lost selected approval response')).toBeVisible();
  first.unmount();
  show();
  const retry = await screen.findByRole('button', { name: 'Retry access decision' });
  expect(await screen.findByRole('button', { name: /Select Repository access/ })).toBeDisabled();
  await userEvent.click(retry);
  await waitFor(() => expect(decide).toHaveBeenCalledTimes(2));
  expect(decide.mock.calls[0]?.[6]).toEqual({
    sandbox_id: scope.sandbox_id,
    scope_revision: scope.scope_revision,
    credential_generation: scope.credential_generation,
    user_service_id: 'repository-access',
  });
  expect(decide.mock.calls[1]?.[3]).toBe(decide.mock.calls[0]?.[3]);
  expect(decide.mock.calls[1]?.[6]).toEqual(decide.mock.calls[0]?.[6]);
});

it('requires fresh consent after a definitely refused stale generation, including after reload', async () => {
  const context = {
    sandbox_id: 'sandbox-one',
    sandbox_name: 'Shared workspace',
    other_agents: ['agent-two'],
    current_user_service_id: 'previous',
    scope_revision: 3,
    credential_generation: 7,
  };
  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, github_approval: context }]);
  const writes: { key: string | null; choice: unknown }[] = [];
  const current = { ...context, scope_revision: 4, credential_generation: 8 };
  vi.stubGlobal(
    'fetch',
    vi.fn(async (_path: string, options?: RequestInit) => {
      if (options?.method === 'POST' && new Headers(options.headers).has('Idempotency-Key')) {
        writes.push({
          key: new Headers(options.headers).get('Idempotency-Key'),
          choice: JSON.parse(String(options.body)).github_connection,
        });
        if (writes.length === 1) {
          vi.mocked(accessRequests.list).mockResolvedValue([
            { ...request, github_approval: current },
          ]);
          return Response.json(
            {
              code: 'agent_access_stale_github_approval',
              detail: 'GitHub access changed before approval. Choose again.',
            },
            { status: 409, headers: { 'Content-Type': 'application/problem+json' } },
          );
        }
        return Response.json({ ...request, status: 'applying' });
      }
      return Response.json({
        owner: 'owner',
        github: {
          manage_url: 'https://nyx.example/keys',
          connections: [
            {
              id: 'repository-access',
              label: 'Repository access',
              granted_scopes: ['repo'],
              manage_url: 'https://nyx.example/keys/repository-access',
            },
          ],
        },
      });
    }),
  );
  const first = show();
  await userEvent.click(await screen.findByRole('button', { name: /Select Repository access/ }));
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  expect(
    await screen.findByText('GitHub access changed before approval. Choose again.'),
  ).toBeVisible();
  expect(screen.getByRole('button', { name: 'Approve' })).toBeDisabled();
  expect(sessionStorage.length).toBe(0);
  first.unmount();

  show();
  expect(await screen.findByRole('button', { name: 'Approve' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Deny' })).toBeEnabled();
  expect(writes).toHaveLength(1);
  await userEvent.click(await screen.findByRole('button', { name: /Select Repository access/ }));
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  await waitFor(() => expect(writes).toHaveLength(2));
  expect(writes[0]?.choice).toMatchObject({ scope_revision: 3, credential_generation: 7 });
  expect(writes[1]?.choice).toMatchObject({
    scope_revision: 4,
    credential_generation: 8,
    user_service_id: 'repository-access',
  });
  expect(writes[1]?.key).not.toBe(writes[0]?.key);
});

it('preserves an exact retry for generic conflict until durable request state is refreshed', async () => {
  const original = {
    ...request,
    github_approval: {
      sandbox_id: 'sandbox-one',
      sandbox_name: 'Shared workspace',
      other_agents: [],
      current_user_service_id: null,
      scope_revision: 3,
      credential_generation: 7,
    },
  };
  vi.mocked(accessRequests.list).mockResolvedValue([original]);
  const writes: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (_path: string, options?: RequestInit) => {
      if (options?.method === 'POST' && new Headers(options.headers).has('Idempotency-Key')) {
        writes.push(new Headers(options.headers).get('Idempotency-Key') ?? '');
        if (writes.length === 2) {
          vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'applying' }]);
        }
        return Response.json(
          { code: 'agent_access_conflict', detail: 'The request already changed.' },
          { status: 409, headers: { 'Content-Type': 'application/problem+json' } },
        );
      }
      return Response.json({
        owner: 'owner',
        github: {
          manage_url: 'https://nyx.example/keys',
          connections: [
            {
              id: 'repository-access',
              label: 'Repository access',
              granted_scopes: ['repo'],
              manage_url: 'https://nyx.example/keys/repository-access',
            },
          ],
        },
      });
    }),
  );
  show();
  await userEvent.click(await screen.findByRole('button', { name: /Select Repository access/ }));
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  await screen.findByText('The request already changed.');
  expect(screen.getByRole('button', { name: 'Deny' })).toBeDisabled();
  expect(screen.getByRole('button', { name: /Select Repository access/ })).toBeDisabled();
  expect(sessionStorage.length).toBe(1);
  await userEvent.click(screen.getByRole('button', { name: 'Retry access decision' }));
  await screen.findByText(/Verifying the updated Agent key/, {}, { timeout: 5000 });
  expect(writes).toHaveLength(2);
  expect(writes[1]).toBe(writes[0]);
  expect(screen.queryByRole('button', { name: 'Retry access decision' })).not.toBeInTheDocument();
});
