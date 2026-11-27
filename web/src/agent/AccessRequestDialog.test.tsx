import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { accessRequests, AgentAccessError, type AgentAccessRequest } from './accessRequests';
import { AgentAccessRequests } from './AccessRequestDialog';
import { useAccessRequests } from './useAccessRequests';
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
it('keeps the server retry floor across turn changes and live events', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const list = vi.mocked(accessRequests.list);
  list.mockRejectedValueOnce(new AgentAccessError(429, 'Limited', 10_000, 'rate_limited'));
  const { rerender } = renderHook(
    ({ running, revision }) => useAccessRequests('agent-one', 'owner', true, running, revision),
    { initialProps: { running: false, revision: 0 } },
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  rerender({ running: true, revision: 1 });
  rerender({ running: false, revision: 2 });
  rerender({ running: true, revision: 3 });
  await act(() => vi.advanceTimersByTimeAsync(9999));
  expect(list).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(list).toHaveBeenCalledTimes(2);
});

it('explains that approval updates access without restarting active Agents', async () => {
  show();
  await screen.findByText(/Approving updates service access without restarting/);
  expect(screen.queryByText(/briefly pauses and resumes/)).not.toBeInTheDocument();
});
it('retains an approved request while paused without claiming runtime verification or repeating approval', async () => {
  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'applying' }]);
  const decide = vi.spyOn(accessRequests, 'decide');
  const view = render(
    <AgentAccessRequests
      agentId="agent-one"
      owner="owner"
      granted={['chrono-llm-public', 'ornn-api']}
      sandboxStatus="paused"
      enabled
    />,
  );
  await screen.findByText(/The Sandbox is paused. Your approval is saved/);
  expect(screen.queryByText(/Verifying the updated Agent key/)).not.toBeInTheDocument();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  view.rerender(
    <AgentAccessRequests
      agentId="agent-one"
      owner="owner"
      granted={['chrono-llm-public', 'ornn-api']}
      sandboxStatus="active"
      enabled
    />,
  );
  await screen.findByText(/Verifying the updated Agent key/);
  expect(decide).not.toHaveBeenCalled();
});
it('explains how to recover a failed update without exposing its internal error code', async () => {
  vi.mocked(accessRequests.list).mockResolvedValue([
    { ...request, status: 'failed', error: 'access_update_cannot_continue' },
  ]);
  show();
  await screen.findByRole('alert');
  expect(screen.getByText(/Ask the Agent to create a new access request/)).toBeVisible();
  expect(screen.queryByText('access_update_cannot_continue')).not.toBeInTheDocument();
});
it('opens only a durable request, keeps required scopes fixed, and submits only on explicit approval', async () => {
  const decide = vi
    .spyOn(accessRequests, 'decide')
    .mockResolvedValue({ ...request, status: 'applying' });
  show();
  await screen.findByRole('dialog', { name: 'Agent key access' });
  expect(screen.getByText(request.reason)).toBeVisible();
  expect(
    screen.getByText('GitHub connection selection is separate from service approval.'),
  ).toBeVisible();
  expect(decide).not.toHaveBeenCalled();
  for (const scope of ['chrono-llm-public', 'ornn-api']) {
    expect(screen.getByRole('checkbox', { name: `${scope} Required` })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: `${scope} Required` })).toBeDisabled();
  }
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Storage' }));
  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'applying' }]);
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  await screen.findByText(/Verifying the updated Agent key/);
  expect(decide).toHaveBeenCalledWith(
    'agent-one',
    'access-one',
    'approve',
    expect.any(String),
    ['chrono-llm-public', 'ornn-api', 'api-github', 'storage'],
    expect.any(AbortSignal),
    undefined,
  );
});

