import { describe, expect, it } from 'vitest';
import {
  ProtocolError,
  ValidationError,
  type AgentCreationCategory,
  type AgentTaskStatus,
} from '../src/index.js';
import { agent, now, transport } from './resource-fixtures.js';

describe('immutable Agent provenance', () => {
  it('rejects an unsupported task filter before sending a list request', () => {
    const test = transport();
    test.answer({ items: [] });
    expect(() =>
      test.client.agents.provenanceInventory({ taskStatus: 'ready' as AgentTaskStatus }),
    ).toThrow(ValidationError);
  });
  it('selects aggregate explicit shares and keeps continuation filters', async () => {
    const test = transport();
    test.answer({ items: [], next: 'agt_scanned' });
    const first = await test.client.agents.provenanceInventory({
      shared: true,
      creationOrigin: 'trigger',
    });
    test.answer({ items: [] });
    await test.client.agents.provenanceInventory({
      shared: true,
      creationOrigin: 'trigger',
      cursor: first.next,
    });
    expect(Object.fromEntries(new URL(test.calls.at(-1)!.url).searchParams)).toEqual({
      shared: 'true',
      creation_origin: 'trigger',
      cursor: 'agt_scanned',
    });
  });
  it('pages safe inventory across empty filtered pages and maps all scope filters', async () => {
    const test = transport();
    test.answer({ items: [], next: 'agt_scanned' });
    const first = await test.client.agents.provenanceInventory({
      organization: 'org',
      sandbox: 'sbx_one',
      workspace: 'wks_one',
      status: 'deleted',
      creationOrigin: 'channel_bridge',
      limit: 2,
    });
    expect(first).toEqual({ items: [], next: 'agt_scanned' });
    const url = new URL(test.calls.at(-1)!.url);
    expect(url.pathname).toBe('/api/v1/agents/provenance');
    expect(Object.fromEntries(url.searchParams)).toEqual({
      organization: 'org',
      sandbox: 'sbx_one',
      workspace: 'wks_one',
      status: 'deleted',
      creation_origin: 'channel_bridge',
      limit: '2',
    });
    const row = {
      agent_id: 'agt_later',
      status: 'deleted',
      created_at: now,
      updated_at: now,
      creation_origin: 'channel_bridge',
    };
    test.answer({ items: [row] });
    expect(
      await test.client.agents.provenanceInventory({
        creationOrigin: 'channel_bridge',
        cursor: first.next,
      }),
    ).toEqual({ items: [row] });
    expect(new URL(test.calls.at(-1)!.url).searchParams.get('cursor')).toBe('agt_scanned');
    test.answer({ items: [{ ...row, creation_origin: 'spoofed' }] });
    await expect(test.client.agents.provenanceInventory()).rejects.toBeInstanceOf(ProtocolError);
    expect(() =>
      test.client.agents.provenanceInventory({
        creationOrigin: 'spoofed' as AgentCreationCategory,
      }),
    ).toThrow(ValidationError);
  });
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
