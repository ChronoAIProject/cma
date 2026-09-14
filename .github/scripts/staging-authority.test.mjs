import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { chmodSync, existsSync, lstatSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { privateFile, requiredCases, runObserver, validateAttestations, validateCampaign, validatePolicy, validateReceipt } from './staging-authority.mjs';

const hash = (value) => createHash('sha256').update(value).digest('hex');
function fixtures() {
  const images = Object.fromEntries(['api', 'web', 'runtime'].map((name) => [name, `registry.example.test/cma-${name}@sha256:${'a'.repeat(64)}`]));
  const request = { schema: 'cma-staging-attestation-request-v1', nonce: '00000000-0000-4000-8000-000000000001', expected: { origin: 'https://staging.example.test', commit: 'b'.repeat(40), nyxidCommit: 'c'.repeat(40), images } };
  const receipt = { schema: 'cma-staging-attestation-v1', nonce: request.nonce, observedAt: new Date().toISOString(), origin: request.expected.origin, commit: request.expected.commit, nyxidCommit: request.expected.nyxidCommit, runtimePin: images.runtime, images: {} };
  for (const name of Object.keys(images)) receipt.images[name] = {
    reference: images[name], rootDigest: `sha256:${'a'.repeat(64)}`,
    platformDigest: `sha256:${'d'.repeat(64)}`, configDigest: `sha256:${'e'.repeat(64)}`,
    source: 'https://github.com/ChronoAIProject/cma', revision: request.expected.commit,
    ...(name === 'runtime' ? {} : { runningImageID: `docker-pullable://registry.example.test/cma-${name}@sha256:${'d'.repeat(64)}` }),
  };
  return { request, receipt };
}

test('live environment policy requires non-bypassable review and exactly develop', () => {
  const policy = { name: 'staging-e2e-mutation', can_admins_bypass: false, deployment_branch_policy: { protected_branches: false, custom_branch_policies: true }, protection_rules: [{ type: 'required_reviewers', prevent_self_review: true, reviewers: [{ type: 'User', reviewer: { id: 7 } }] }] };
  const branches = { total_count: 1, branch_policies: [{ name: 'develop', type: 'branch' }] };
  validatePolicy(policy, branches);
  for (const mutate of [
    (p) => { p.can_admins_bypass = true; },
    (p) => { p.protection_rules = []; },
    (p) => { p.protection_rules[0].prevent_self_review = false; },
    (p) => { p.protection_rules[0].reviewers = []; },
    (p) => { p.deployment_branch_policy.protected_branches = true; },
  ]) { const changed = structuredClone(policy); mutate(changed); assert.throws(() => validatePolicy(changed, branches)); }
  assert.throws(() => validatePolicy(policy, { total_count: 1, branch_policies: [{ name: '*', type: 'branch' }] }));
  assert.throws(() => validatePolicy(policy, { total_count: 2, branch_policies: branches.branch_policies }));
});

test('independent receipt binds challenge, freshness, desired root, running platform and config identities', () => {
  const { request, receipt } = fixtures();
  validateReceipt(receipt, request);
  for (const mutate of [
    (v) => { v.nonce = 'another-run'; },
    (v) => { v.observedAt = '2000-01-01T00:00:00Z'; },
    (v) => { v.origin = 'https://other.example.test'; },
    (v) => { v.nyxidCommit = 'f'.repeat(40); },
    (v) => { v.runtimePin = 'runtime:latest'; },
    (v) => { v.images.api.reference = 'api:latest'; },
    (v) => { v.images.api.rootDigest = v.images.api.configDigest; },
    (v) => { v.images.web.runningImageID = `registry.example.test/web@${v.images.web.configDigest}`; },
    (v) => { v.images.runtime.source = 'https://example.invalid'; },
    (v) => { v.images.runtime.revision = 'f'.repeat(40); },
    (v) => { v.images.runtime.configDigest = ''; },
    (v) => { v.images.api.extra = 'raw-log'; },
  ]) { const changed = structuredClone(receipt); mutate(changed); assert.throws(() => validateReceipt(changed, request)); }
});

test('observer bytes and config are private, exact, unchanged and invoked without inherited authority', () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-observer-authority-')));
  const verifier = join(directory, 'observer.mjs'), config = join(directory, 'target.json');
  const { request, receipt } = fixtures();
  writeFileSync(verifier, 'reviewed-verifier', { mode: 0o600 });
  writeFileSync(config, 'reviewed-config', { mode: 0o600 });
  const workdirs = new Set();
  const invoke = (change) => runObserver(verifier, hash('reviewed-verifier'), config, hash('reviewed-config'), request, (_program, args, options) => {
    assert.ok(!workdirs.has(options.cwd));
    workdirs.add(options.cwd);
    assert.deepEqual(args, [verifier]);
    assert.deepEqual(options.env, { PATH: '/usr/local/bin:/usr/bin:/bin', CMA_ATTESTATION_CONFIG: config });
    assert.deepEqual(JSON.parse(options.input), request);
    assert.ok(options.timeout <= 240000 && options.maxBuffer <= 65536);
    change?.();
    return JSON.stringify(receipt);
  });
  try {
    assert.deepEqual(invoke(), receipt);
    assert.throws(() => invoke(() => writeFileSync(config, 'changed-config')));
    writeFileSync(config, 'reviewed-config');
    assert.throws(() => invoke(() => writeFileSync(verifier, 'changed-verifier')));
    writeFileSync(verifier, 'reviewed-verifier');
    chmodSync(verifier, 0o644);
    assert.throws(() => invoke());
    chmodSync(verifier, 0o600);
    symlinkSync(verifier, join(directory, 'alias.mjs'));
    assert.throws(() => privateFile(join(directory, 'alias.mjs')));
    assert.throws(() => privateFile(resolve('.github/scripts/staging-authority.mjs')));
    assert.throws(() => runObserver(verifier, undefined, config, undefined, request));
  } finally {
    rmSync(directory, { recursive: true });
    for (const workdir of workdirs) assert.equal(existsSync(workdir), false);
  }
});