it('opens owner-scoped GitHub connection management without submitting an approval', async () => {
  const decide = vi.spyOn(accessRequests, 'decide');
  const manageUrl = 'https://nyx.example/keys';
  const fetch = vi.fn(async () =>
    Response.json({
      owner: 'owner',
      github: {
        manage_url: manageUrl,
        connections: [
          {
            id: 'profile-only',
            label: 'Profile access',
            granted_scopes: ['read:user'],
            manage_url: `${manageUrl}/profile-only`,
          },
          {
            id: 'repository-access',
            label: 'Repository access',
            granted_scopes: ['repo'],
            manage_url: `${manageUrl}/repository-access`,
          },
        ],
      },
    }),
  );
  vi.stubGlobal('fetch', fetch);
  show();
  const manage = await screen.findByRole('button', { name: 'Manage GitHub connections' });
  expect(fetch).not.toHaveBeenCalled();
  await userEvent.click(manage);
  expect(fetch).toHaveBeenCalledExactlyOnceWith(
    '/auth/connections/repository-access',
    expect.objectContaining({
      body: JSON.stringify({
        target: {
          type: 'agent_service_access',
          id: 'agent-one',
          services: ['api-github', 'chrono-llm-public', 'ornn-api'],
        },
      }),
    }),
  );
  expect(
    await screen.findByRole('link', { name: 'Repository access (repository-access)' }),
  ).toHaveAttribute('href', `${manageUrl}/repository-access`);
  expect(screen.getByRole('link', { name: 'Profile access (profile-only)' })).toHaveAttribute(
    'href',
    `${manageUrl}/profile-only`,
  );
  expect(screen.getByText('Reported scopes: repo')).toBeVisible();
  expect(screen.getByText('Reported scopes: read:user')).toBeVisible();
  expect(screen.getByText(/CMA has not verified the Agent/)).toBeVisible();
  expect(decide).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Hide GitHub connections' }));
  expect(screen.queryByRole('link', { name: /Repository access/ })).not.toBeInTheDocument();
});
it('retains the exact approval after a lost response and locks edits during recovery', async () => {
  const decide = vi
    .spyOn(accessRequests, 'decide')
    .mockRejectedValueOnce(new Error('Lost approval response'))
    .mockResolvedValue({ ...request, status: 'applying' });
  const first = show();
  await screen.findByRole('dialog');
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  await screen.findByText('Lost approval response');
  first.unmount();
  show();
  await screen.findByRole('button', { name: 'Retry access decision' });
  expect(await screen.findByRole('checkbox', { name: 'api-github Requested' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry access decision' }));
  await waitFor(() => expect(decide).toHaveBeenCalledTimes(2));
  expect(decide.mock.calls[1]?.slice(0, 5)).toEqual(decide.mock.calls[0]?.slice(0, 5));
});
it('does not treat successful OAuth verification as runtime access completion', async () => {
  vi.useFakeTimers();
  vi.mocked(accessRequests.list).mockResolvedValue([
    {
      ...request,
      status: 'connecting',
      connection: { target: { type: 'agent_access_request', id: request.id } },
    },
  ]);
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-10-13T00:00:00Z',
    services: [],
  });
  show();
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText(/Verifying the updated Agent key/)).toBeVisible();
  expect(screen.getByRole('dialog', { name: 'Agent key access' })).toBeVisible();
  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'succeeded' }]);
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
it('leaves a dismissed request pending and permits reopening without approval', async () => {
  const decide = vi.spyOn(accessRequests, 'decide');
  const user = userEvent.setup();
  show();
  await screen.findByRole('dialog', { name: 'Agent key access' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus());
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  await screen.findByRole('button', { name: 'Review access request' });
  expect(decide).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Review access request' }));
  expect(screen.getByRole('dialog', { name: 'Agent key access' })).toBeVisible();
});
it('never displays another Agent’s access request', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
    new Response(JSON.stringify({ items: [{ ...request, agent_id: 'someone-else' }] }), {
      status: 200,
    }),
  );
  vi.mocked(accessRequests.list).mockRestore();
  await expect(accessRequests.list('agent-one', new AbortController().signal)).rejects.toThrow(
    'invalid access request',
  );
  expect(fetch).toHaveBeenCalledOnce();
});
it('refuses oversized additive approval before saving an unchangeable decision', async () => {
  const decide = vi.spyOn(accessRequests, 'decide');
  vi.mocked(accessRequests.list).mockResolvedValue([
    { ...request, services: Array.from({ length: 31 }, (_, index) => `service-${index}`) },
  ]);
  show();
  await screen.findByRole('dialog');
  await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
  await screen.findByText(
    'Choose up to 32 services. Existing Sandbox access must remain included.',
  );
  expect(decide).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
  expect(screen.getByRole('button', { name: 'Approve' })).toBeEnabled();
});
it('rejects invalid service projections and approval bodies before network effects', async () => {
  vi.mocked(accessRequests.list).mockRestore();
  const fetch = vi.spyOn(globalThis, 'fetch');
  for (const services of [
    [],
    ['github/read'],
    ['x'.repeat(129)],
    Array<string>(33).fill('service'),
  ]) {
    fetch.mockResolvedValue(new Response(JSON.stringify({ items: [{ ...request, services }] })));
    await expect(accessRequests.list('agent-one', new AbortController().signal)).rejects.toThrow(
      'invalid access request',
    );
  }
  fetch.mockClear();
  await expect(
    accessRequests.decide(
      'agent-one',
      'access-one',
      'approve',
      'key',
      [],
      new AbortController().signal,
    ),
  ).rejects.toThrow('Choose 1 to 32');
  expect(fetch).not.toHaveBeenCalled();
});
it('replays an uncertain denial as the original denial after reload', async () => {
  const decide = vi
    .spyOn(accessRequests, 'decide')
    .mockRejectedValue(new Error('Lost denial response'));
  const first = show();
  await screen.findByRole('dialog');
  await userEvent.click(screen.getByRole('button', { name: 'Deny' }));
  await screen.findByText('Lost denial response');
  first.unmount();
  show();
  await userEvent.click(await screen.findByRole('button', { name: 'Retry access decision' }));
  await waitFor(() => expect(decide).toHaveBeenCalledTimes(2));
  expect(decide.mock.calls[0]?.slice(0, 5)).toEqual([
    'agent-one',
    'access-one',
    'deny',
    expect.any(String),
    [],
  ]);
  expect(decide.mock.calls[1]?.slice(0, 5)).toEqual(decide.mock.calls[0]?.slice(0, 5));
});

