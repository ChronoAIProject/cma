import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { appendFileSync, lstatSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const digest = /^sha256:[0-9a-f]{64}$/;
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const keys = (value, names) => assert.deepEqual(Object.keys(value).sort(), [...names].sort());
const source = 'https://github.com/ChronoAIProject/cma';
export const requiredCases = [
  'SMOKE public discovery and landing are read-only',
  'SECURITY anonymous badge sets no cookie and references no credentials',
  'SMOKE anonymous console stays at the passive sign-in gate',
  ...['install_failure', 'clone_failure', 'worktree_conflict', 'agent_start_timeout', 'first_message_ambiguity', 'worker_crash'].map((name) => `FAULT-${name} records the real fault and preserves command identity`),
  'FAULT-nyxid-requirements upstream outage never invents satisfied requirements',
  'FAULT-browser-close durable Launch completes while its browser is closed',
  'FAULT-popup-blocked connection falls back to the same tab without duplicate handoff',
  'FAULT-handoff-response lost browser response keeps the same unsettled handoff command',
  'FAULT-nyxid real upstream unavailability preserves handoff command identity',
  'J0 new account returns to the same Trigger and reaches ready after one CMA click',
  'J1 always-new creates distinct visitor-owned Sandboxes',
  'J2 reuse Sandbox creates a new Workspace each time',
  'J3 reuse both supports ask resume, ask new and resume-existing',
  ...['oauth', 'api_key', 'device_code'].map((mode) => `J4-${mode} G-T2 missing service connection assistant continues via ${mode}`),
  'J7-retired Trigger cannot admit work',
  'J7-paused orchestration freezes a durable Launch without failing it',
  'J6 G-S1 G-P1 trusted Sandbox, two Agents, stop/delete, pause/revive and rotation preserve work',
  'SECURITY-negative visitor cannot create Sandboxes, profiles or Triggers',
  'SECURITY-csrf cross-site form POST is refused without admitting a Launch',
  'G-T3 fifty explicit clicks admit at most five Launches and expose resource_quota reset',
  'J5 profiles publish to Marketplace, second owner forks, wizard publishes embeds and stats',
  ...['markdown', 'html', 'javascript', 'react'].map((kind) => `G-T1-${kind} real embed click-through preserves placement attribution`),
];

export function privateFile(path, expectedHash) {
  assert.ok(isAbsolute(path) && path === realpathSync(path));
  const outside = relative(root, path);
  assert.ok(outside.startsWith('../') || isAbsolute(outside));
  const stat = lstatSync(path);
  assert.ok(stat.isFile() && !stat.isSymbolicLink() && stat.uid === process.getuid() && (stat.mode & 0o077) === 0);
  const bytes = readFileSync(path);
  if (expectedHash !== undefined) {
    assert.match(expectedHash, /^[0-9a-f]{64}$/);
    assert.equal(hash(bytes), expectedHash);
  }
  return bytes;
}

export function validatePolicy(environment, branches) {
  assert.equal(environment.name, 'staging-e2e-mutation');
  assert.equal(environment.can_admins_bypass, false);
  assert.deepEqual(environment.deployment_branch_policy, { protected_branches: false, custom_branch_policies: true });
  const reviewers = environment.protection_rules.filter((rule) => rule.type === 'required_reviewers');
  assert.equal(reviewers.length, 1);
  assert.equal(reviewers[0].prevent_self_review, true);
  assert.ok(reviewers[0].reviewers.length > 0);
  assert.equal(branches.total_count, 1);
  assert.equal(branches.branch_policies.length, 1);
  assert.equal(branches.branch_policies[0].name, 'develop');
  assert.equal(branches.branch_policies[0].type, 'branch');
}

export function validateReceipt(receipt, request, now = Date.now(), maxAge = 300000) {
  keys(request, ['schema', 'nonce', 'expected']);
  assert.equal(request.schema, 'cma-staging-attestation-request-v1');
  assert.match(request.nonce, /^[0-9a-f-]{36}$/);
  keys(request.expected, ['origin', 'commit', 'nyxidCommit', 'images']);
  assert.match(request.expected.commit, /^[0-9a-f]{40}$/);
  assert.match(request.expected.nyxidCommit, /^[0-9a-f]{40}$/);
  const origin = new URL(request.expected.origin);
  assert.ok(origin.protocol === 'https:' && origin.origin === request.expected.origin);
  keys(receipt, ['schema', 'nonce', 'observedAt', 'origin', 'commit', 'nyxidCommit', 'runtimePin', 'images']);
  assert.equal(receipt.schema, 'cma-staging-attestation-v1');
  assert.equal(receipt.nonce, request.nonce);
  const observed = Date.parse(receipt.observedAt);
  assert.ok(Number.isFinite(observed) && observed <= now + 5000 && now - observed <= maxAge);
  for (const field of ['origin', 'commit', 'nyxidCommit']) assert.equal(receipt[field], request.expected[field]);
  assert.equal(receipt.runtimePin, request.expected.images.runtime);
  keys(receipt.images, ['api', 'web', 'runtime']);
  keys(request.expected.images, ['api', 'web', 'runtime']);
  for (const name of ['api', 'web', 'runtime']) {
    const row = receipt.images[name];
    keys(row, ['reference', 'rootDigest', 'platformDigest', 'configDigest', 'source', 'revision', ...(name === 'runtime' ? [] : ['runningImageID'])]);
    assert.equal(row.reference, request.expected.images[name]);
    assert.equal(row.rootDigest, row.reference.slice(row.reference.lastIndexOf('@') + 1));
    for (const field of ['rootDigest', 'platformDigest', 'configDigest']) assert.match(row[field], digest);
    assert.equal(row.source, source);
    assert.equal(row.revision, request.expected.commit);
    if (name !== 'runtime') {
      assert.match(row.runningImageID, /@sha256:[0-9a-f]{64}$/);
      assert.equal(row.runningImageID.slice(row.runningImageID.lastIndexOf('@') + 1), row.platformDigest);
    }
  }
}

const github = (path) => JSON.parse(execFileSync('gh', ['api', path], { encoding: 'utf8', timeout: 30000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] }));
const repository = () => {
  assert.match(process.env.GITHUB_REPOSITORY, /^ChronoAIProject\/cma$/i);
  return `repos/${process.env.GITHUB_REPOSITORY}`;
};

