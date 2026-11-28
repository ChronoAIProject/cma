import { render, screen } from '@testing-library/react';
import { createClient } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { Console } from './Console';
import { CmaClientProvider } from './api/context';
import { createRestWebClient } from './api/rest';
import type { AppRoute } from './routing/routes';

const cases: readonly { route: AppRoute; heading: string }[] = [
  { route: { kind: 'marketplace' }, heading: 'Marketplace' },
  {
    route: { kind: 'marketplace_profile', profileKind: 'sandbox', profileId: 'sandbox-1' },
    heading: 'Research sandbox',
  },
];

function publicClient() {
  const fetch = vi.fn(async (url: string | URL | Request) => {
    const path = new URL(String(url), 'https://cma.example.test').pathname;
    throw new Error(`Unexpected request after profile retirement: ${path}`);
  });
  const sdk = createClient({ fetch });
  return { client: createRestWebClient(sdk), fetch };
}

it.each(cases)(
  'mounts the real public-client profile route $route.kind / $heading',
  async ({ route, heading }) => {
    const clients = publicClient();
    render(
      <CmaClientProvider client={clients.client}>
        <Console
          user={{
            id: 'owner',
            handle: 'Owner',
            visitor: false,
            capabilities: ['manage_own_profiles'],
          }}
          route={route}
          onNavigate={vi.fn()}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
    expect(
      await screen.findByRole('heading', { name: 'Configure Agents directly', level: 1 }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: heading, level: 1 })).not.toBeInTheDocument();
    expect(clients.fetch.mock.calls.some(([url]) => /profile|marketplace/.test(String(url)))).toBe(
      false,
    );
  },
);

it('does not permit profile management without the public capability', async () => {
  const clients = publicClient();
  render(
    <CmaClientProvider client={clients.client}>
      <Console
        user={{ id: 'visitor', handle: 'Visitor', visitor: true, capabilities: [] }}
        route={{ kind: 'profile', profileKind: 'agent', profileId: 'agent-1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(
    await screen.findByRole('heading', { name: 'Configure Agents directly' }),
  ).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Save draft' })).not.toBeInTheDocument();
  expect(clients.fetch.mock.calls.some(([url]) => /profile|marketplace/.test(String(url)))).toBe(
    false,
  );
});

it.each([
  { kind: 'profiles' },
  { kind: 'profile', profileKind: 'agent', profileId: 'agent-1' },
  { kind: 'profile', profileKind: 'workspace', profileId: 'workspace-1' },
  { kind: 'profile', profileKind: 'sandbox', profileId: 'sandbox-1' },
  { kind: 'event_worker_profiles' },
  { kind: 'event_worker_profile', profileId: 'worker-1' },
] as const)('retires the legacy profile route $kind', async (route) => {
  const clients = publicClient();
  render(
    <CmaClientProvider client={clients.client}>
      <Console
        user={{
          id: 'owner',
          handle: 'Owner',
          visitor: false,
          capabilities: ['manage_own_profiles'],
        }}
        route={route}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(
    await screen.findByRole('heading', { name: 'Configure Agents directly' }),
  ).toBeInTheDocument();
  expect(screen.getByText(/New Sandboxes use the deployed runtime image\./)).toBeInTheDocument();
  expect(clients.fetch.mock.calls.some(([url]) => /profile|marketplace/.test(String(url)))).toBe(
    false,
  );
});
import './test/connectedAccounts';
