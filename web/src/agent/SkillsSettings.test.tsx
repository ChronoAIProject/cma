import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { AgentSkillsView } from '../api/client';
import { agentClient } from '../test/agent';
import { referenceClient } from '../test/references';
import { agentSkillsClient } from '../api/rest/agentSkills';
import { SkillsSettings } from './SkillsSettings';

afterEach(() => vi.restoreAllMocks());
const empty: AgentSkillsView = { items: [], pending: false, can_install: true };
const pending: AgentSkillsView = {
  items: [{ id: 'typescript', version: '1.2', source: 'ornn', status: 'pending' }],
  pending: true,
  can_install: false,
};
const installed: AgentSkillsView = {
  items: [{ ...pending.items[0]!, status: 'installed' }],
  pending: false,
  can_install: true,
};
function profiles() {
  const p = referenceClient();
  return {
    ...p,
    authoring: {
      ...p.authoring!,
      searchSkills: vi.fn().mockResolvedValue({
        items: [{ id: 'typescript', name: 'TypeScript', versions: ['1.2'] }],
      }),
    },
  };
}
it('lists accessible Ornn releases and shows installed only after the Agent confirms it', async () => {
  const read = vi.fn().mockResolvedValue(empty);
  const install = vi.fn().mockImplementation(async () => {
    read.mockResolvedValue(pending);
    return pending;
  });
  render(
    <CmaClientProvider
      client={agentClient({ readAgentSkills: read, installAgentSkill: install })}
      profiles={profiles()}
    >
      <SkillsSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  const button = await screen.findByRole('button', { name: 'Install TypeScript' });
  await userEvent.click(button);
  await screen.findByText('Waiting for the Agent to confirm installation.');
  expect(button).toBeDisabled();
  expect(screen.queryByText('1.2 · Installed')).not.toBeInTheDocument();
  expect(install).toHaveBeenCalledWith(
    'agent-one',
    { id: 'typescript', version: '1.2' },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  read.mockResolvedValue(installed);
  await screen.findByText('1.2 · Installed', {}, { timeout: 2500 });
  expect(
    within(screen.getByRole('list', { name: 'Available Ornn skills' })).getByRole('button', {
      name: 'Install TypeScript',
    }),
  ).toBeDisabled();
});
it('retries the identical install intent after a lost response', async () => {
  const install = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost install response'))
    .mockResolvedValue(pending);
  render(
    <CmaClientProvider
      client={agentClient({ readAgentSkills: async () => empty, installAgentSkill: install })}
      profiles={profiles()}
    >
      <SkillsSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  await userEvent.click(await screen.findByRole('button', { name: 'Install TypeScript' }));
  await screen.findByText('Lost install response');
  expect(screen.getByRole('button', { name: 'Install TypeScript' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry installation' }));
  await waitFor(() => expect(install).toHaveBeenCalledTimes(2));
  expect(install.mock.calls[1]?.[1]).toEqual(install.mock.calls[0]?.[1]);
  expect(install.mock.calls[1]?.[2].idempotencyKey).toBe(install.mock.calls[0]?.[2].idempotencyKey);
});
it('uses authenticated Agent skill routes and rejects contradictory status responses', async () => {
  const fetch = vi
    .spyOn(globalThis, 'fetch')
    .mockResolvedValue(new Response(JSON.stringify(pending), { status: 202 }));
  const client = agentSkillsClient();
  await expect(
    client.installAgentSkill!(
      'agent/one',
      { id: 'typescript', version: '1.2' },
      { idempotencyKey: 'same' },
    ),
  ).resolves.toEqual(pending);
  expect(fetch).toHaveBeenCalledWith(
    '/api/v1/agents/agent%2Fone/skills',
    expect.objectContaining({
      method: 'POST',
      credentials: 'same-origin',
      cache: 'no-store',
      headers: { 'Content-Type': 'application/json', 'Idempotency-Key': 'same' },
      body: JSON.stringify({ id: 'typescript', version: '1.2' }),
    }),
  );
  fetch.mockResolvedValue(
    new Response(JSON.stringify({ ...pending, pending: false }), { status: 200 }),
  );
  await expect(client.readAgentSkills!('agent-one')).rejects.toThrow('invalid skill status');
  fetch.mockResolvedValue(
    new Response(JSON.stringify({ items: [], pending: false }), { status: 200 }),
  );
  await expect(client.readAgentSkills!('agent-one')).rejects.toThrow('invalid skill status');
});

it('keeps catalog and installed status visible but disables installs on a retained runtime', async () => {
  const install = vi.fn();
  render(
    <CmaClientProvider
      client={agentClient({
        readAgentSkills: async () => ({
          ...installed,
          items: [{ ...installed.items[0]!, id: 'existing-skill' }],
          can_install: false,
        }),
        installAgentSkill: install,
      })}
      profiles={profiles()}
    >
      <SkillsSettings agentId="agent-one" active busy={false} />
    </CmaClientProvider>,
  );
  expect(await screen.findByText('1.2 · Installed')).toBeVisible();
  expect(await screen.findByRole('button', { name: 'Install TypeScript' })).toBeDisabled();
  expect(
    screen.getByText('This Agent runtime does not support installing skills yet.'),
  ).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Install TypeScript' }));
  expect(install).not.toHaveBeenCalled();
});

it('refreshes install capability when the Agent finishes its current turn', async () => {
  let ready = false;
  const read = vi.fn().mockImplementation(async () => ({ ...empty, can_install: ready }));
  const client = agentClient({ readAgentSkills: read, installAgentSkill: vi.fn() });
  const profileClient = profiles();
  const view = (busy: boolean) => (
    <CmaClientProvider client={client} profiles={profileClient}>
      <SkillsSettings agentId="agent-one" active busy={busy} />
    </CmaClientProvider>
  );
  const { rerender } = render(view(true));
  expect(await screen.findByRole('button', { name: 'Install TypeScript' })).toBeDisabled();
  ready = true;
  rerender(view(false));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Install TypeScript' })).toBeEnabled(),
  );
  expect(read).toHaveBeenCalledTimes(2);
});
