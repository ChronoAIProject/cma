import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { accessSync, constants, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const patches = [
  ['persist_conversations', 'persistConversations'], ['subscription_auth', 'subscriptionAuth'],
  ['managed_model_catalog', 'managedModelCatalog'], ['runtime_observations', 'runtimeObservations'],
  ['required_access_tool', 'requiredAccessTool'], ['session_commands', 'sessionCommands'],
  ['agent_instructions', 'agentInstructions'], ['turn_steering', 'turnSteering'],
  ['open_completion', 'openCompletion'], ['deferred_session_metadata', 'deferredSessionMetadata'],
  ['session_open_overlap', 'sessionOpenOverlap'],
];
export const cases = ['scheduler_panic_preserves_native_task_home_and_conversation',
  'scheduler_stall_preserves_native_task_home_and_conversation']
  .map(name => 'store::worker_runtime_supervision_tests::' + name);
export const hash = file => createHash('sha256').update(readFileSync(file)).digest('hex');

export function cohort(root = repository) {
  const recipe = readFileSync(join(root, 'api/crates/layer4-runtime/Dockerfile'), 'utf8');
  const pin = name => {
    const values = [...recipe.matchAll(new RegExp('^ARG ' + name + '=(\\S+)$', 'gm'))];
    assert.equal(values.length, 1, 'missing/ambiguous runtime pin: ' + name);
    return values[0][1];
  };
  assert.deepEqual([...recipe.matchAll(/node \/tmp\/cma-codex-source\/(\w+)\.cjs/g)].map(m => m[1]),
    patches.map(([file]) => file), 'native patch order differs from the runtime recipe');
  const result = { codex: pin('CODEX_VERSION'), acp: pin('CODEX_ACP_VERSION'),
    upstream: pin('CODEX_ACP_UPSTREAM_SHA256'), bridge: pin('CODEX_ACP_SHA256') };
  for (const key of ['upstream', 'bridge']) assert.match(result[key], /^[a-f0-9]{64}$/);
  for (const key of ['codex', 'acp']) assert.match(result[key], /^\d+\.\d+\.\d+$/);
  return result;
}

export function patchBridge(file, root = repository) {
  const expected = cohort(root);
  assert.equal(hash(file), expected.upstream, 'unreviewed ACP input');
  let source = readFileSync(file, 'utf8');
  const require = createRequire(import.meta.url);
  for (const [name, transform] of patches) {
    source = require(join(root, 'api/crates/layer3-adapter/src/codex', name + '.cjs'))[transform](source);
  }
  assert.equal(createHash('sha256').update(source).digest('hex'), expected.bridge, 'unreviewed patched ACP');
  writeFileSync(file, source);
}

export function artifact(json, root, target, test) {
  const rows = json.trim().split('\n').map(line => JSON.parse(line));
  assert.equal(rows.filter(row => row.reason === 'build-finished' && row.success === true).length, 1,
    'missing successful build completion');
  assert.ok(!rows.some(row => row.reason === 'build-finished' && row.success !== true));
  const matches = rows.filter(row => row.reason === 'compiler-artifact' && row.target?.name === target
    && row.target.kind?.includes('bin') && row.profile?.test === test && row.executable !== null);
  assert.equal(matches.length, 1, 'expected exactly one ' + target + ' executable');
  const row = matches[0];
  assert.equal(row.manifest_path, join(root, 'api/crates', target, 'Cargo.toml'), 'wrong artifact package');
  assert.equal(row.target.src_path, join(root, 'api/crates', target, 'src/main.rs'), 'wrong artifact source');
  assert.equal(typeof row.executable, 'string');
  assert.ok(row.executable.startsWith('/'), 'artifact executable must be absolute');
  const path = realpathSync(row.executable);
  assert.ok(statSync(path).isFile());
  accessSync(path, constants.X_OK);
  return { path, sha256: hash(path) };
}

export function verifyArtifacts(identity) {
  for (const key of ['runtime', 'test', 'codex', 'bridge']) {
    assert.equal(hash(identity[key].path), identity[key].sha256, key + ' artifact changed');
  }
}

export function testInventory(text) {
  const names = text.split('\n').filter(line => line.endsWith(': test')).map(line => line.slice(0, -6))
    .filter(name => name.startsWith('store::worker_runtime_supervision_tests::'));
  assert.deepEqual(names.sort(), [...cases].sort(), 'native test inventory changed');
}

export function qualification(results, receipts) {
  const exited = results.length === cases.length && cases.every(name =>
    results.filter(result => result.name === name && result.code === 0 && result.signal === null).length === 1);
  const cleaned = receipts.length === cases.length && [false, true].every(stall =>
    receipts.filter(({ result }) => result?.stall === stall).length === 1)
    && receipts.every(({ cleanup, result }) => cleanup?.runtime_exited === true && cleanup?.task_exited === true
      && cleanup?.server_closed === true && ['database_dropped', 'recorder_stopped', 'scheduler_stopped', 'fixture_exited']
        .every(key => result?.cleanup?.[key] === true));
  return { cleanup_complete: cleaned, passed: exited && cleaned && receipts.every(({ result }) => result?.passed === true) };
}
