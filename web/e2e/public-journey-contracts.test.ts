// @vitest-environment node
import { describe, expect, it } from 'vitest';
import {
  assertContextualTask,
  assertInlineTarget,
  assertPublishedLaunch,
  assertPublisherIdentity,
  assertSingleLaunchDelta,
} from './public-journey-contracts';
import { hashValue, workCommands } from './journey-proof';

const target = {
  image: {
    id: 'public-image',
    version: 'version-one',
    reference: `registry.example.test/runtime@sha256:${'a'.repeat(64)}`,
  },
  agent: {
    model: 'fixture-model',
    reasoning_effort: 'high' as const,
    permission_mode: 'guarded' as const,
    skills: [],
    system_prompt: 'Private configuration',
    services: ['model'],
  },
  workspace: {
    repository: { url: 'https://github.com/example/repository', reference: 'b'.repeat(40) },
  },
};

describe('current public journey assertions', () => {
  it('accepts selected inline configuration with empty legacy sentinels only', () => {
    assertInlineTarget({ inline: target, sandbox_profile: { id: '', revision: 0 } }, target);
    expect(() =>
      assertInlineTarget({ sandbox_profile: { id: 'old-profile', revision: 1 } }, target),
    ).toThrow();
    expect(() =>
      assertInlineTarget(
        { inline: target, sandbox_profile: { id: 'old-profile', revision: 1 } },
        target,
      ),
    ).toThrow();
    expect(() =>
      assertInlineTarget(
        { inline: { ...target, image: { ...target.image, version: 'wrong' } } },
        target,
      ),
    ).toThrow();
  });
  it('requires the authenticated nonvisitor publisher, independent of resource labels', () => {
    assertPublisherIdentity({ authenticated: true, visitor: false, subject: 'owner' }, 'owner');
    for (const value of [
      { authenticated: true, visitor: true, subject: 'owner' },
      { authenticated: true, visitor: false, subject: 'foreign' },
      { subject: 'owner' },
    ])
      expect(() => assertPublisherIdentity(value, 'owner')).toThrow();
  });
  it('binds immutable launch attribution and created Sandbox ownership', () => {
    const expected = {
      triggerId: 'trigger',
      publicId: 'public',
      placementId: 'placement',
      digest: 'digest',
      subject: 'visitor',
    };
    const launch = {
      trigger_id: 'trigger',
      public_id: 'public',
      placement_id: 'placement',
      revision_digest: 'digest',
      resolution: { sandbox: { state: 'created', id: 'sandbox' } },
    };
    const sandbox = { id: 'sandbox', owner: 'visitor' };
    assertPublishedLaunch(launch, sandbox, expected);
    for (const change of [
      { trigger_id: 'foreign' },
      { public_id: 'foreign' },
      { placement_id: 'foreign' },
      { revision_digest: 'foreign' },
      { resolution: { sandbox: { state: 'reused', id: 'sandbox' } } },
    ])
      expect(() => assertPublishedLaunch({ ...launch, ...change }, sandbox, expected)).toThrow();
    expect(() =>
      assertPublishedLaunch(launch, { ...sandbox, owner: 'publisher' }, expected),
    ).toThrow();
  });
  it('requires one total and one placement increment, rejecting preexisting nonzero counts', () => {
    const stats = (count: number) => ({
      totals: { launches_total: count, agents_ready_total: count },
      placements: [{ placement_id: 'placement', launches_total: count, agents_ready_total: count }],
    });
    assertSingleLaunchDelta(stats(7), stats(8), 'placement');
    for (const count of [0, 7, 9])
      expect(() => assertSingleLaunchDelta(stats(7), stats(count), 'placement')).toThrow();
    expect(() =>
      assertSingleLaunchDelta(stats(7), { ...stats(8), placements: [] }, 'placement'),
    ).toThrow();
  });
  it('refuses historical profile fields and changed task bodies in contextual creation', () => {
    const body = { name: 'New task', workspace_id: 'workspace', initial_message: 'hello' };
    assertContextualTask(body, 'workspace', 'hello');
    for (const change of [
      { workspace_id: 'foreign' },
      { initial_message: 'other' },
      { agent_profile: { id: 'old', revision: 1 } },
    ])
      expect(() => assertContextualTask({ ...body, ...change }, 'workspace', 'hello')).toThrow();
  });
  it('compares target hashes independent of object order and counts extra work commands', () => {
    expect(hashValue({ a: 1, b: 2 })).toBe(hashValue({ b: 2, a: 1 }));
    const commands = [
      '/api/v1/agents',
      '/api/v1/triggers/public/public/launches',
      '/api/v1/agents/a/input',
      '/api/v1/agui/run',
      '/api/v1/responses',
      '/api/v1/triggers',
    ].map((path) => ({ method: 'POST', path, bodyHash: 'hash' }));
    expect(workCommands(commands)).toHaveLength(5);
  });
});
