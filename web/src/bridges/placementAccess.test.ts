import { expect, it } from 'vitest';
import { loadPlacementAccess } from './placementAccess';
import { placementTransport } from './placement.test-support';
import { WIRE_WORKSPACE_DETAIL } from '../test/rest';

const authorized = {
  owner: { kind: 'person', is_current_person: true },
  capabilities: ['view_details', 'manage_sandbox'],
  revalidate_at: null,
  policy_revision: 1,
};

it.each([
  { ...authorized, owner: { kind: 'person', is_current_person: false } },
  { ...authorized, owner: { kind: 'organization', organization_id: 'other' } },
  { ...authorized, owner: undefined },
  { ...authorized, capabilities: ['view_details'] },
  { ...authorized, revalidate_at: '2020-01-01T00:00:00Z' },
])(
  'does not accept visible resources without exact current owner/management authority: %j',
  async (access) => {
    const test = placementTransport(undefined, (url) =>
      url.pathname.includes('/resource-access/') ? Response.json(access) : undefined,
    );
    await expect(
      loadPlacementAccess(
        test.client,
        'sandbox-1',
        undefined,
        undefined,
        new AbortController().signal,
      ),
    ).rejects.toThrow();
    expect(
      test.fetch.mock.calls.some(
        ([url]) => new URL(String(url)).pathname === '/api/v1/sandboxes/sandbox-1',
      ),
    ).toBe(false);
  },
);

it('requires the selected organization rather than personal identity or another organization', async () => {
  for (const owner of [
    { kind: 'person', is_current_person: true },
    { kind: 'organization', organization_id: 'other' },
  ]) {
    const test = placementTransport('org-one', (url) =>
      url.pathname.includes('/resource-access/')
        ? Response.json({ ...authorized, owner })
        : undefined,
    );
    await expect(
      loadPlacementAccess(
        test.client,
        'sandbox-1',
        'workspace-1',
        'org-one',
        new AbortController().signal,
      ),
    ).rejects.toThrow();
  }
  const test = placementTransport('org-one');
  await expect(
    loadPlacementAccess(
      test.client,
      'sandbox-1',
      'workspace-1',
      'org-one',
      new AbortController().signal,
    ),
  ).resolves.toMatchObject({ value: true });
});

it('rejects changed policy after the detail read and a Workspace from a different Sandbox', async () => {
  let reads = 0;
  const policy = placementTransport(undefined, (url) =>
    url.pathname.includes('/resource-access/')
      ? Response.json({ ...authorized, policy_revision: ++reads })
      : undefined,
  );
  await expect(
    loadPlacementAccess(
      policy.client,
      'sandbox-1',
      undefined,
      undefined,
      new AbortController().signal,
    ),
  ).rejects.toThrow('Resource access changed');
  const ancestry = placementTransport(undefined, (url) =>
    url.pathname.endsWith('/workspaces/workspace-1')
      ? Response.json({ ...WIRE_WORKSPACE_DETAIL, sandbox_id: 'other-sandbox' })
      : undefined,
  );
  await expect(
    loadPlacementAccess(
      ancestry.client,
      'sandbox-1',
      'workspace-1',
      undefined,
      new AbortController().signal,
    ),
  ).rejects.toThrow('belong to this Sandbox');
});
