import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { CmaClientProvider } from './api/context';
import { agentClient } from './test/agent';
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

afterEach(() => window.history.replaceState({}, '', '/'));

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
  await actor.click(await screen.findByRole('button', { name: 'Settings' }));
  await actor.click(screen.getByRole('menuitem', { name: 'Manage environments' }));
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
