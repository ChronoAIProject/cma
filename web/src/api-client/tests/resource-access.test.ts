// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { createClient } from '../index.js';

const policy = { capabilities: ['view_details'], revalidate_at: null, policy_revision: 1 };

describe('effective resource ownership', () => {
  it.each([
    undefined,
    { kind: 'person', is_current_person: false },
    { kind: 'person', is_current_person: true },
    { kind: 'organization', organization_id: 'org' },
  ])('reads current safe ownership while accepting older servers: %j', async (owner) => {
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async () => Response.json({ ...policy, owner }),
    });
    expect((await client.resourceAccess.effective('agent', 'agent')).owner).toEqual(owner);
  });

  it.each([
    { kind: 'person', subject: 'private-person' },
    { kind: 'person', is_current_person: 'true' },
    { kind: 'organization' },
    { kind: 'future' },
  ])('rejects malformed ownership without guessing resource permissions: %j', async (owner) => {
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async () => Response.json({ ...policy, owner }),
    });
    await expect(client.resourceAccess.effective('agent', 'agent')).rejects.toThrow();
  });
});