it('lets the owner review expired connections without repeating catalog checks during hot application', async () => {
  vi.useFakeTimers();
  const target = { type: 'agent_access_request' as const, id: request.id };
  vi.mocked(accessRequests.list).mockResolvedValue([
    { ...request, status: 'connecting', connection: { target } },
  ]);
  let ready = true;
  vi.spyOn(connectionClient, 'check').mockImplementation(async () => ({
    owner: 'owner',
    ready,
    checked_at: new Date().toISOString(),
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: ready ? 'satisfied' : 'inactive',
        connection_mode: 'hosted',
      },
    ],
  }));
  const start = vi.spyOn(connectionClient, 'start').mockImplementation(async (_, active) => ({
    ...active,
    owner: 'owner',
    hosted_url: 'https://identity.example.test/reconnect',
    expires_at: new Date(Date.now() + 600000).toISOString(),
  }));
  const popup = {
    opener: {},
    closed: false,
    location: { href: '' },
    focus: vi.fn(),
    close: vi.fn(),
  };
  vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
  const decide = vi.spyOn(accessRequests, 'decide');
  show();
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText(/Verifying the updated Agent key/)).toBeVisible();

  vi.mocked(accessRequests.list).mockResolvedValue([
    { ...request, status: 'applying', connection: { target } },
  ]);
  await act(() => vi.advanceTimersByTimeAsync(2000));
  ready = false;
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(connectionClient.check).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Review service connections' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('dialog', { name: 'Connect Agent services' })).toBeVisible();
  expect(screen.getByText('Connection needed')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(start).toHaveBeenCalledOnce();
  expect(start.mock.calls[0]?.[0]).toEqual(target);
  expect(start.mock.calls[0]?.[1].service_id).toBe('api-github');
  expect(popup.location.href).toBe('https://identity.example.test/reconnect');

  popup.closed = true;
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.getByRole('dialog', { name: 'Connect Agent services' })).toBeVisible();
  expect(screen.queryByText(/Verifying the updated Agent key/)).not.toBeInTheDocument();
  ready = true;
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.getByRole('dialog', { name: 'Agent key access' })).toBeVisible();
  expect(screen.getByText(/Verifying the updated Agent key/)).toBeVisible();
  expect(decide).not.toHaveBeenCalled();
  expect(start).toHaveBeenCalledOnce();

  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'succeeded' }]);
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('pauses hidden access-request reads and resumes without overlapping an unfinished read', async () => {
  vi.useFakeTimers();
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  let resolveRead: (value: AgentAccessRequest[]) => void = () => {};
  vi.mocked(accessRequests.list)
    .mockImplementationOnce(
      () =>
        new Promise<AgentAccessRequest[]>((resolve) => {
          resolveRead = resolve;
        }),
    )
    .mockResolvedValue([]);
  const { unmount } = show();
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(accessRequests.list).not.toHaveBeenCalled();
  hidden.mockReturnValue(false);
  await act(async () => {
    document.dispatchEvent(new Event('visibilitychange'));
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(accessRequests.list).toHaveBeenCalledTimes(1);
  hidden.mockReturnValue(true);
  await act(async () => {
    resolveRead([]);
  });
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(accessRequests.list).toHaveBeenCalledTimes(1);
  hidden.mockReturnValue(false);
  await act(async () => {
    document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(accessRequests.list).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(29999));
  expect(accessRequests.list).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(accessRequests.list).toHaveBeenCalledTimes(3);
  unmount();
  await act(async () => {
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(accessRequests.list).toHaveBeenCalledTimes(3);
});

it('requires service connection readiness before the final approval and does not grant on preflight', async () => {
  vi.mocked(connectionClient.check).mockResolvedValue({
    owner: 'owner',
    ready: false,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  });
  const decide = vi.spyOn(accessRequests, 'decide');
  show();
  await screen.findByRole('button', { name: 'Connect services' });
  expect(screen.getByRole('button', { name: 'Approve' })).toBeDisabled();
  expect(decide).not.toHaveBeenCalled();
  expect(connectionClient.check).toHaveBeenCalledWith(
    {
      type: 'agent_service_access',
      id: 'agent-one',
      services: ['api-github', 'chrono-llm-public', 'ornn-api'],
    },
    undefined,
    expect.any(AbortSignal),
  );
});
