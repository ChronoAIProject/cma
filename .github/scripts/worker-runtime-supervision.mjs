import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { buildExecutable, command } from './worker-runtime-build.mjs';
import { cases, cohort, hash, patchBridge, repository, qualification, testInventory, verifyArtifacts } from './worker-runtime-cohort.mjs';

assert.equal(process.argv.length, 2, 'native runner accepts no arguments');
process.umask(0o077);
const mongo = new URL(process.env.CHRONO_TEST_MONGODB_URI);
assert.equal(mongo.protocol, 'mongodb:');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(mongo.hostname), 'disposable loopback Mongo required');
const git = (...args) => execFileSync('git', args, { cwd: repository, encoding: 'utf8' }).trim();
const source = () => {
  assert.equal(git('status', '--porcelain', '--untracked-files=all'), '', 'commit the candidate before qualification');
  return { head: git('rev-parse', 'HEAD'), tree: git('rev-parse', 'HEAD^{tree}'), clean: true };
};
const original = source();
const pins = cohort();
const proof = realpathSync(mkdtempSync(join(tmpdir(), 'cma-worker-runtime-')));
const scratch = join(proof, 'scratch');
const evidence = join(scratch, 'fixtures');
for (const path of [scratch, evidence]) mkdirSync(path);
console.log('Native continuity evidence: ' + proof);
const results = [];
let passed = false;
let identity;
try {
  const tools = join(scratch, 'tools');
  const env = { ...process.env, CARGO_INCREMENTAL: '0', RUST_MIN_STACK: '33554432',
    npm_config_cache: join(scratch, 'npm-cache'), npm_config_userconfig: '/dev/null' };
  const installed = await command('npm', ['install', '--prefix', tools, '--ignore-scripts', '--no-audit', '--no-fund',
    '@openai/codex@' + pins.codex, '@agentclientprotocol/codex-acp@' + pins.acp], { env }, join(proof, 'install.log'));
  assert.equal(installed.code, 0, 'native tool installation failed');
  const bridge = join(tools, 'node_modules/@agentclientprotocol/codex-acp/dist/index.js');
  patchBridge(bridge);
  const natives = readdirSync(join(tools, 'node_modules/@openai'), { recursive: true, withFileTypes: true })
    .filter(entry => entry.isFile() && entry.name === 'codex')
    .map(entry => join(entry.parentPath, entry.name));
  assert.equal(natives.length, 1, 'expected one installed native Codex executable');
  const codex = realpathSync(natives[0]);
  assert.equal(execFileSync(codex, ['--version'], { encoding: 'utf8' }).trim(), 'codex-cli ' + pins.codex);
  const runtime = await buildExecutable(repository, proof, 'layer4-runtime', false, env);
  const test = await buildExecutable(repository, proof, 'cma', true, env);
  assert.deepEqual(source(), original, 'source changed during the build');
  identity = { source: original, runtime, test, codex: { path: codex, sha256: hash(codex) },
    bridge: { path: bridge, sha256: hash(bridge) }, pins, npm_lock_sha256: hash(join(tools, 'package-lock.json')),
    node: process.version, platform: process.platform, arch: process.arch,
    rust: execFileSync('rustc', ['+1.96.1', '-vV'], { encoding: 'utf8' }).trim(),
    build_environment: Object.fromEntries(['CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'CARGO_PROFILE_DEV_DEBUG',
      'CARGO_PROFILE_TEST_DEBUG', 'RUSTFLAGS', 'CARGO_INCREMENTAL'].filter(key => env[key] !== undefined).map(key => [key, env[key]])),
    boundary: 'Ordinary host-native Codex/ACP/runtime; synthetic loopback model/provider; disposable Mongo. No desktop or OCI qualification.' };
  const identityPath = join(proof, 'identity.json');
  writeFileSync(identityPath, JSON.stringify(identity, null, 2));
  const testEnv = { PATH: process.env.PATH, TMPDIR: scratch, RUST_MIN_STACK: '33554432',
    CHRONO_TEST_MONGODB_URI: process.env.CHRONO_TEST_MONGODB_URI, CMA_CODEX_TEST_BINARY: codex,
    CMA_ACP_TEST_BRIDGE: bridge, CMA_TEST_RUNTIME_BINARY: runtime.path,
    CMA_NATIVE_IDENTITY: identityPath, CMA_SUPERVISION_EVIDENCE_DIR: evidence };
  testInventory(execFileSync(test.path, ['--list', '--ignored', '--format', 'terse'], { env: testEnv, encoding: 'utf8' }));
  for (const name of cases) {
    verifyArtifacts(identity);
    const status = await command(test.path, [name, '--exact', '--ignored', '--nocapture', '--test-threads=1'],
      { cwd: repository, env: testEnv, timeout: 300000 }, join(proof, name.split('::').at(-1) + '.log'));
    results.push({ name, ...status });
    console.log(name + ': ' + (status.code === 0 ? 'PASS' : 'FAIL'));
  }
  verifyArtifacts(identity);
  assert.deepEqual(source(), original, 'source changed during execution');
  passed = results.length === cases.length && results.every(result => result.code === 0 && result.signal === null);
} catch (error) {
  console.error('Native qualification refused: ' + error.message);
  process.exitCode = 1;
} finally {
  const roots = readdirSync(evidence).map(name => join(evidence, name));
  const receipts = [];
  for (const root of roots) {
    const deadline = Date.now() + 15000;
    while (!readdirSync(root).includes('cleanup.json') && Date.now() < deadline) await delay(50);
    let cleanup = null, result = null;
    try { cleanup = JSON.parse(readFileSync(join(root, 'cleanup.json'), 'utf8')); } catch {}
    try { result = JSON.parse(readFileSync(join(root, 'result.json'), 'utf8')); } catch {}
    receipts.push({ cleanup, result });
  }
  const completed = qualification(results, receipts);
  const cleanupComplete = completed.cleanup_complete;
  passed = passed && completed.passed;
  // Preserve only the small terminal receipts, never the generated home/config/tool caches.
  if (receipts.every(({ cleanup }) => cleanup?.runtime_exited && cleanup?.task_exited)) rmSync(scratch, { recursive: true });
  const scratchRemoved = !readdirSync(proof).includes('scratch');
  passed = passed && scratchRemoved;
  writeFileSync(join(proof, 'qualification.json'), JSON.stringify({ passed, results, receipts,
    cleanup_complete: cleanupComplete, scratch_removed: scratchRemoved }, null, 2));
  if (!passed) process.exitCode = 1;
  console.log('Native qualification ' + (passed ? 'PASS' : 'FAIL') + ': ' + proof);
}
