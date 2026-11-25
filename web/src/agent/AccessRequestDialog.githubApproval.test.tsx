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
