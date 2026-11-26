import assert from 'node:assert/strict';
import {mkdtemp, readdir, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import test from 'node:test';
import {drainOwnerCase} from './readiness.mjs';

async function fixture(t, {readyAfterDrain = 503, peerCanWrite = true} = {}) {
  const output = await mkdtemp(join(tmpdir(), 'cma-fixture-'));
  t.after(() => rm(output, {recursive: true, force: true}));
  const agentId = 'test-agent';
  const original = {am: 'a', agent_id: agentId, epoch: 1, instance_id: 'first', pod_uid: 'first-pod'};
  const successor = {am: 'b', agent_id: agentId, epoch: 2, instance_id: 'peer', pod_uid: 'peer-pod'};
  const calls = [];
  let owner = original;
  let preferences = {title: 'Original', revision: 1};
  const adapter = {
    ownerOf: async () => owner,
    drainOwner: async ({owner: requested}) => {
      assert.deepEqual(requested, original);
      calls.push('drain');
      return {am: 'a', readiness: readyAfterDrain, elapsed_ms: 10};
    },
    restartDrained: async input => {
      assert.deepEqual(input, {am: 'a'});
      calls.push('restart');
      await new Promise(resolve => setTimeout(resolve, 20));
      return {origin: 'http://127.0.0.1:9999', ready: true};
    },
    topology: async () => ({ams: {
      a: {ready: true, pod_uid: 'restarted-pod'},
      b: {ready: true, pod_uid: successor.pod_uid},
    }}),
  };
  const api = {rawRequest: async (am, method, path, options) => {
    assert.equal(am, 'b');
    assert.match(path, /\/preferences$/);
    calls.push(method);
    if (method === 'PATCH') {
      if (!peerCanWrite) throw Error('fixture_peer_write_failed');
      assert.equal(options.body.expected_revision, preferences.revision);
      preferences = {title: options.body.title, revision: preferences.revision + 1};
      owner = successor;
    }
    return {body: preferences};
  }};
  const config = {output_dir: output, ams: {}};
  return {adapter, api, record: {agentId, name: preferences.title}, config, calls};
}

test('the peer writes under a new epoch and serves while the drained manager restarts', async t => {
  const context = await fixture(t);
  const result = await drainOwnerCase(context);
  assert.equal(result.peer_advanced, true);
  assert(result.useful_reads_during_restart > 0);
  assert.equal(context.config.ams.a, 'http://127.0.0.1:9999');
  assert(context.calls.lastIndexOf('PATCH') > context.calls.indexOf('restart'));
});

test('a failed readiness transition still restarts the drained manager', async t => {
  const context = await fixture(t, {readyAfterDrain: 200});
  await assert.rejects(drainOwnerCase(context), /manager_drain_bound/);
  assert(context.calls.includes('restart'));
  assert(!context.calls.includes('PATCH'));
});

test('simultaneous peer and restart failure retains each cause', async t => {
  const context = await fixture(t, {peerCanWrite: false});
  context.adapter.restartDrained = async () => { throw Error('fixture_restart_failed'); };
  await assert.rejects(drainOwnerCase(context), /fixture_peer_write_failed/);
  const output = context.config.output_dir;
  const failures = await Promise.all((await readdir(output)).map(async name =>
    JSON.parse(await readFile(join(output, name), 'utf8'))));
  assert.deepEqual(Object.fromEntries(failures.map(error => [error.stage, error.error_code])),
    {primary: 'fixture_peer_write_failed', restoration: 'fixture_restart_failed'});
});
