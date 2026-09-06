import { expect, it } from 'vitest';
import { createCreationClient, fromMarketplace, fromProfileUse } from './creation';
import { restTransport, wireProfile, wireRevision } from '../test/rest';
import { MARKETPLACE_SANDBOX } from '../test/profiles';

it('resolves exact deployment-default and nested revisions without Marketplace discovery', async () => {
  const t = restTransport();
  const client = createCreationClient(t.sdk);
  t.reply({
    sandbox_profile: { id: 'sandbox-1', revision: 3 },
    workspace_profile: { id: 'workspace-1', revision: 2 },
    agent_profile: { id: 'agent-1', revision: 4 },
  });
  const page = await client.listProfiles('sandbox', 'default');
  expect(page.items[0]).toMatchObject({ id: 'sandbox-1', revision: 3, source: 'default' });
  t.reply({ ...wireRevision('sandbox'), revision: 3 });
  t.reply(wireRevision('workspace'));
  const profile = await client.readProfile('sandbox', page.items[0]!);
  expect(profile.revision.revision).toBe(3);
  expect(profile.disclosures).toHaveLength(2);
  expect(profile.requiresConfirmation).toBe(true);
  expect(t.fetch.mock.calls.map(([path]) => path)).toEqual([
    '/api/v1/defaults',
    '/api/v1/sandbox-profiles/sandbox-1/revisions/3',
    '/api/v1/workspace-profiles/workspace-1/revisions/1',
  ]);
});

it('excludes unpublished and retired drafts from creation choices while retaining pagination', async () => {
  const t = restTransport();
  t.reply({
    items: [
      wireProfile('agent'),
      { ...wireProfile('agent'), id: 'unpublished', latest_revision: undefined },
      { ...wireProfile('agent'), id: 'retired', retired_at: '2026-09-06T00:00:00Z' },
    ],
    next: 'page-two',
  });
  const page = await createCreationClient(t.sdk).listProfiles('agent', 'own');
  expect(page.items).toHaveLength(1);
  expect(page.next).toBe('page-two');
});

it('reads immutable own agent content instead of its mutable draft', async () => {
  const t = restTransport();
  t.reply(wireRevision('agent'));
  const profile = await createCreationClient(t.sdk).readProfile('agent', {
    id: 'agent-1',
    revision: 1,
    name: 'Research agent',
    source: 'own',
  });
  expect(profile.requiresConfirmation).toBe(false);
  expect(profile.revision.content).toMatchObject({ thinkingEffort: 'high' });
});

it('preserves a Marketplace handoff and refuses identity or trust-digest disagreement', () => {
  const selection = {
    kind: 'sandbox' as const,
    profile: { id: MARKETPLACE_SANDBOX.revision.profileId, revision: 1 },
    entry: MARKETPLACE_SANDBOX,
  };
  const profile = fromProfileUse(selection);
  expect(profile.revision).toBe(MARKETPLACE_SANDBOX.revision);
  expect(profile.disclosures.map((item) => item.commands)).toEqual([
    ['mkdir -p /home/cma/tools'],
    ['mkdir -p reports'],
  ]);
  expect(() =>
    fromProfileUse({ ...selection, profile: { ...selection.profile, revision: 2 } }),
  ).toThrow('selection changed');
  expect(() =>
    fromMarketplace({
      ...(MARKETPLACE_SANDBOX as Extract<typeof MARKETPLACE_SANDBOX, { kind: 'sandbox' }>),
      trust: { requiresConfirmation: true, commandsDigest: 'different' },
    }),
  ).toThrow('trust digest');
});
