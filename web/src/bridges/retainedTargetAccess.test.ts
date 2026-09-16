import { expect, it, vi } from 'vitest';
import { loadRetainedTarget } from './retainedTargetAccess';
import { placementTransport } from './placement.test-support';
import {
  organizationAccess,
  organizationResponse,
  organizationTarget,
  organizationWorkspace,
} from './organization.fixture';

function fixture(override?: Parameters<typeof placementTransport>[1]) {
  return placementTransport('org-one', (url, init) => {
    const result = override?.(url, init);
    if (result !== undefined) return result;
    const value = organizationResponse(url);
    return value === undefined ? undefined : Response.json(value);
  });
}
function read(test: ReturnType<typeof fixture>) {
  return loadRetainedTarget(
    test.client,
    organizationTarget,
    'org-one',
    new AbortController().signal,
  );
}

it.each([
  { ...organizationAccess, owner: { kind: 'person', is_current_person: true } },
  { ...organizationAccess, owner: { kind: 'organization', organization_id: 'other' } },
  { ...organizationAccess, capabilities: ['view_details', 'interact'] },
  { ...organizationAccess, capabilities: ['view_details', 'manage_bridge'] },
  { ...organizationAccess, revalidate_at: '2020-01-01T00:00:00Z' },
])(
  'refuses wrong owner, missing current capability or expired authority before hierarchy disclosure: %j',
  async (access) => {
    const test = fixture((url) =>
      url.pathname.includes('/resource-access/agent/') ? Response.json(access) : undefined,
    );
    await expect(read(test)).rejects.toThrow();
    expect(test.fetch.mock.calls).toHaveLength(1);
  },
);

it.each([
  {
    ...organizationWorkspace,
    agents: [{ ...organizationWorkspace.agents[0]!, sandbox_id: 'other' }],
  },
  {
    ...organizationWorkspace,
    agents: [...organizationWorkspace.agents, ...organizationWorkspace.agents],
  },
  {
    ...organizationWorkspace,
    worktrees: [{ ...organizationWorkspace.worktrees[0]!, agent_id: 'other' }],
  },
])(
  'rejects changed or ambiguous exact child ancestry during the fresh read: %j',
  async (workspace) => {
    const test = fixture((url) =>
      url.pathname.endsWith('/workspaces/workspace-1') ? Response.json(workspace) : undefined,
    );
    await expect(read(test)).rejects.toThrow('exact hierarchy');
    expect(
      test.fetch.mock.calls.some(
        ([url]) => new URL(String(url)).pathname === '/api/v1/agents/agent-1',
      ),
    ).toBe(false);
  },
);

it('rejects a selected-Agent policy change while target details are read', async () => {
  let reads = 0;
  const test = fixture((url) =>
    url.pathname.includes('/resource-access/agent/')
      ? Response.json({ ...organizationAccess, policy_revision: ++reads })
      : undefined,
  );
  await expect(read(test)).rejects.toThrow('Agent access changed');
});

it('does not extend the original authority lifetime through a slow channel-use read', async () => {
  const started = Date.now();
  const now = vi.spyOn(Date, 'now').mockReturnValue(started);
  const test = fixture((url) => {
    if (url.pathname.endsWith('/channel-usage')) now.mockReturnValue(started + 56_000);
    return undefined;
  });
  try {
    await expect(read(test)).rejects.toThrow();
  } finally {
    now.mockRestore();
  }
});

it('keeps unavailable channel marking separate from current target authority', async () => {
  const test = fixture((url) =>
    url.pathname.endsWith('/channel-usage') ? new Response(null, { status: 503 }) : undefined,
  );
  await expect(read(test)).resolves.toMatchObject({
    value: { model: undefined, services: undefined, channelUsage: undefined },
  });
});
