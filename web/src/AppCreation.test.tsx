import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { CmaClientProvider } from './api/context';
import { agentClient } from './test/agent';
import { creationClient } from './test/creation';
import { sandboxFixture } from './test/hierarchy';
import {
  documentClient,
  OWN_PROFILES,
  AGENT_PROFILE,
  WORKSPACE_PROFILE,
  SANDBOX_PROFILE,
  marketplaceClient,
} from './test/profiles';

afterEach(() => window.history.replaceState({}, '', '/'));

it('binds the production App create command and opens the created Sandbox route', async () => {
  const user = userEvent.setup();
  const createSandbox = vi.fn().mockResolvedValue(sandboxFixture('provisioning'));
  const client = agentClient({ createSandbox });
  render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={client} creation={creationClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  await user.click(await screen.findByRole('button', { name: 'New sandbox' }));
  await user.click(await screen.findByRole('button', { name: /Research sandbox Version/ }));
  await user.click(await screen.findByRole('checkbox', { name: 'I trust this profile' }));
  await user.click(screen.getByRole('button', { name: 'Create sandbox' }));
  await waitFor(() => expect(window.location.pathname).toBe('/sandboxes/sbx_1'));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(await screen.findByRole('heading', { name: 'Atlas' })).toBeInTheDocument();
});

it('hands the reviewed Marketplace revision to creation without silently accepting its commands', async () => {
  window.history.replaceState({}, '', '/marketplace/sandbox/sbp_research');
  const user = userEvent.setup();
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
  await user.click(await screen.findByRole('button', { name: 'Use' }));
  const dialog = await screen.findByRole('dialog', { name: 'New sandbox' });
  expect(within(dialog).getByRole('checkbox', { name: 'I trust this profile' })).not.toBeChecked();
  expect(within(dialog).getByText('mkdir -p /home/cma/tools')).toBeInTheDocument();
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
