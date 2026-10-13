import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  catalog.listServices.mockResolvedValue([
    { id: 'api-github', name: 'GitHub' },
    { id: 'storage', name: 'Storage' },
  ]);
  vi.spyOn(accessRequests, 'list').mockResolvedValue([request]);
});
afterEach(() => {
  vi.restoreAllMocks();
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
it('opens only a durable request, keeps required scopes fixed, and submits only on explicit approval', async () => {
  const decide = vi
    .spyOn(accessRequests, 'decide')
    .mockResolvedValue({ ...request, status: 'applying' });
  show();
  await screen.findByRole('dialog', { name: 'Agent key access' });
  expect(screen.getByText(request.reason)).toBeVisible();
  expect(decide).not.toHaveBeenCalled();
  for (const scope of ['chrono-llm-public', 'ornn-api']) {
    expect(screen.getByRole('checkbox', { name: `${scope} Required` })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: `${scope} Required` })).toBeDisabled();
  }
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Storage' }));
  vi.mocked(accessRequests.list).mockResolvedValue([{ ...request, status: 'applying' }]);
  await userEvent.click(screen.getByRole('button', { name: 'Approve and connect' }));
  await screen.findByText(/Verifying the updated Agent key/);
  expect(decide).toHaveBeenCalledWith(
    'agent-one',
    'access-one',
    'approve',
    expect.any(String),
    ['chrono-llm-public', 'ornn-api', 'api-github', 'storage'],
    expect.any(AbortSignal),
  );
});
it('retains the exact approval after a lost response and locks edits during recovery', async () => {
  const decide = vi
    .spyOn(accessRequests, 'decide')
    .mockRejectedValueOnce(new Error('Lost approval response'))
    .mockResolvedValue({ ...request, status: 'applying' });
  const first = show();
  await screen.findByRole('dialog');
  await userEvent.click(screen.getByRole('button', { name: 'Approve and connect' }));
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
  show();
  await screen.findByRole('dialog');
  fireEvent.keyDown(document, { key: 'Escape' });
  await screen.findByRole('button', { name: 'Review access request' });
  expect(decide).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Review access request' }));
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
  await userEvent.click(screen.getByRole('button', { name: 'Approve and connect' }));
  await screen.findByText(
    'Choose up to 32 services. Existing Sandbox access must remain included.',
  );
  expect(decide).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
  expect(screen.getByRole('button', { name: 'Approve and connect' })).toBeEnabled();
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
