import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { artifact, cases, cohort, hash, patches, patchBridge, repository, qualification, testInventory, verifyArtifacts } from './worker-runtime-cohort.mjs';
import { buildExecutable } from './worker-runtime-build.mjs';

function setup(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'native-worker-mechanics-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const path of ['api/crates/layer4-runtime', 'api/crates/layer3-adapter/src/codex', 'api/target/debug', 'lane', 'bin']) {
    mkdirSync(join(root, path), { recursive: true });
  }
  const original = readFileSync(join(repository, 'api/crates/layer4-runtime/Dockerfile'), 'utf8');
  writeFileSync(join(root, 'api/crates/layer4-runtime/Dockerfile'), original);
  const executable = join(root, 'lane/layer4-runtime');
  writeFileSync(executable, 'synthetic current artifact'); chmodSync(executable, 0o700);
  writeFileSync(join(root, 'api/target/debug/layer4-runtime'), 'stale artifact');
  const row = { reason: 'compiler-artifact', manifest_path: join(root, 'api/crates/layer4-runtime/Cargo.toml'),
    target: { name: 'layer4-runtime', kind: ['bin'], src_path: join(root, 'api/crates/layer4-runtime/src/main.rs') },
    profile: { test: false }, executable };
  return { root, row, executable, original };
}
const output = rows => [...rows, { reason: 'build-finished', success: true }].map(JSON.stringify).join('\n');

