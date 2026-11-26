import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { RepositoryConnectionRepair } from './ConnectionWizard';

const target = { type: 'sandbox_profile', profile: { id: 'profile', revision: 1 } } as const;
const root = 'https://nyx.example/keys';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

test('shows reported grants for distinct connections and requires an explicit recheck', async () => {
  const recheck = vi.fn();
  const fetch = vi.fn(async () =>
    Response.json({
      owner: 'owner',
      github: {
        manage_url: root,
        connections: [
          {
            id: 'profile-only',
            label: 'Profile account',
            granted_scopes: ['read:user'],
            manage_url: `${root}/profile-only`,
          },
          {
            id: 'repo-access',
            label: 'Repository account',
            granted_scopes: ['repo'],
            manage_url: `${root}/repo-access`,
          },
        ],
      },
    }),
  );
  vi.stubGlobal('fetch', fetch);
  render(
    <RepositoryConnectionRepair
      owner="owner"
      target={target}
      pending={false}
      onRecheck={recheck}
    />,
  );

  expect(
    await screen.findByRole('link', { name: 'Repository account (repo-access)' }),
  ).toHaveAttribute('href', `${root}/repo-access`);
  expect(screen.getByText('Reported scopes: repo')).toBeInTheDocument();
  expect(screen.getByText('Reported scopes: read:user')).toBeInTheDocument();
  expect(screen.getByText('Your request is saved.')).toBeVisible();
  expect(recheck).not.toHaveBeenCalled();
  expect(fetch).toHaveBeenCalledWith(
    '/auth/connections/repository-access',
    expect.objectContaining({ body: JSON.stringify({ target }) }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Recheck repository access' }));
  expect(recheck).toHaveBeenCalledOnce();
});

test.each(['different owner', 'older API', 'invalid link'])(
  'uses general repair for %s',
  async (mode) => {
    const fetch = vi.fn(async () =>
      mode === 'older API'
        ? new Response(null, { status: 404 })
        : Response.json({
            owner: mode === 'different owner' ? 'other' : 'owner',
            github: {
              manage_url: root,
              connections: [
                {
                  id: 'wrong',
                  label: 'Hidden connection',
                  granted_scopes: ['repo'],
                  manage_url: 'https://untrusted.example/keys/wrong',
                },
              ],
            },
          }),
    );
    vi.stubGlobal('fetch', fetch);
    render(
      <RepositoryConnectionRepair
        owner="owner"
        target={target}
        pending={false}
        onRecheck={() => {}}
      />,
    );

    if (mode === 'invalid link') {
      expect(
        await screen.findByRole('link', { name: 'Manage permissions in NyxID' }),
      ).toHaveAttribute('href', root);
    } else {
      expect(await screen.findByText(/In NyxID, open External Services/)).toBeInTheDocument();
    }
    expect(screen.queryByRole('link', { name: /Hidden connection/ })).not.toBeInTheDocument();
  },
);

test('links the insufficient grant to the exact owner connection', () => {
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  render(
    <RepositoryConnectionRepair
      owner="owner"
      target={target}
      repository_access="insufficient_scope"
      repository_scope_repair={{
        connection_id: 'weak',
        operation: 'public_repository_write',
        required_scope: 'public_repo',
        manage_url: root + '/weak',
      }}
      pending={false}
      onRecheck={() => {}}
    />,
  );

  expect(screen.getByText('public_repo')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Manage permissions in NyxID' })).toHaveAttribute(
    'href',
    root + '/weak',
  );
  expect(fetch).not.toHaveBeenCalled();
});