for (const outcome of ['success', 'child failure', 'invalid JSON', 'invalid receipt']) {
  test(`observer helper caches stay outside caller cwd and are cleaned after ${outcome}`, () => {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-observer-cache-test-')));
    const verifier = join(directory, 'observer.mjs'), config = join(directory, 'target.json');
    const { request, receipt } = fixtures();
    const cache = join('.kube', 'gke_gcloud_auth_plugin_cache');
    const caller = process.cwd();
    const callerCacheExists = existsSync(join(caller, cache));
    const output = outcome === 'invalid JSON' ? 'invalid-json' : JSON.stringify(outcome === 'invalid receipt' ? {} : receipt);
    const script = `
      import { mkdirSync, writeFileSync } from 'node:fs';
      mkdirSync('.kube');
      writeFileSync(${JSON.stringify(cache)}, 'dummy-helper-cache', { mode: 0o600 });
      ${outcome === 'child failure' ? 'process.exit(1);' : `process.stdout.write(${JSON.stringify(output)});`}
    `;
    writeFileSync(verifier, script, { mode: 0o600 });
    writeFileSync(config, 'reviewed-config', { mode: 0o600 });
    let workdir;
    let cacheCreated = false;
    const invoke = () => runObserver(verifier, hash(script), config, hash('reviewed-config'), request, (program, args, options) => {
      workdir = options.cwd;
      assert.ok(isAbsolute(workdir));
      assert.equal(realpathSync(workdir), workdir);
      assert.notEqual(workdir, caller);
      const outside = relative(caller, workdir);
      assert.ok(outside.startsWith('../') || isAbsolute(outside));
      const stat = lstatSync(workdir);
      assert.ok(stat.isDirectory() && !stat.isSymbolicLink());
      assert.equal(stat.uid, process.getuid());
      assert.equal(stat.mode & 0o777, 0o700);
      try { return execFileSync(program, args, options); }
      finally {
        assert.equal(readFileSync(join(workdir, cache), 'utf8'), 'dummy-helper-cache');
        cacheCreated = true;
      }
    });
    try {
      if (outcome === 'success') assert.deepEqual(invoke(), receipt);
      else assert.throws(invoke);
      assert.equal(cacheCreated, true);
      assert.equal(existsSync(workdir), false);
      assert.equal(process.cwd(), caller);
      assert.equal(existsSync(join(caller, cache)), callerCacheExists);
    } finally { rmSync(directory, { recursive: true }); }
  });
}

