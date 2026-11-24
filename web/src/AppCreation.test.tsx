import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { CmaClientProvider } from './api/context';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { connectionClient } from './connections/client';
import { lifecycleStorageKey } from './create/lifecycleIntent';
import { creationClient } from './test/creation';
import { taskExperience } from './test/taskExperience';
import {
  AGENT_PROFILE,
  OWN_PROFILES,
  SANDBOX_PROFILE,
  WORKSPACE_PROFILE,
  documentClient,
  marketplaceClient,
} from './test/profiles';

afterEach(() => {
  window.history.replaceState({}, '', '/');
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it('discards a GitHub-waiting task across history, reload, and New agent from another Agent', async () => {
  window.history.replaceState({}, '', '/agents/agt_1');
  const target = {
    type: 'sandbox_profile',
    profile: { id: 'profile-owned', revision: 1 },
  } as const;
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'mock-subject',
    ready: false,
    checked_at: '2026-11-24T00:00:00Z',
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  });
  const create = vi
    .fn<NonNullable<ReturnType<typeof agentClient>['createAgentLifecycle']>>()
    .mockImplementation(async (body) => ({
      id: body.repository ? 'repository-operation' : 'independent-operation',
      status: 'queued',
    }));
  const client = agentClient({
    createAgentLifecycle: create,
    taskExperience: taskExperience('mock-subject'),
    readTaskCreationSupport: async () => true,
    readTaskRepositorySupport: async () => true,
    readAgentLifecycle: async (id) =>
      id === 'repository-operation'
        ? {
            id,
            status: 'waiting_for_connection',
            phase: 'sandbox_ready',
            connection: { services: ['api-github'], target },
            events: [],
          }
        : { id, status: 'queued', phase: 'admission', events: [] },
  });
  const app = () => (
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={client} creation={creationClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>
  );
  const actor = userEvent.setup();
  const newAgent = () =>
    within(screen.getByRole('complementary', { name: 'Application navigation' })).getByRole(
      'button',
      { name: 'New agent' },
    );
  const first = render(app());
  await screen.findByRole('heading', { name: AGENT_DETAIL.title });
  await actor.click(newAgent());
  const composer = await screen.findByLabelText('Your task');
  fireEvent.change(composer, { target: { value: 'Inspect the repository' } });
  await actor.click(screen.getByRole('button', { name: 'Add git repo' }));
  fireEvent.change(screen.getByLabelText('GitHub repository'), {
    target: { value: 'https://github.com/example/project' },
  });
  await actor.click(screen.getByRole('button', { name: 'Add repo' }));
  const start = screen.getByRole('button', { name: 'Start task' });
  await waitFor(() => expect(start).toBeEnabled());
  await actor.click(start);

  await actor.click(await screen.findByRole('button', { name: 'Continue original task' }));
  const wizard = await screen.findByRole('dialog', { name: 'Connect your accounts' });
  expect(check).toHaveBeenCalled();
  await actor.click(within(wizard).getByRole('button', { name: 'Close' }));

  const storageKey = lifecycleStorageKey('mock-subject');
  const original = sessionStorage.getItem(storageKey);
  expect(original).toContain('repository-operation');
  await actor.click(screen.getByRole('button', { name: 'Discard task' }));
  const confirmation = screen.getByText('Discard this saved task and start over in this tab?');
  expect(confirmation).toBeVisible();
  await actor.click(screen.getByRole('button', { name: 'Keep it' }));
  expect(sessionStorage.getItem(storageKey)).toBe(original);
  expect(screen.getByRole('button', { name: 'Continue original task' })).toBeVisible();
  await actor.click(screen.getByRole('button', { name: 'Discard task' }));
  await actor.click(screen.getByRole('button', { name: 'Discard task' }));

  const expectFreshForm = async () => {
    expect(await screen.findByLabelText('Your task')).toHaveValue('');
    expect(screen.queryByRole('region', { name: 'Submitted task' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add git repo' })).toBeVisible();
    expect(sessionStorage.getItem(storageKey)).toBeNull();
  };
  await expectFreshForm();
  window.history.back();
  await screen.findByRole('heading', { name: AGENT_DETAIL.title });
  window.history.forward();
  await expectFreshForm();
  window.history.back();
  await screen.findByRole('heading', { name: AGENT_DETAIL.title });
  await actor.click(newAgent());
  await expectFreshForm();
  first.unmount();
  render(app());
  await expectFreshForm();

  fireEvent.change(screen.getByLabelText('Your task'), { target: { value: 'Write an outline' } });
  const next = screen.getByRole('button', { name: 'Start task' });
  await waitFor(() => expect(next).toBeEnabled());
  await actor.click(next);
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1]?.[0]).toEqual({
    name: 'New task',
    initial_message: 'Write an outline',
  });
  expect(create.mock.calls[1]?.[1].idempotencyKey).not.toBe(
    create.mock.calls[0]?.[1].idempotencyKey,
  );
});

it('retires the profile-driven Sandbox creation entry from the Agent Manager home', async () => {
  window.history.replaceState({}, '', '/agent-manager');
  render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={agentClient()} creation={creationClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await screen.findByRole('heading', { name: 'What should your Agent work on?' });
  expect(
    within(screen.getByRole('complementary', { name: 'Application navigation' })).getByRole(
      'button',
      { name: 'New agent' },
    ),
  ).toHaveAttribute('aria-current', 'page');
  expect(screen.queryByRole('button', { name: 'New sandbox' })).not.toBeInTheDocument();
});

it('keeps Marketplace profile revisions read-only while creation moves to Agent Manager', async () => {
  window.history.replaceState({}, '', '/marketplace/sandbox/sbp_research');
  const readMarketplace = vi.fn(marketplaceClient().readMarketplace);
  const profiles = {
    profiles: OWN_PROFILES,
    agent: documentClient(AGENT_PROFILE),
    workspace: documentClient(WORKSPACE_PROFILE),
    sandbox: documentClient(SANDBOX_PROFILE),
    marketplace: marketplaceClient({ readMarketplace }),
  };
  render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={agentClient()} profiles={profiles} creation={creationClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await screen.findByRole('heading', { name: 'Configure Agents directly' });
  expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
  expect(screen.getByText(/Agent setup is now saved inline/)).toBeInTheDocument();
  expect(readMarketplace).not.toHaveBeenCalled();
});

it('does not offer Sandbox creation without its distinct capability', async () => {
  window.history.replaceState({}, '', '/agent-manager');
  render(
    <AuthContextProvider
      provider={createMockAuthProvider({
        initiallyAuthenticated: true,
        capabilities: ['use_own_sandboxes'],
      })}
    >
      <CmaClientProvider client={agentClient()} creation={creationClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await screen.findByRole('heading', { name: 'What should your Agent work on?' });
  expect(screen.queryByRole('button', { name: 'New sandbox' })).not.toBeInTheDocument();
});
it('creates the Agent in the Workspace whose row opened New agent', async () => {
  window.history.replaceState({}, '', '/agent-manager');
  const createAgentLifecycle = vi.fn().mockRejectedValue(new Error('Lost create response'));
  render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider
        client={agentClient({
          createAgentLifecycle,
          readTaskCreationSupport: async () => true,
          taskExperience: taskExperience('mock-subject'),
        })}
        creation={creationClient()}
      >
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  const actor = userEvent.setup();
  await actor.click(
    within(await screen.findByRole('complementary', { name: 'Application navigation' })).getByRole(
      'button',
      { name: 'Environments' },
    ),
  );
  await actor.click(
    await screen.findByRole('button', { name: 'Options for /workspaces/chrono-bot' }),
  );
  await actor.click(screen.getByRole('menuitem', { name: 'New agent' }));
  const dialog = await screen.findByRole('dialog', { name: 'New task' });
  expect(within(dialog).getByText('Creating in Workspace wsp_1')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Add git repo' })).toBeDisabled();
  expect(within(dialog).getByRole('button', { name: 'Add git repo' })).toHaveAccessibleDescription(
    /Repository context is inherited/,
  );
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  await actor.type(within(dialog).getByLabelText('Your task'), 'Read the selected workspace');
  await actor.click(within(dialog).getByRole('button', { name: 'Start task' }));
  await waitFor(() => expect(createAgentLifecycle).toHaveBeenCalledOnce());
  expect(createAgentLifecycle.mock.calls[0]?.[0]).toEqual({
    name: 'New task',
    initial_message: 'Read the selected workspace',
    workspace_id: 'wsp_1',
  });
});

import './test/connectedAccounts';