function plan() {
  assert.equal(process.env.GITHUB_REF, 'refs/heads/develop');
  assert.match(process.env.REVIEWED_SHA, /^[0-9a-f]{40}$/);
  assert.equal(process.env.GITHUB_SHA, process.env.REVIEWED_SHA);
  assert.equal(github(`${repository()}/git/ref/heads/develop`).object.sha, process.env.REVIEWED_SHA);
  for (const key of ['VERIFIER_SHA256', 'CONFIG_SHA256']) assert.match(process.env[key], /^[0-9a-f]{64}$/);
  appendFileSync(process.env.GITHUB_OUTPUT, `sha=${process.env.REVIEWED_SHA}\nverifier=${process.env.VERIFIER_SHA256}\nconfig=${process.env.CONFIG_SHA256}\n`);
}

export function runObserver(verifier, verifierHash, config, configHash, request, execute = execFileSync) {
  assert.match(verifierHash, /^[0-9a-f]{64}$/);
  assert.match(configHash, /^[0-9a-f]{64}$/);
  privateFile(verifier, verifierHash);
  privateFile(config, configHash);
  const receipt = JSON.parse(execute(process.execPath, [verifier], {
    input: JSON.stringify(request), encoding: 'utf8', timeout: 240000, maxBuffer: 65536,
    env: { PATH: '/usr/local/bin:/usr/bin:/bin', CMA_ATTESTATION_CONFIG: config },
    stdio: ['pipe', 'pipe', 'pipe'],
  }));
  validateReceipt(receipt, request);
  privateFile(verifier, verifierHash);
  privateFile(config, configHash);
  return receipt;
}