test('full campaign evidence refuses discovery, skips, owner-only runs and any unsettled cleanup', () => {
  const results = { status: 'passed', cases: requiredCases.map((name) => ({ case: name, status: 'passed', durationMs: 1 })) };
  const identity = { run: 'run-test', at: new Date().toISOString(), kind: 'sandboxes', id: 'sbx_test', subject: 'owner' };
  const journal = [{ ...identity, action: 'created' }, { ...identity, action: 'cleanup_complete' }];
  validateCampaign(results, journal);
  assert.throws(() => validateCampaign({ status: 'passed', cases: [] }, journal));
  assert.throws(() => validateCampaign({ status: 'passed', cases: [{ case: 'OWNER partial', status: 'passed' }] }, journal));
  assert.throws(() => validateCampaign({ ...results, cases: results.cases.map((row, index) => index ? row : { ...row, status: 'skipped' }) }, journal));
  for (const removed of requiredCases) assert.throws(() => validateCampaign({ ...results, cases: results.cases.filter((row) => row.case !== removed) }, journal), removed);
  assert.throws(() => validateCampaign({ ...results, cases: [...results.cases, results.cases[0]] }, journal));
  assert.throws(() => validateCampaign(results, [journal[0]]));
  assert.throws(() => validateCampaign(results, [...journal].reverse()));
  assert.throws(() => validateCampaign(results, []));
  assert.throws(() => validateCampaign(results, [{ action: 'created' }, { action: 'cleanup_complete' }]));
  assert.throws(() => validateCampaign(results, [journal[0], journal[0], journal[1]]));
  assert.throws(() => validateCampaign(results, [journal[0], { ...journal[1], subject: 'other' }]));
  for (const action of ['cleanup_failed', 'cleanup_pending', 'cleanup_blocked', 'external_account_cleanup_required']) assert.throws(() => validateCampaign(results, [...journal, { action }]));
});

test('both attestations are validated with a bounded three-hour campaign window', () => {
  const now = Date.now();
  const before = fixtures();
  const after = fixtures();
  before.receipt.observedAt = new Date(now - 170 * 60 * 1000).toISOString();
  after.request.nonce = after.receipt.nonce = '00000000-0000-4000-8000-000000000002';
  validateAttestations(before, after, now);
  for (const mutate of [
    (v) => { v.receipt.schema = 'operator-declared'; },
    (v) => { v.receipt.nonce = 'stale-nonce'; },
    (v) => { v.request.expected.commit = 'f'.repeat(40); },
    (v) => { v.receipt.images.api.source = 'https://example.invalid'; },
    (v) => { v.receipt.observedAt = new Date(now - 186 * 60 * 1000).toISOString(); },
    (v) => { v.receipt.observedAt = new Date(now + 1000).toISOString(); },
  ]) { const changed = structuredClone(before); mutate(changed); assert.throws(() => validateAttestations(changed, after, now)); }
  assert.throws(() => validateAttestations(after, after, now));
  after.receipt.observedAt = new Date(now - 301000).toISOString();
  assert.throws(() => validateAttestations(before, after, now));
});
