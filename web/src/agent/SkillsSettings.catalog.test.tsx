import { ApiError, ProtocolError, TransportError } from '@cma/client';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { AgentSkillsView } from '../api/client';
import * as serviceAuthorization from '../auth/serviceAuthorization';
import { agentClient } from '../test/agent';
import { referenceClient } from '../test/references';
import { SkillsSettings } from './SkillsSettings';

const privateBody = '<html>synthetic-private-body</html>';
const empty: AgentSkillsView = { items: [], pending: false, can_install: true };
const skill = { id: 'typescript', name: 'TypeScript', versions: ['1.2'] };
const page = { items: [skill], next: 'page-two' };
const installed: AgentSkillsView = {
  ...empty,
  items: [
    { id: 'saved', name: 'Saved skill', version: '1.0', source: 'ornn', status: 'installed' },
  ],
};
function fixture() {
  const profiles = referenceClient();
  const search = vi.fn().mockResolvedValue(page);
  const read = vi.fn().mockResolvedValue(installed);
  const install = vi.fn().mockResolvedValue(empty);
  const client = agentClient({ readAgentSkills: read, installAgentSkill: install });
  const profileClient = {
    ...profiles,
    authoring: { ...profiles.authoring!, searchSkills: search },
  };
  const view = (agentId = 'agent-one', viewerId = 'owner-one', active = true) => (
    <CmaClientProvider client={client} profiles={profileClient}>
      <SkillsSettings agentId={agentId} viewerId={viewerId} active={active} busy={false} />
    </CmaClientProvider>
  );
  return { search, read, install, view };
}
const open = () => userEvent.click(screen.getByText('Add skills', { selector: 'summary' }));
afterEach(() => vi.restoreAllMocks());