function attest(output) {
  const environment = `${repository()}/environments/staging-e2e-mutation`;
  validatePolicy(github(environment), github(`${environment}/deployment-branch-policies`));
  assert.equal(process.env.GITHUB_REF, 'refs/heads/develop');
  assert.equal(execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), process.env.REVIEWED_SHA);
  const verifier = process.env.CMA_ATTESTATION_VERIFIER;
  const config = process.env.CMA_ATTESTATION_CONFIG;
  assert.match(process.env.VERIFIER_SHA256, /^[0-9a-f]{64}$/);
  assert.match(process.env.CONFIG_SHA256, /^[0-9a-f]{64}$/);
  const fixtureBytes = privateFile(process.env.CMA_E2E_FIXTURE_FILE);
  const fixture = JSON.parse(fixtureBytes);
  assert.equal(fixture.origin, process.env.CMA_E2E_MUTATION_TARGET);
  assert.equal(fixture.origin, process.env.CMA_E2E_BASE_URL);
  assert.equal(fixture.fixtureOwner, process.env.CMA_E2E_FIXTURE_OWNER);
  const origin = new URL(fixture.origin);
  assert.ok(origin.protocol === 'https:' && !origin.username && !origin.password && origin.origin === fixture.origin);
  const provenance = fixture.provenance;
  assert.equal(provenance.commit, process.env.REVIEWED_SHA);
  assert.match(provenance.nyxidCommit, /^[0-9a-f]{40}$/);
  const images = {};
  for (const component of ['api', 'web', 'runtime']) {
    images[component] = provenance[`${component}Image`];
    assert.match(images[component], /^[a-z0-9][a-z0-9./:_-]*@sha256:[0-9a-f]{64}$/);
  }
  const request = { schema: 'cma-staging-attestation-request-v1', nonce: randomUUID(), expected: { origin: fixture.origin, commit: provenance.commit, nyxidCommit: provenance.nyxidCommit, images } };
  const receipt = runObserver(verifier, process.env.VERIFIER_SHA256, config, process.env.CONFIG_SHA256, request);
  assert.equal(hash(privateFile(process.env.CMA_E2E_FIXTURE_FILE)), hash(fixtureBytes));
  writeFileSync(output, JSON.stringify({ request, receipt }), { flag: 'wx', mode: 0o600 });
}

export function validateCampaign(results, journal) {
  keys(results, ['status', 'cases']);
  assert.equal(results.status, 'passed');
  assert.deepEqual(results.cases.map((row) => row.case).sort(), [...requiredCases].sort());
  for (const row of results.cases) {
    keys(row, ['case', 'status', 'durationMs']);
    assert.equal(row.status, 'passed');
    assert.ok(Number.isFinite(row.durationMs) && row.durationMs >= 0);
  }
  assert.ok(Array.isArray(journal));
  assert.ok(journal.length > 0);
  assert.ok(journal.some((row) => row.action === 'created'));
  assert.ok(!journal.some((row) => ['cleanup_refused', 'cleanup_blocked', 'cleanup_read_failed', 'cleanup_failed', 'cleanup_pending', 'stop_pending', 'external_account_cleanup_required'].includes(row.action)));
  const identity = (row) => JSON.stringify([row.run, row.kind, row.id, row.subject]);
  const owned = new Set();
  for (const row of journal) {
    assert.ok(row && typeof row === 'object' && !Array.isArray(row));
    assert.match(row.run, /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/);
    assert.ok(Number.isFinite(Date.parse(row.at)));
    assert.ok(['created', 'cancel_requested', 'launch_observed', 'already_absent', 'stop_requested', 'cleanup_accepted', 'cleanup_complete'].includes(row.action));
    assert.ok(['launches', 'sandboxes', 'workspaces', 'agents', 'agent-profiles', 'workspace-profiles', 'sandbox-profiles', 'triggers'].includes(row.kind));
    for (const key of ['id', 'subject']) assert.match(row[key], /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}$/);
    if (row.action === 'created') { assert.ok(!owned.has(identity(row))); owned.add(identity(row)); }
    else assert.ok(owned.has(identity(row)), 'cleanup cannot precede ownership');
  }
  for (const [index, created] of journal.entries()) {
    if (created.action !== 'created') continue;
    assert.ok(journal.slice(index + 1).some((row) => identity(row) === identity(created) && (
      ['cleanup_complete', 'already_absent'].includes(row.action) ||
      (created.kind === 'launches' && row.action === 'launch_observed' && ['ready', 'completed', 'cancelled', 'failed_terminal'].includes(row.state))
    )));
  }
}

