import { render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { CmaClientProvider } from './api/context';
import { agentClient } from './test/agent';
import { creationClient } from './test/creation';
import {
  documentClient,
  OWN_PROFILES,
  AGENT_PROFILE,
  WORKSPACE_PROFILE,
  SANDBOX_PROFILE,
  marketplaceClient,
} from './test/profiles';

afterEach(() => window.history.replaceState({}, '', '/'));

it('retires the profile-driven Sandbox creation entry from the Agent Manager home', async () => {
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
  const catalog = creationClient();
  const readProfile = vi.spyOn(catalog, 'readProfile');
  const profiles = {
    profiles: OWN_PROFILES,
    agent: documentClient(AGENT_PROFILE),
    workspace: documentClient(WORKSPACE_PROFILE),
    sandbox: documentClient(SANDBOX_PROFILE),
    marketplace: marketplaceClient({ readMarketplace }),
  };
  render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={agentClient()} profiles={profiles} creation={catalog}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await screen.findByRole('heading', { name: 'Research sandbox' });
  expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
  expect(screen.getByText(/Profile based creation is retired/)).toBeInTheDocument();
  expect(screen.getByText('mkdir -p /home/cma/tools')).toBeInTheDocument();
  expect(readMarketplace).toHaveBeenCalledTimes(1);
  expect(readProfile).not.toHaveBeenCalled();
});

it('does not offer Sandbox creation without its distinct capability', async () => {
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
