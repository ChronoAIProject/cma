import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import {
  FORK_ID,
  PROFILE_IDS,
  profileAuthoringTransport,
  REPOSITORY,
} from './test/profileAuthoring';
import type { ProfileKind } from './routing/routes';

afterEach(() => window.history.replaceState({}, '', '/'));

it('authors linked research profiles through routed public SDK clients and forks a published Sandbox as another owner', async () => {
  const test = profileAuthoringTransport();
  const user = userEvent.setup();
  const first = test.render();
  const startProfile = async (kind: string) => {
    await user.click(await screen.findByRole('tab', { name: kind }));
    await user.click(screen.getByRole('button', { name: 'New profile' }));
    await screen.findByRole('heading', { name: `New ${kind.toLowerCase()} profile` });
    expect(window.location.pathname).toBe(`/profiles/${kind.toLowerCase()}-profiles/new`);
    fireEvent.change(screen.getByLabelText('Name'), {
      target: { value: `trureturing ${kind.toLowerCase()}` },
    });
    fireEvent.change(screen.getByLabelText('Description'), {
      target: { value: 'Repository research and development.' },
    });
  };
  const publishAndShare = async (kind: ProfileKind) => {
    await user.click(screen.getByRole('button', { name: 'Save draft' }));
    await screen.findByRole('heading', { name: `trureturing ${kind}` });
    expect(window.location.pathname).toBe(`/profiles/${kind}-profiles/${PROFILE_IDS[kind]}`);
    await screen.findByRole('button', { name: 'Publish revision' });
    if (kind === 'agent') {
      fireEvent.change(screen.getByLabelText('Description'), {
        target: { value: 'Published research instructions for trureturing.' },
      });
      await user.click(screen.getByRole('button', { name: 'Save draft' }));
      await screen.findByText('Draft saved.');
    }
    await user.click(await screen.findByRole('button', { name: 'Publish revision' }));
    const dialog = screen.getByRole('dialog', { name: 'Publish revision 1' });
    await user.click(within(dialog).getByRole('button', { name: 'Publish revision 1' }));
    await screen.findByText('Revision 1 published.');
    await user.click(screen.getByRole('button', { name: 'Visibility' }));
    await user.click(screen.getByRole('menuitemradio', { name: 'Public' }));
    await screen.findByText('Sharing updated.');
  };
  const profilesHome = async () => {
    await user.click(screen.getByRole('button', { name: 'Account menu for research-owner' }));
    await user.click(screen.getByRole('menuitem', { name: 'Profiles' }));
    await screen.findByRole('heading', { name: 'Profiles' });
  };
  await startProfile('Agent');
  fireEvent.change(screen.getByLabelText('Model'), { target: { value: 'gpt-5' } });
  await user.click(screen.getByRole('radio', { name: 'High' }));
  await user.click(screen.getByRole('radio', { name: 'Autonomous' }));
  fireEvent.change(screen.getByLabelText('Search Ornn skills'), { target: { value: 'research' } });
  await user.click(await screen.findByRole('button', { name: 'Add Research' }));
  await user.click(screen.getByRole('button', { name: 'Version for research' }));
  await user.click(screen.getByRole('menuitemradio', { name: '1.1' }));
  fireEvent.change(screen.getByLabelText('System prompt'), {
    target: { value: 'Inspect trureturing, explain findings, and verify changes with tests.' },
  });
  await publishAndShare('agent');
  await profilesHome();
  await startProfile('Workspace');
  fireEvent.change(screen.getByLabelText('Repository URL'), { target: { value: REPOSITORY } });
  fireEvent.change(screen.getByLabelText('Branch or tag'), { target: { value: 'main' } });
  expect(screen.getByLabelText('Directory name')).toHaveValue('trureturing');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(test.records.has(PROFILE_IDS.workspace)).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Verify repository' }));
  await screen.findByText('Repository verified');
  await user.click(screen.getByRole('button', { name: 'Add command' }));
  fireEvent.change(screen.getByLabelText('Setup command 1'), { target: { value: 'npm ci' } });
  await user.click(await screen.findByRole('button', { name: 'Add trureturing agent' }));
  await publishAndShare('workspace');
  await profilesHome();
  await startProfile('Sandbox');
  await waitFor(() => expect(screen.getByRole('radio', { name: 'Custom' })).toBeEnabled());
  await user.click(screen.getByRole('radio', { name: 'Custom' }));
  fireEvent.change(screen.getByLabelText('CPU cores'), { target: { value: '3' } });
  fireEvent.change(screen.getByLabelText('Memory (GiB)'), { target: { value: '6' } });
  await user.click(await screen.findByRole('checkbox', { name: 'GitHub' }));
  await user.click(screen.getByRole('radio', { name: 'Your own account' }));
  fireEvent.change(screen.getByLabelText('Maximum workspaces'), { target: { value: '2' } });
  fireEvent.change(screen.getByLabelText('Maximum agents per workspace'), {
    target: { value: '3' },
  });
  await user.click(await screen.findByRole('button', { name: 'Add trureturing workspace' }));
  await user.click(screen.getByRole('button', { name: 'Add command' }));
  fireEvent.change(screen.getByLabelText('Install command 1'), {
    target: { value: 'mkdir -p /home/cma/tools' },
  });
  await user.click(screen.getByRole('button', { name: 'Add variable' }));
  fireEvent.change(screen.getByLabelText('Variable 1 name'), {
    target: { value: 'RESEARCH_MODE' },
  });
  fireEvent.change(screen.getByLabelText('Variable 1 value'), { target: { value: 'review' } });
  await publishAndShare('sandbox');
  expect(test.records.get(PROFILE_IDS.agent)?.draft).toEqual({
    model: 'gpt-5',
    thinking_effort: 'high',
    permission_level: 'autonomous',
    skills: [{ source: 'ornn', id: 'research', version: '1.1' }],
    system_prompt: 'Inspect trureturing, explain findings, and verify changes with tests.',
  });
  expect(test.records.get(PROFILE_IDS.workspace)?.draft).toEqual({
    location: { type: 'git_repository', url: REPOSITORY, reference: 'main' },
    name: 'trureturing',
    setup_commands: ['npm ci'],
    agent_profiles: [{ id: PROFILE_IDS.agent, revision: 1 }],
  });
  const sandbox = test.records.get(PROFILE_IDS.sandbox);
  expect(sandbox?.draft).toEqual({
    cpu_millis: 3000,
    memory_mib: 6144,
    nyxid_services: ['github'],
    agent_provider: 'codex',
    model_access: 'own_credentials',
    max_workspaces: 2,
    max_agents_per_workspace: 3,
    workspace_profiles: [{ id: PROFILE_IDS.workspace, revision: 1 }],
    software_install_commands: ['mkdir -p /home/cma/tools'],
    environment_variables: [{ name: 'RESEARCH_MODE', value: 'review' }],
  });
  expect(sandbox?.visibility).toBe('public');
  first.unmount();
  test.render('second-owner', '/marketplace');
  await user.click(await screen.findByRole('link', { name: 'trureturing sandbox' }));
  await screen.findByRole('heading', { name: 'trureturing sandbox', level: 1 });
  expect(screen.getByText('mkdir -p /home/cma/tools')).toBeInTheDocument();
  expect(screen.getByText('npm ci')).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Fork to my profiles' }));
  await waitFor(() =>
    expect(window.location.pathname).toBe(`/profiles/sandbox-profiles/${FORK_ID}`),
  );
  await screen.findByRole('heading', { name: 'trureturing sandbox', level: 1 });
  expect(test.records.get(FORK_ID)).toMatchObject({
    owner: 'second-owner',
    visibility: 'private',
    draft_version: 1,
    forked_from: { id: PROFILE_IDS.sandbox, revision: 1 },
    draft: sandbox?.draft,
  });
  expect(test.writes()).toHaveLength(11);
  expect(new Set(test.writes().map((request) => request.headers.get('idempotency-key'))).size).toBe(
    11,
  );
  expect(test.writes().every((request) => request.headers.has('idempotency-key'))).toBe(true);
  const resolution = test.requests.find((request) =>
    request.url.pathname.endsWith('/repositories/resolve'),
  );
  expect(resolution?.body).toEqual({ url: REPOSITORY, reference: 'main' });
  expect(resolution?.headers.has('idempotency-key')).toBe(false);
  expect(
    test
      .writes()
      .filter((request) => request.url.pathname.endsWith('/revisions'))
      .map((request) => request.body),
  ).toEqual([{ draft_version: 2 }, { draft_version: 1 }, { draft_version: 1 }]);
  expect(
    test
      .writes()
      .find(
        (request) =>
          request.method === 'PUT' &&
          request.url.pathname === `/api/v1/agent-profiles/${PROFILE_IDS.agent}`,
      )?.body,
  ).toMatchObject({
    draft_version: 1,
    description: 'Published research instructions for trureturing.',
  });
  expect(test.writes().at(-1)?.headers.get('authorization')).toBe('Bearer second-owner');
  expect(test.writes().at(-1)?.body).toEqual({ revision: 1 });
  expect(test.unexpected).toEqual([]);
}, 20_000);
