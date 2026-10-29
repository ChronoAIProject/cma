import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { now, transport } from './resource-fixtures.js';

const association = {
  binding_id: 'binding-one',
  binding_generation: 1,
  provider_conversation_id: 'existing_agent',
  relationship: 'borrowed',
  sandbox_id: 'sandbox-1',
  workspace_id: 'workspace-1',
  worktree_id: 'worktree-1',
  conversation_state: 'retired',
  binding_state: 'ready',
  current_binding_generation: 3,
  is_current_generation: false,
  association_created_at: now,
  association_updated_at: now,
};
const page = { agent_id: 'agt_one', owner: 'owner', agent_created_at: now, items: [association] };

describe('retained provider associations', () => {
  it('preserves borrowed retirement and old generation identity while paging', async () => {
    const test = transport();
    test.answer({ ...page, next: 'opaque_cursor' });
    const first = await test.client.agents.providerAssociations('agt_one', { limit: 1 });
    expect(first.items[0]).toEqual(association);
    test.answer({ ...page, items: [] });
    await test.client.agents.providerAssociations('agt_one', { limit: 1, cursor: first.next });
    expect(new URL(test.calls.at(-1)!.url).pathname).toBe(
      '/api/v1/agents/agt_one/provider-associations',
    );
    expect(Object.fromEntries(new URL(test.calls.at(-1)!.url).searchParams)).toEqual({
      limit: '1',
      cursor: 'opaque_cursor',
    });
  });
  it('refuses wrong Agent identity, repeated membership, invalid generations and stalled cursors', async () => {
    for (const value of [
      { ...page, agent_id: 'foreign' },
      { ...page, items: [association, association] },
      { ...page, items: [{ ...association, is_current_generation: true }] },
      { ...page, items: [{ ...association, binding_generation: 4 }] },
      { ...page, items: [{ ...association, relationship: 'unknown' }] },
      { ...page, items: [], next: 'opaque_cursor' },
    ]) {
      const test = transport();
      test.answer(value);
      await expect(test.client.agents.providerAssociations('agt_one')).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    const test = transport();
    test.answer({ ...page, next: 'same' });
    await expect(
      test.client.agents.providerAssociations('agt_one', { limit: 1, cursor: 'same' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('validates request bounds without sending an invalid request', async () => {
    for (const options of [
      { limit: 0 },
      { limit: 201 },
      { cursor: '' },
      { cursor: 'x'.repeat(4097) },
      { cursor: 'a/b' },
    ]) {
      const test = transport();
      await expect(async () =>
        test.client.agents.providerAssociations('agt_one', options),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(test.calls).toHaveLength(0);
    }
  });
});