it.each([
  [new ApiError(401, privateBody, 'not_authenticated', 'local-request'), /Sign in again/, false],
  [new ApiError(403, privateBody, 'forbidden', 'local-request'), /do not have permission/, false],
  [
    new ApiError(429, privateBody, 'rate_limited', 'local-request', 3000),
    /Too many catalog requests/,
    true,
  ],
  [
    new ApiError(503, privateBody, 'profile_skills_unavailable', 'local-request'),
    /temporarily unavailable/,
    true,
  ],
  [new TransportError(privateBody), /could not be reached/, true],
  [new ProtocolError(privateBody), /invalid response/, true],
])(
  'shows distinct safe initial failures and preserves installed skills %#',
  async (failure, message, retry) => {
    const f = fixture();
    f.search.mockRejectedValue(failure);
    const logs = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    render(f.view());
    await open();
    expect(screen.getByRole('textbox', { name: 'Search available Ornn skills' })).toBeDisabled();
    await screen.findByText(message);
    expect(screen.getByText('Saved skill')).toBeVisible();
    expect(screen.queryByText('No matching Ornn skills.')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Retry Ornn catalog' }) !== null).toBe(retry);
    if (failure instanceof ApiError) {
      expect(screen.getByText(`HTTP status: ${failure.status}`)).toBeVisible();
      expect(screen.getByText('Request ID: local-request')).toBeVisible();
      if (failure.status === 429)
        expect(screen.getByRole('button', { name: 'Retry Ornn catalog' })).toBeDisabled();
    }
    expect(document.body.innerHTML).not.toContain('synthetic-private-body');
    expect(JSON.stringify(logs.mock.calls)).not.toContain('synthetic-private-body');
    expect(f.install).not.toHaveBeenCalled();
  },
);

it('keeps repeated failures distinct from empty success and restores search and installation', async () => {
  const f = fixture();
  f.search
    .mockRejectedValueOnce(new ApiError(503, privateBody))
    .mockRejectedValueOnce(new ProtocolError(privateBody))
    .mockResolvedValueOnce({ items: [] });
  render(f.view());
  await open();
  await userEvent.click(await screen.findByRole('button', { name: 'Retry Ornn catalog' }));
  await screen.findByText(/invalid response/);
  expect(screen.queryByText('No matching Ornn skills.')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Retry Ornn catalog' }));
  await screen.findByText('No matching Ornn skills.');
  const search = screen.getByRole('textbox', { name: 'Search available Ornn skills' });
  expect(search).toBeEnabled();
  await userEvent.type(search, 'typescript');
  expect(await screen.findByRole('button', { name: 'Install TypeScript' })).toBeEnabled();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(f.install).not.toHaveBeenCalled();
});

it('requests Ornn consent for the selected Agent and loads the catalog on return', async () => {
  const f = fixture();
  f.search.mockRejectedValue(
    new ApiError(403, privateBody, 'skill_catalog_authorization_required'),
  );
  const consentUrl = 'https://provider.example/consent';
  const start = vi
    .spyOn(serviceAuthorization, 'startServiceAuthorization')
    .mockRejectedValueOnce(new Error('synthetic failure'))
    .mockResolvedValueOnce(consentUrl);
  const navigate = vi
    .spyOn(serviceAuthorization, 'navigateToServiceAuthorization')
    .mockImplementation(() => undefined);
  const view = render(f.view());
  await open();
  const authorize = await screen.findByRole('button', { name: 'Authorize Ornn access' });
  expect(screen.getByRole('textbox', { name: 'Search available Ornn skills' })).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Retry Ornn catalog' })).toBeNull();
  expect(start).not.toHaveBeenCalled();
  await userEvent.click(authorize);
  await screen.findByText(/Ornn authorization could not start/);
  expect(screen.getByRole('link', { name: 'sign in again' })).toHaveAttribute(
    'href',
    expect.stringContaining('return_to=%2Fagents%2Fagent-one'),
  );
  await userEvent.click(authorize);
  expect(start).toHaveBeenLastCalledWith(
    { type: 'agent', id: 'agent-one' },
    'owner-one',
    '/agents/agent-one',
    expect.any(AbortSignal),
    'skill_catalog',
  );
  expect(navigate).toHaveBeenCalledOnce();
  expect(navigate).toHaveBeenCalledWith(window, consentUrl);
  expect(f.install).not.toHaveBeenCalled();

  view.unmount();
  f.search.mockResolvedValue(page);
  render(f.view());
  await open();
  const search = screen.getByRole('textbox', { name: 'Search available Ornn skills' });
  expect(await screen.findByRole('button', { name: 'Install TypeScript' })).toBeEnabled();
  expect(search).toBeEnabled();
  await userEvent.type(search, 'typescript');
  await waitFor(() => expect(f.search).toHaveBeenLastCalledWith('typescript', expect.anything()));
  expect(screen.queryByRole('alert')).toBeNull();
});

it('keeps installed read failures independent from a successful catalog', async () => {
  const f = fixture();
  f.read.mockRejectedValue(new Error('Installed status unavailable'));
  render(f.view());
  await open();
  await screen.findByText('Installed status unavailable');
  expect(await screen.findByRole('button', { name: 'Install TypeScript' })).toBeDisabled();
  expect(screen.getByRole('textbox', { name: 'Search available Ornn skills' })).toBeEnabled();
  expect(screen.queryByText('Loading Agent skills…')).not.toBeInTheDocument();
  f.read.mockResolvedValue(empty);
  await userEvent.click(screen.getByRole('button', { name: 'Refresh skill status' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Install TypeScript' })).toBeEnabled(),
  );
});

it('catalog page recovery cannot resubmit or replace an uncertain installation intent', async () => {
  const f = fixture();
  f.install.mockRejectedValueOnce(new Error('Unknown install result')).mockResolvedValue(empty);
  f.search
    .mockResolvedValueOnce(page)
    .mockRejectedValueOnce(new ApiError(503, privateBody))
    .mockResolvedValue({ items: [skill], next: undefined });
  const random = vi.spyOn(crypto, 'randomUUID');
  render(f.view());
  await open();
  await userEvent.click(await screen.findByRole('button', { name: 'Install TypeScript' }));
  await screen.findByText('Unknown install result');
  const generated = random.mock.calls.length;
  await userEvent.click(screen.getByRole('button', { name: 'More Ornn skills' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Retry skill page' }));
  await waitFor(() =>
    expect(screen.queryByText(/temporarily unavailable/)).not.toBeInTheDocument(),
  );
  await waitFor(() => expect(f.search).toHaveBeenCalledTimes(3));
  expect(f.install).toHaveBeenCalledTimes(1);
  expect(random).toHaveBeenCalledTimes(generated);
  expect(
    within(screen.getByRole('list', { name: 'Available Ornn skills' })).getAllByRole('listitem'),
  ).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: 'Retry installation' }));
  await waitFor(() => expect(f.install).toHaveBeenCalledTimes(2));
  expect(f.install.mock.calls[1]?.slice(0, 2)).toEqual(f.install.mock.calls[0]?.slice(0, 2));
  expect(f.install.mock.calls[1]?.[2].idempotencyKey).toBe(
    f.install.mock.calls[0]?.[2].idempotencyKey,
  );
});

it.each(['owner', 'agent'])(
  'clears scoped state and aborts late requests on %s changes',
  async (scope) => {
    const f = fixture();
    let complete!: (value: typeof page) => void;
    f.search.mockReturnValueOnce(
      new Promise((resolve) => {
        complete = resolve;
      }),
    );
    const { rerender, unmount } = render(f.view());
    await open();
    await waitFor(() => expect(f.search).toHaveBeenCalledTimes(1));
    const signal = f.search.mock.calls[0]?.[1].signal as AbortSignal;
    rerender(
      f.view(
        scope === 'agent' ? 'agent-two' : 'agent-one',
        scope === 'owner' ? 'owner-two' : 'owner-one',
      ),
    );
    expect(signal.aborted).toBe(true);
    await open();
    await screen.findByRole('button', { name: 'Install TypeScript' });
    await act(async () =>
      complete({ items: [{ ...skill, id: 'old', name: 'Old private skill' }], next: 'old-page' }),
    );
    expect(screen.queryByText('Old private skill')).not.toBeInTheDocument();
    expect(f.install).not.toHaveBeenCalled();
    unmount();
    expect(f.search.mock.calls[1]?.[1].signal.aborted).toBe(true);
  },
);

it('clears an uncertain install error when installed-state refresh confirms that exact intent', async () => {
  const f = fixture();
  f.install.mockRejectedValue(new Error('Unknown install result'));
  const { rerender } = render(f.view());
  await open();
  await userEvent.click(await screen.findByRole('button', { name: 'Install TypeScript' }));
  await screen.findByText('Unknown install result');
  f.read.mockResolvedValue({
    ...empty,
    items: [{ id: 'typescript', version: '1.2', source: 'ornn', status: 'installed' }],
  });
  rerender(f.view('agent-one', 'owner-one', false));
  rerender(f.view());
  await screen.findByText('1.2 · Installed');
  expect(screen.queryByText('Unknown install result')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Retry installation' })).not.toBeInTheDocument();
  expect(f.install).toHaveBeenCalledTimes(1);
});

it('retains search and a chosen version across catalog page failure and retry', async () => {
  const f = fixture();
  const versions = { ...skill, versions: ['1.2', '1.1'] };
  f.search.mockResolvedValue({ items: [versions], next: 'page-two' });
  render(f.view());
  await open();
  const search = screen.getByRole('textbox', { name: 'Search available Ornn skills' });
  await waitFor(() => expect(search).toBeEnabled());
  await userEvent.type(search, 'typescript');
  await waitFor(() => expect(f.search).toHaveBeenLastCalledWith('typescript', expect.anything()));
  const picker = await screen.findByRole('button', { name: 'Version for TypeScript' });
  await waitFor(() => expect(picker).toBeEnabled());
  await userEvent.click(picker);
  await userEvent.click(screen.getByRole('menuitemradio', { name: '1.1' }));
  f.search.mockRejectedValueOnce(new ApiError(503, privateBody));
  await userEvent.click(screen.getByRole('button', { name: 'More Ornn skills' }));
  expect(
    await screen.findByRole('heading', { name: 'Couldn’t load the skills catalog' }),
  ).toBeVisible();
  expect(picker).toHaveTextContent('1.1');
  expect(search).toHaveValue('typescript');
  expect(screen.getByRole('button', { name: 'Install TypeScript' })).toBeDisabled();
  f.search.mockResolvedValue({ items: [], next: undefined });
  await userEvent.click(screen.getByRole('button', { name: 'Retry skill page' }));
  await waitFor(() => expect(picker).toBeEnabled());
  expect(picker).toHaveTextContent('1.1');
  expect(search).toHaveValue('typescript');
  expect(f.install).not.toHaveBeenCalled();
});
