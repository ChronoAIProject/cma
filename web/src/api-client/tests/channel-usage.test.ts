// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../index.js';
import { now, transport } from './resource-fixtures.js';

const binding = {
  binding_id: 'binding-one',
  bridge_id: 'bridge-one',
  partition_id: 'partition-one',
  generation: 1,
  previous_binding_id: null,
  attached_at: now,
  first_used_at: null,
  last_used_at: null,
};
const summary = { active: true, has_history: true, first_used_at: null, last_used_at: null };
const usage = { agent_id: 'agt_one', channel_usage: summary };
function event(revision = 1, kind = 'attach') {
  return {
    revision,
    actor: { kind: 'human', subject: 'manager' },
    at: now,
    detached_at: kind === 'detach' || kind === 'replace' ? now : null,
    reason:
      kind === 'attach'
        ? 'attachment'
        : kind === 'use'
          ? 'accepted_command'
          : 'administrative_intervention',
    change:
      kind === 'replace'
        ? { kind, before_binding_id: 'previous', binding_id: 'binding-one' }
        : { kind, binding_id: 'binding-one' },
    before: kind === 'attach' ? [] : [binding],
    after: kind === 'detach' ? [] : [binding],
  };
}
describe('explicit Agent channel usage', () => {
  it('filters scoped inventory by explicit channel facts and preserves empty continuations', async () => {
    const test = transport();
    const options = {
      shared: true,
      channelActive: false,
      channelHistory: true,
      creationOrigin: 'manual' as const,
    };
    test.answer({ items: [], next: 'agt_scanned' });
    const first = await test.client.agents.provenanceInventory(options);
    const older = {
      agent_id: 'agt_one',
      status: 'deleted',
      created_at: now,
      updated_at: now,
      creation_origin: 'manual',
    };
    const row = {
      ...older,
      channel_usage: { ...summary, active: false },
    };
    test.answer({ items: [row] });
    expect(
      await test.client.agents.provenanceInventory({ ...options, cursor: first.next }),
    ).toEqual({ items: [row] });
    expect(Object.fromEntries(new URL(test.calls.at(-1)!.url).searchParams)).toEqual({
      shared: 'true',
      creation_origin: 'manual',
      channel_active: 'false',
      channel_history: 'true',
      cursor: 'agt_scanned',
    });
    test.answer({ items: [older] });
    expect(
      (await test.client.agents.provenanceInventory()).items[0]?.channel_usage,
    ).toBeUndefined();
    for (const option of [{ channelActive: 'true' as never }, { channelHistory: 'false' as never }])
      expect(() => test.client.agents.provenanceInventory(option)).toThrow(ValidationError);
    test.answer({ items: [{ ...row, channel_usage: { ...summary, active: 'true' } }] });
    await expect(test.client.agents.provenanceInventory()).rejects.toBeInstanceOf(ProtocolError);
  });
  it('reads shared and manager summaries without inventing creation or binding details', async () => {
    const test = transport();
    test.answer(usage);
    expect(await test.client.agents.channelUsage('agt_one')).toEqual(usage);
    expect(new URL(test.calls.at(-1)!.url).pathname).toBe('/api/v1/agents/agt_one/channel-usage');
    test.answer({ ...usage, bindings: [binding] });
    expect((await test.client.agents.channelUsage('agt_one')).bindings).toEqual([binding]);
    for (const channel_usage of [
      { active: false, has_history: false, first_used_at: null, last_used_at: null },
      { active: false, has_history: true, first_used_at: now, last_used_at: now },
    ]) {
      test.answer({ ...usage, channel_usage, bindings: [] });
      expect((await test.client.agents.channelUsage('agt_one')).channel_usage).toEqual(
        channel_usage,
      );
    }
  });
  it('rejects wrong identities, inconsistent summaries and invalid or duplicate binding records', async () => {
    const test = transport();
    for (const malformed of [
      { ...usage, agent_id: 'another-agent' },
      { ...usage, channel_usage: { ...summary, active: 'true' } },
      { ...usage, channel_usage: { ...summary, first_used_at: now } },
      { ...usage, channel_usage: { ...summary, has_history: false } },
      { ...usage, bindings: [] },
      {
        ...usage,
        bindings: Array.from({ length: 17 }, (_, i) => ({ ...binding, binding_id: String(i) })),
      },
      { ...usage, bindings: [binding, binding] },
      { ...usage, bindings: [{ ...binding, generation: 0 }] },
    ]) {
      test.answer(malformed);
      await expect(test.client.agents.channelUsage('agt_one')).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
  });
  it('pages every transition kind with exact Agent identity and revision continuation', async () => {
    const test = transport();
    const items = ['attach', 'use', 'replace', 'detach'].map((kind, i) => event(i + 1, kind));
    test.answer({ agent_id: 'agt_one', items, next: '4' });
    const first = await test.client.agents.channelUsageHistory('agt_one', { limit: 4 });
    expect(first.items.map((item) => item.change.kind)).toEqual([
      'attach',
      'use',
      'replace',
      'detach',
    ]);
    expect(new URL(test.calls.at(-1)!.url).pathname).toBe(
      '/api/v1/agents/agt_one/channel-usage/history',
    );
    test.answer({ agent_id: 'agt_one', items: [] });
    expect(
      await test.client.agents.channelUsageHistory('agt_one', { limit: 4, cursor: first.next }),
    ).toEqual({ agent_id: 'agt_one', items: [] });
    expect(Object.fromEntries(new URL(test.calls.at(-1)!.url).searchParams)).toEqual({
      limit: '4',
      cursor: '4',
    });
  });
  it('refuses malformed pages and invalid cursors before making a request', async () => {
    const test = transport();
    for (const cursor of ['', '0', '-1', '01', '1.1', '9007199254740992'])
      expect(() => test.client.agents.channelUsageHistory('agt_one', { cursor })).toThrow(
        ValidationError,
      );
    for (const limit of [0, 201, 1.1])
      expect(() => test.client.agents.channelUsageHistory('agt_one', { limit })).toThrow(
        ValidationError,
      );
    expect(test.calls).toHaveLength(0);
    for (const malformed of [
      { agent_id: 'another-agent', items: [] },
      { agent_id: 'agt_one', items: [event(), event(2)] },
      { agent_id: 'agt_one', items: [event(2)] },
      { agent_id: 'agt_one', items: [], next: '1' },
      { agent_id: 'agt_one', items: [event()], next: '2' },
      { agent_id: 'agt_one', items: [event()], next: 'invalid' },
      { agent_id: 'agt_one', items: [{ ...event(), detached_at: now }] },
      { agent_id: 'agt_one', items: [{ ...event(), reason: 'future-reason' }] },
      { agent_id: 'agt_one', items: [{ ...event(), change: { kind: 'future-change' } }] },
      { agent_id: 'agt_one', items: [{ ...event(), actor: { kind: 'unknown' } }] },
    ]) {
      test.answer(malformed);
      await expect(
        test.client.agents.channelUsageHistory('agt_one', { limit: 1 }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
  });
});
