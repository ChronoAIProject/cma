import { describe, expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { agent, now, transport } from './resource-fixtures.js';

describe('immutable Agent provenance', () => {
  it('reads categories and authorized attribution with an exact Agent identity', async () => {
    const test = transport();
    for (const creation_origin of ['manual', 'trigger', 'channel_bridge', 'legacy_unknown']) {
      const category = { agent_id: agent.id, created_at: now, creation_origin };
      test.answer(category);
      expect(await test.client.agents.provenance(agent.id)).toEqual(category);
      expect(test.calls.at(-1)?.url).toContain(`/agents/${agent.id}/provenance`);
    }
    const actor = { kind: 'api', subject: 'actual-creator', delegation_id: null };
    const detailed = {
      agent_id: agent.id,
      created_at: now,
      creation_origin: 'trigger',
      details: {
        created_by: actor,
        trigger: {
          trigger_id: 'original-trigger',
          launch_id: 'original-launch',
          launching_actor: actor,
        },
        channel_bridge: null,
        workspace_profile_index: null,
      },
    };
    test.answer(detailed);
    expect(await test.client.agents.provenance(agent.id)).toEqual(detailed);
    for (const malformed of [
      { ...detailed, agent_id: 'another-agent' },
      { ...detailed, creation_origin: 'future' },
      { ...detailed, details: { ...detailed.details, created_by: { kind: 'future' } } },
      { ...detailed, details: { ...detailed.details, trigger: { trigger_id: 'missing-launch' } } },
    ]) {
      test.answer(malformed);
      await expect(test.client.agents.provenance(agent.id)).rejects.toBeInstanceOf(ProtocolError);
    }
  });
});
