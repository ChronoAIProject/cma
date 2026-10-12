import { render, screen } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { Console } from './Console';
import { CmaClientProvider } from './api/context';
import { createRestWebClient } from './api/rest';
import { createProfileViewsClient } from './api/profiles';
import type { AppRoute } from './routing/routes';
import { wireMarketplace, wireProfile } from './test/rest';

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
    if (path === '/api/v1/sandboxes') return Response.json({ items: [] });
    if (path === '/api/v1/profile-authoring/skills') return Response.json({ items: [] });
    if (path === '/api/v1/profile-authoring/services') return Response.json([]);
    if (path === '/api/v1/profile-authoring/bounds')
      return Response.json({
        min_cpu_millis: 1000,
        max_cpu_millis: 8000,
        min_memory_mib: 1024,
        max_memory_mib: 16384,
      });
    const match =
      /^\/api\/v1\/(marketplace\/)?(agent|workspace|sandbox)-profiles(?:\/[^/]+)?$/.exec(path);
    const kind = match?.[2];
    if (kind !== 'agent' && kind !== 'workspace' && kind !== 'sandbox')
      throw new Error(`Unexpected test route ${path}`);
    const collection = path.endsWith('-profiles');
    const profile = match?.[1] === undefined ? wireProfile(kind) : wireMarketplace(kind);
    return Response.json(
      collection
        ? { items: [match?.[1] === undefined ? profile : wireMarketplace(kind).summary] }
        : profile,
    );
  });
  const sdk = createClient({ fetch });
  return { client: createRestWebClient(sdk), profiles: createProfileViewsClient(sdk), fetch };
}

it.each(cases)(
  'mounts the real public-client profile route $route.kind / $heading',
  async ({ route, heading }) => {
    const clients = publicClient();
    render(
      <CmaClientProvider client={clients.client} profiles={clients.profiles}>
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
    expect(await screen.findByRole('heading', { name: heading, level: 1 })).toBeInTheDocument();
    if (route.kind === 'marketplace_profile') {
      expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
      expect(screen.getByText(/Profile based creation is retired/)).toBeInTheDocument();
    }
    expect(clients.fetch.mock.calls.every(([url]) => String(url).startsWith('/api/v1/'))).toBe(
      true,
    );
  },
);

it('does not permit profile management without the public capability', async () => {
  const clients = publicClient();
  render(
    <CmaClientProvider client={clients.client} profiles={clients.profiles}>
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
    <CmaClientProvider client={clients.client} profiles={clients.profiles}>
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
  expect(clients.fetch.mock.calls.some(([url]) => /profile|marketplace/.test(String(url)))).toBe(
    false,
  );
});
import './test/connectedAccounts';
