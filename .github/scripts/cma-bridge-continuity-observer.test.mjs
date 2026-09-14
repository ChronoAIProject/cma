import assert from 'node:assert/strict';
import test from 'node:test';
import { capture, compareSnapshots, digest, eventPrefix, validateConfig } from './cma-bridge-continuity-observer.mjs';

const config = {
  schema: 'cma-bridge-continuity-config-v1', origin: 'https://fixture.example',
  context: 'fixture', namespace: 'disposable', deploymentId: 'continuity',
  tokenFile: '/private/saved-token', agentIds: ['agt_fixture'],
};
const time = '2026-09-14T00:00:00Z';
const event = (seq, text = 'retained text') => ({ seq, appended_at: time, kind: { type: 'message', text } });

async function observation({ events = [event(1)], phase = 'Running', agentStatus = 'ready', inventory = true } = {}) {
  const paths = [];
  const snapshot = await capture(config, {
    get: async (path, authenticated) => {
      paths.push(path);
      if (path === '/') { assert.equal(authenticated, false); return null; }
      assert.equal(authenticated, true);
      if (path.includes('/events?')) return { items: events };
      assert.equal(path, '/api/v1/agents/agt_fixture');
      return {
        id: 'agt_fixture', sandbox: { id: 'sbx_fixture' }, workspace: { id: 'wks_fixture' },
        worktree: { id: 'wkt_fixture' }, profile: { id: 'agp_fixture', revision: 1 },
        created_at: time, status: agentStatus,
      };
    },
    pods: async () => ({ items: inventory ? [{
      metadata: { name: 'retained-pod', uid: 'retained-uid', labels: { 'cma/sandbox': 'sbx_fixture', 'cma/runtime-generation': '1', 'cma/runtime-contract': '3' } },
      spec: { containers: [{ name: 'runtime' }] },
      status: { phase, containerStatuses: [{ name: 'runtime', ready: true, imageID: `registry/runtime@sha256:${'a'.repeat(64)}`, containerID: 'containerd://retained', restartCount: 0 }] },
    }] : [] }),
  }, () => new Date(time));
  assert.deepEqual(paths, ['/', '/api/v1/agents/agt_fixture', '/api/v1/agents/agt_fixture/events?after=0&limit=100']);
  return snapshot;
}

test('growing history and normal activity retain the observed prefix', async () => {
  const before = await observation();
  const after = await observation({ events: [event(1), event(2, 'new user activity')], agentStatus: 'working' });
  const result = compareSnapshots(before, after);
  assert.equal(result.status, 'passed');
  assert.ok(result.unproved.includes('continuous_availability_between_samples'));
  assert.ok(!JSON.stringify(before).includes('retained text'));
});

test('canonical hashes ignore object key order but retain content changes', () => {
  assert.equal(digest({ a: 1, b: { c: 2, d: 3 } }), digest({ b: { d: 3, c: 2 }, a: 1 }));
  assert.notEqual(digest(event(1)), digest(event(1, 'changed')));
});

test('history mutation and truncation fail despite stable Agent IDs', async () => {
  const before = await observation({ events: [event(1), event(2)] });
  for (const events of [[event(1)], [event(1, 'replaced'), event(2)]]) {
    const result = compareSnapshots(before, await observation({ events }));
    assert.ok(result.failures.some((failure) => failure.code === 'history_prefix_changed'));
  }
});

test('a replaced Pod, restarted container, or image change fails continuity', async () => {
  const before = await observation();
  for (const mutate of [
    (runtime) => { runtime.uid = 'new-uid'; },
    (runtime) => { runtime.containers[0].restarts += 1; },
    (runtime) => { runtime.containers[0].id = 'containerd://replacement'; },
    (runtime) => { runtime.containers[0].image = `registry/runtime@sha256:${'b'.repeat(64)}`; },
  ]) {
    const after = structuredClone(before);
    mutate(after.runtimes[0]);
    assert.equal(compareSnapshots(before, after).status, 'failed');
  }
});

test('lineage, profile pin and creation identity cannot silently change', async () => {
  const before = await observation();
  for (const [field, value] of [['sandboxId', 'sbx_other'], ['workspaceId', 'wks_other'], ['worktreeId', 'wkt_other'], ['profileHash', 'b'.repeat(64)], ['createdAt', '2026-09-15T00:00:00Z']]) {
    const after = structuredClone(before);
    after.agents[0][field] = value;
    if (field === 'sandboxId') after.runtimes[0].sandboxId = value;
    assert.ok(compareSnapshots(before, after).failures.some((failure) => failure.code === 'agent_identity_changed'));
  }
});

test('forced pause and failure remain failures with retained history', async () => {
  const before = await observation();
  for (const agentStatus of ['paused', 'stopped', 'failed', 'deleted']) {
    const result = compareSnapshots(before, await observation({ agentStatus }));
    assert.ok(result.failures.some((failure) => failure.code === 'live_agent_became_unavailable'));
  }
});

test('missing or unavailable live runtime never produces a successful observation', async () => {
  await assert.rejects(observation({ inventory: false }), /live_agent_runtime_missing_or_ambiguous/);
  await assert.rejects(observation({ phase: 'Pending' }), /runtime_not_running/);
});

test('gaps and duplicated event sequences are refused', () => {
  for (const events of [[event(2)], [event(1), event(1)], [event(1), event(3)]]) {
    assert.throws(() => eventPrefix(events), /event_sequence_gap_or_duplicate/);
  }
});

test('incomplete receipts and scope changes cannot be compared as success', async () => {
  const before = await observation();
  for (const mutate of [
    (after) => { after.status = 'failed'; },
    (after) => { after.agents = []; },
    (after) => { after.agents.push(after.agents[0]); },
    (after) => { after.scope.namespace = 'other'; },
    (after) => { delete after.agents[0].profileHash; },
    (after) => { delete after.runtimes[0].containers[0].id; },
    (after) => { after.runtimes = []; },
    (after) => { after.observedAt = '2026-09-13T00:00:00Z'; },
  ]) {
    const after = structuredClone(before);
    mutate(after);
    assert.throws(() => compareSnapshots(before, after));
  }
});

test('empty, duplicate or unbounded targets and unsafe origins are refused', () => {
  for (const patch of [
    { agentIds: [] }, { agentIds: ['agt_fixture', 'agt_fixture'] },
    { agentIds: ['../admin'] }, { origin: 'http://fixture.example' },
    { origin: 'https://fixture.example/path' }, { tokenFile: 'relative' },
  ]) assert.throws(() => validateConfig({ ...config, ...patch }));
});

test('HTTP failure is not downgraded to an empty or skipped observation', async () => {
  await assert.rejects(capture(config, {
    get: async () => { throw new Error('public_read_failed_503'); },
    pods: async () => { assert.fail('must not continue after failed public read'); },
  }), /public_read_failed_503/);
});

test('nonadvancing event cursors fail without retrying forever', async () => {
  await assert.rejects(capture(config, {
    get: async (path) => {
      if (path === '/') return null;
      if (path.includes('/events?')) return { items: [event(1)], next: 0 };
      return { id: 'agt_fixture' };
    },
    pods: async () => { assert.fail('invalid history must abort inventory'); },
  }), /event_cursor_did_not_advance/);
});
