import { render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { CmaClientProvider } from './api/context';
import { agentClient } from './test/agent';
import { creationClient } from './test/creation';
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
  await screen.findByRole('heading', { name: 'Welcome to CMA' });
  expect(screen.getAllByRole('button', { name: 'New agent' }).length).toBeGreaterThan(0);
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
  await screen.findByRole('heading', { name: 'Welcome to CMA' });
  expect(screen.queryByRole('button', { name: 'New sandbox' })).not.toBeInTheDocument();
});
import './test/connectedAccounts';