export function validateAttestations(before, after, now = Date.now()) {
  keys(before, ['request', 'receipt']);
  keys(after, ['request', 'receipt']);
  validateReceipt(before.receipt, before.request, now, 185 * 60 * 1000);
  validateReceipt(after.receipt, after.request, now);
  assert.notEqual(before.request.nonce, after.request.nonce);
  assert.ok(Date.parse(before.receipt.observedAt) <= Date.parse(after.receipt.observedAt));
  assert.deepEqual(after.request.expected, before.request.expected);
  assert.deepEqual(after.receipt.images, before.receipt.images);
}

function inventory() {
  const listing = JSON.parse(execFileSync(process.execPath, [join(root, 'web/node_modules/@playwright/test/cli.js'), 'test', '--config', 'e2e/playwright.config.ts', '--list', '--reporter=json'], {
    cwd: join(root, 'web'), env: { ...process.env, CMA_E2E_MODE: 'mutation' }, encoding: 'utf8', timeout: 60000, maxBuffer: 1024 * 1024,
  }));
  assert.deepEqual(listing.errors, []);
  const titles = [];
  const visit = (suite) => {
    for (const spec of suite.specs ?? []) {
      assert.equal(spec.tests.length, 1);
      assert.equal(spec.tests[0].expectedStatus, 'passed');
      assert.ok(!spec.tests[0].annotations.some((row) => ['skip', 'fixme'].includes(row.type)));
      titles.push(spec.title);
    }
    for (const child of suite.suites ?? []) visit(child);
  };
  for (const suite of listing.suites) visit(suite);
  assert.deepEqual(titles.sort(), [...requiredCases].sort());
}

function audit(directory, beforePath, afterPath) {
  const before = JSON.parse(privateFile(beforePath));
  const after = JSON.parse(privateFile(afterPath));
  validateAttestations(before, after);
  assert.equal(after.request.expected.commit, process.env.REVIEWED_SHA);
  assert.equal(after.request.expected.origin, process.env.CMA_E2E_MUTATION_TARGET);
  const results = JSON.parse(readFileSync(join(directory, 'results.json'), 'utf8'));
  const journal = readFileSync(join(directory, 'cleanup.ndjson'), 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  validateCampaign(results, journal);
  writeFileSync(join(directory, 'attestation.json'), JSON.stringify({ before: before.receipt, after: after.receipt, scope: 'independent-deployment-identity-not-model-proof' }), { flag: 'wx', mode: 0o600 });
  appendFileSync(process.env.GITHUB_OUTPUT, 'publishable=true\n');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [mode, ...args] = process.argv.slice(2);
    if (mode === 'plan' && args.length === 0) plan();
    else if (mode === 'inventory' && args.length === 0) inventory();
    else if (mode === 'attest' && args.length === 1) attest(...args);
    else if (mode === 'audit' && args.length === 3) audit(...args);
    else throw Error('invalid invocation');
  } catch { process.stderr.write('Staging authority or final evidence refused; private inputs withheld\n'); process.exitCode = 1; }
}