test('native cohort shares the current ordinary runtime recipe and eleven transforms', () => {
  assert.equal(patches.length, 11);
  const pins = cohort();
  assert.equal(pins.codex, '0.147.0');
  assert.equal(pins.acp, '1.4.0');
  assert.equal(pins.bridge, '8f0c023269496f9ad21aa2958491772fefe65011b9aebaf674c67d2aaa9fbcf4');
});
for (const label of ['omitted patch', 'reordered patches', 'missing pin', 'ambiguous pin']) {
  test('cohort rejects ' + label, t => {
    const s = setup(t);
    let recipe = s.original;
    if (label === 'omitted patch') recipe = recipe.replace('node /tmp/cma-codex-source/session_open_overlap.cjs', 'true');
    if (label === 'reordered patches') recipe = recipe.replaceAll('session_open_overlap.cjs', 'TEMP.cjs').replaceAll('deferred_session_metadata.cjs', 'session_open_overlap.cjs').replaceAll('TEMP.cjs', 'deferred_session_metadata.cjs');
    if (label === 'missing pin') recipe = recipe.replace(/^ARG CODEX_ACP_SHA256=.*$/m, '');
    if (label === 'ambiguous pin') recipe += '\nARG CODEX_VERSION=0.1.0\n';
    writeFileSync(join(s.root, 'api/crates/layer4-runtime/Dockerfile'), recipe);
    assert.throws(() => cohort(s.root));
  });
}
test('bridge transforms run in recipe order and reject changed input or final pin', t => {
  const s = setup(t), input = 'synthetic ACP';
  const digest = value => createHash('sha256').update(value).digest('hex');
  const patched = input + patches.map(([name]) => ':' + name).join('');
  const recipe = s.original.replace(/^ARG CODEX_ACP_UPSTREAM_SHA256=.*$/m, 'ARG CODEX_ACP_UPSTREAM_SHA256=' + digest(input))
    .replace(/^ARG CODEX_ACP_SHA256=.*$/m, 'ARG CODEX_ACP_SHA256=' + digest(patched));
  const recipePath = join(s.root, 'api/crates/layer4-runtime/Dockerfile');
  writeFileSync(recipePath, recipe);
  for (const [file, method] of patches) writeFileSync(join(s.root, 'api/crates/layer3-adapter/src/codex', file + '.cjs'),
    `exports.${method} = source => source + ${JSON.stringify(':' + file)};`);
  const bridge = join(s.root, 'bridge.js');
  writeFileSync(bridge, input); patchBridge(bridge, s.root);
  assert.equal(readFileSync(bridge, 'utf8'), patched);
  assert.throws(() => patchBridge(bridge, s.root), /unreviewed ACP input/);
  writeFileSync(bridge, input);
  writeFileSync(recipePath, recipe.replace(digest(patched), '0'.repeat(64)));
  assert.throws(() => patchBridge(bridge, s.root), /unreviewed patched ACP/);
  assert.equal(readFileSync(bridge, 'utf8'), input);
});
for (const custom of [false, true]) test(`actual build command binds ${custom ? 'custom' : 'default'} Cargo output, never a stale fixed path`, async t => {
  const s = setup(t);
  if (!custom) {
    s.row.executable = join(s.root, 'api/target/debug/layer4-runtime');
    writeFileSync(s.row.executable, 'new default artifact'); chmodSync(s.row.executable, 0o700);
  }
  const cargo = join(s.root, 'bin/cargo');
  writeFileSync(cargo, `#!${process.execPath}\nconst fs=require('node:fs');
fs.writeFileSync(${JSON.stringify(join(s.root, 'argv.json'))}, JSON.stringify({args:process.argv.slice(2),incremental:process.env.CARGO_INCREMENTAL,target:process.env.CARGO_TARGET_DIR}));
process.stdout.write(${JSON.stringify(output([s.row]))});`);
  chmodSync(cargo, 0o700);
  const result = await buildExecutable(s.root, s.root, 'layer4-runtime', false,
    { PATH: join(s.root, 'bin'), ...(custom ? { CARGO_TARGET_DIR: join(s.root, 'lane') } : {}) });
  assert.equal(result.path, s.row.executable);
  assert.equal(result.sha256, hash(s.row.executable));
  const invocation = JSON.parse(readFileSync(join(s.root, 'argv.json')));
  assert.equal(invocation.incremental, '0');
  assert.ok(invocation.args.includes('--message-format=json'));
  assert.equal(invocation.target, custom ? join(s.root, 'lane') : undefined);
});
for (const [label, mutate] of [
  ['no artifact', s => []], ['multiple artifacts', s => [s.row, s.row]],
  ['wrong name', s => [{ ...s.row, target: { ...s.row.target, name: 'other' } }]],
  ['wrong kind', s => [{ ...s.row, target: { ...s.row.target, kind: ['lib'] } }]],
  ['test binary for runtime', s => [{ ...s.row, profile: { test: true } }]],
  ['wrong manifest', s => [{ ...s.row, manifest_path: '/elsewhere/Cargo.toml' }]],
  ['wrong source', s => [{ ...s.row, target: { ...s.row.target, src_path: '/elsewhere/main.rs' } }]],
  ['missing executable', s => [{ ...s.row, executable: join(s.root, 'missing') }]],
  ['relative executable', s => [{ ...s.row, executable: 'lane/layer4-runtime' }]],
  ['null executable', s => [{ ...s.row, executable: null }]],
]) test('artifact selection refuses ' + label, t => {
  const s = setup(t);
  assert.throws(() => artifact(output(mutate(s)), s.root, 'layer4-runtime', false));
});
test('failed build refuses even when it emitted a valid artifact', async t => {
  const s = setup(t), cargo = join(s.root, 'bin/cargo');
  writeFileSync(cargo, `#!${process.execPath}\nprocess.stdout.write(${JSON.stringify(output([s.row]))}); process.exit(17);`);
  chmodSync(cargo, 0o700);
  await assert.rejects(buildExecutable(s.root, s.root, 'layer4-runtime', false, { PATH: join(s.root, 'bin') }), /build failed/);
});
test('inventory refuses missing, additional and duplicate cases', () => {
  const listing = cases.map(name => name + ': test').join('\n');
  testInventory(listing);
  for (const invalid of ['', cases[0] + ': test', listing + '\n' + cases[0] + ': test', listing.replace(cases[0], cases[0] + '_other')]) {
    assert.throws(() => testInventory(invalid));
  }
});
test('artifact identity refuses replacement after build', t => {
  const s = setup(t), built = { path: s.executable, sha256: hash(s.executable) };
  const identity = Object.fromEntries(['runtime', 'test', 'codex', 'bridge'].map(key => [key, built]));
  verifyArtifacts(identity);
  writeFileSync(s.executable, 'replaced');
  assert.throws(() => verifyArtifacts(identity));
});

function receipts() {
  return [false, true].map(stall => ({ cleanup: { runtime_exited: true, task_exited: true, server_closed: true },
    result: { stall, passed: true, cleanup: { database_dropped: true, recorder_stopped: true, scheduler_stopped: true, fixture_exited: true } } }));
}
test('qualification requires exact process exits and terminal cleanup, including controlled failure', () => {
  const results = cases.map(name => ({ name, code: 0, signal: null }));
  assert.equal(qualification(results, receipts()).passed, true);
  for (const field of ['database_dropped', 'recorder_stopped', 'scheduler_stopped', 'fixture_exited']) {
    const rows = receipts(); delete rows[0].result.cleanup[field];
    assert.equal(qualification(results, rows).passed, false, field);
  }
  for (const field of ['runtime_exited', 'task_exited', 'server_closed']) {
    const rows = receipts(); rows[0].cleanup[field] = false;
    assert.equal(qualification(results, rows).passed, false, field);
  }
  const failed = results.map((result, index) => ({ ...result, code: index === 0 ? 101 : 0 }));
  assert.deepEqual(qualification(failed, receipts()), { passed: false, cleanup_complete: true });
  assert.equal(qualification(results, [receipts()[0], receipts()[0]]).passed, false);
  assert.equal(qualification(results, [{ result: { passed: true } }]).passed, false);
});
