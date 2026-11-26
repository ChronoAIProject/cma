import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { appendFileSync, lstatSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { currentJourneyCases, validatePublicJourneyProof } from './public-journey-evidence.mjs';
import { connectionCaseTitles } from './connection-journey-evidence.mjs';
import { bindConnectionFixtures, requireConnectionCoverage, validateConnectionCases } from './connection-campaign.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const digest = /^sha256:[0-9a-f]{64}$/;
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const keys = (value, names) => assert.deepEqual(Object.keys(value).sort(), [...names].sort());
const source = 'https://github.com/ChronoAIProject/cma';
export const observerCodes = Object.freeze([
  'absolute_real_path_required', 'ambiguous_registry_platform', 'ambiguous_running_platform',
  'api_configmap_explicitly_overridden', 'api_configmap_not_bound', 'api_configmap_prefixed', 'api_configmap_shadowed',
  'canonical_expected_reference_required', 'command_or_input_incomplete', 'conditions_missing',
  'config_changed_during_observation', 'config_platform_mismatch', 'config_too_large', 'configmap_identity_incomplete',
  'configured_runtime_or_origin_mismatch', 'container_not_ready', 'container_projection_incomplete', 'container_status_incomplete',
  'deployment_changed_during_observation', 'deployment_generation_unobserved', 'deployment_identity_or_deletion',
  'deployment_image_mismatch', 'deployment_not_settled', 'deployment_unavailable', 'duplicate_container',
  'environment_source_identity_incomplete', 'environment_source_keys_incomplete', 'environment_sources_exceeded',
  'explicit_registry_required', 'foreign_resource_owner', 'invalid_config_schema', 'invalid_deployment_generation_or_scale',
  'invalid_environment_source', 'invalid_kubernetes_projection', 'invalid_nyxid_source', 'invalid_origin',
  'invalid_registry_descriptor', 'invalid_registry_document', 'invalid_registry_index', 'invalid_request_schema',
  'invalid_schema', 'invalid_service_port', 'invalid_tool_size', 'nested_registry_index_refused', 'node_platform_incomplete',
  'nyxid_scope_incomplete', 'observation_deadline', 'owner_uid_missing', 'ownership_missing', 'pod_identity_or_deletion',
  'pod_not_running', 'pod_owner_name_mismatch', 'pod_owner_uid_mismatch', 'pod_population_not_settled', 'pod_selector_mismatch',
  'pod_template_images_mismatch', 'private_file_permissions_required', 'registry_content_digest_mismatch',
  'registry_source_or_revision_mismatch', 'regular_file_required', 'replicaset_identity_or_deletion', 'request_too_large',
  'resource_not_ready', 'route_backend_mismatch', 'route_filter_or_backend_ambiguity', 'route_identity_incomplete',
  'route_origin_mismatch', 'route_parent_mismatch', 'route_parent_status_missing', 'route_path_mapping_mismatch',
  'running_image_id_missing', 'running_image_is_not_platform_manifest', 'running_repository_mismatch', 'runtime_contract_mismatch',
  'service_identity_incomplete', 'service_pod_identity_mismatch', 'service_pod_population_mismatch', 'service_pod_selector_mismatch',
  'service_port_mismatch', 'service_selector_mismatch', 'service_selector_missing', 'service_target_port_mismatch',
  'stale_route_status', 'symlink_refused', 'target_container_missing', 'target_container_status_missing', 'tool_digest_mismatch',
  'trusted_executable_required', 'unexpected_arguments', 'unsupported_configmap_keys', 'unsupported_deployment_selector',
  'unsupported_image_config', 'unsupported_manifest_schema', 'unsupported_platform_manifest', 'unsupported_runtime_platform',
]);
export const phases = Object.freeze([
  'invocation', 'plan_reviewed_commit', 'plan_develop_head', 'plan_authority_hash', 'plan_output',
  'environment_policy', 'checkout_commit', 'authority_hash', 'fixture_file', 'fixture_binding', 'observer', 'receipt',
  'fixture_changed', 'attestation_output', 'attestation_files', 'attestations', 'expected_identity', 'campaign_results',
  'cleanup_journal', 'connection_fixtures', 'connection_coverage', 'audit_output', 'inventory_listing', 'inventory_cases', 'unclassified',
]);
const signals = Object.freeze(['SIGTERM', 'SIGINT', 'SIGKILL', 'SIGHUP']);
export const refusalCodes = Object.freeze([
  ...phases.filter((phase) => phase !== 'invocation'), 'invalid_invocation', 'github_api', 'observer_timeout', 'observer_signal',
  ...signals.map((signal) => `observer_signal:${signal}`), ...observerCodes.map((code) => `observer:${code}`),
]);
export class AuthorityError extends Error {
  constructor(code) { super(refusalCodes.includes(code) ? code : 'unclassified'); }
}
export const check = (condition, code) => { if (!condition) throw new AuthorityError(code); };
export function withPhase(phase, action) {
  check(phases.includes(phase), 'unclassified');
  try {
    return action();
  } catch (error) {
    const refusal = error instanceof AuthorityError ? error : new AuthorityError(phase);
    if (refusal.phase === undefined) refusal.phase = phase;
    throw refusal;
  }
}
export function formatRefusal(error, mode, step) {
  mode = ['plan', 'inventory', 'attest', 'audit'].includes(mode) ? mode : 'invalid';
  const code = error instanceof AuthorityError && refusalCodes.includes(error.message) ? error.message : 'unclassified';
  let phase = 'unclassified';
  if (error instanceof AuthorityError) {
    if (phases.includes(error.phase)) phase = error.phase;
    else if (error.phase === undefined && code === 'invalid_invocation') phase = 'invocation';
  }
  const action = typeof step === 'string' && /^[A-Za-z0-9_-]{1,64}$/.exec(step)?.[0] === step ? ` step=${step}` : '';
  return `Staging authority or final evidence refused (mode=${mode}${action} phase=${phase} code=${code}); private inputs withheld.\n`;
}
export function classifyObserverError(error, elapsedMs = 0) {
  const stderr = typeof error?.stderr === 'string' || Buffer.isBuffer(error?.stderr) ? error.stderr.toString() : '';
  const last = stderr.split(/\r?\n/).filter((line) => line.trim().length > 0).at(-1) ?? '';
  const match = /^Deployment observation incomplete \(([a-z_]{1,64})\); raw diagnostics withheld\.$/.exec(last);
  if (match?.[0] === last && observerCodes.includes(match?.[1])) return `observer:${match[1]}`;
  if (error?.code === 'ETIMEDOUT' || (error?.signal === 'SIGTERM' && elapsedMs >= 240000)) return 'observer_timeout';
  if (error?.signal) return signals.includes(error.signal) ? `observer_signal:${error.signal}` : 'observer_signal';
  return 'observer';
}
const commandInput = (action) => { try { return action(); } catch { check(false, 'github_api'); } };
const commitInput = (action) => commandInput(() => {
  const value = action();
  check(typeof value === 'string' && /^[0-9a-f]{40}$/.test(value), 'github_api');
  return value;
});

export const requiredCases = [
  'SMOKE public discovery and landing are read-only',
  'SECURITY anonymous badge sets no cookie and references no credentials',
  'SMOKE anonymous console stays at the passive sign-in gate',
  ...['install_failure', 'clone_failure', 'worktree_conflict', 'agent_start_timeout', 'first_message_ambiguity', 'worker_crash'].map((name) => `FAULT-${name} records the real fault and preserves command identity`),
  'FAULT-nyxid-requirements upstream outage never invents satisfied requirements',
  'FAULT-browser-close durable Launch completes while its browser is closed',
  connectionCaseTitles['FAULT-popup-blocked'],
  connectionCaseTitles['FAULT-handoff-response'],
  connectionCaseTitles['FAULT-nyxid'],
  'J0 new account returns to the same Trigger and reaches ready after one CMA click',
  'J1 always-new creates distinct visitor-owned Sandboxes',
  'J2 reuse Sandbox creates a new Workspace each time',
  'J3 reuse both supports ask resume, ask new and resume-existing',
  ...['oauth', 'api_key', 'device_code'].map((mode) => connectionCaseTitles[`J4-${mode}`]),
  'J7-retired Trigger cannot admit work',
  'J7-paused orchestration freezes a durable Launch without failing it',
  currentJourneyCases.j6,
  currentJourneyCases.compatibility,
  'SECURITY-negative visitor cannot create Sandboxes, profiles or Triggers',
  'SECURITY-csrf cross-site form POST is refused without admitting a Launch',
  'G-T3 fifty explicit clicks admit at most five Launches and expose resource_quota reset',
  currentJourneyCases.j5,
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

const github = (path) => commandInput(() => JSON.parse(execFileSync('gh', ['api', path], { encoding: 'utf8', timeout: 30000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] })));
const repository = () => {
  assert.match(process.env.GITHUB_REPOSITORY, /^ChronoAIProject\/cma$/i);
  return `repos/${process.env.GITHUB_REPOSITORY}`;
};

function plan() {
  withPhase('plan_reviewed_commit', () => {
    assert.equal(process.env.GITHUB_REF, 'refs/heads/develop');
    assert.match(process.env.REVIEWED_SHA, /^[0-9a-f]{40}$/);
    assert.equal(process.env.GITHUB_SHA, process.env.REVIEWED_SHA);
  });
  withPhase('plan_develop_head', () => {
    const path = `${repository()}/git/ref/heads/develop`;
    assert.equal(commitInput(() => github(path).object.sha), process.env.REVIEWED_SHA);
  });
  withPhase('plan_authority_hash', () => {
    for (const key of ['VERIFIER_SHA256', 'CONFIG_SHA256']) assert.match(process.env[key], /^[0-9a-f]{64}$/);
  });
  withPhase('plan_output', () => appendFileSync(process.env.GITHUB_OUTPUT, `sha=${process.env.REVIEWED_SHA}\nverifier=${process.env.VERIFIER_SHA256}\nconfig=${process.env.CONFIG_SHA256}\n`));
}

export function runObserver(verifier, verifierHash, config, configHash, request, execute = execFileSync) {
  withPhase('authority_hash', () => {
    assert.match(verifierHash, /^[0-9a-f]{64}$/);
    assert.match(configHash, /^[0-9a-f]{64}$/);
    privateFile(verifier, verifierHash);
    privateFile(config, configHash);
  });
  // Credential helpers may fall back to cwd when HOME is absent.
  const directory = withPhase('observer', () => mkdtempSync(join(realpathSync(tmpdir()), 'cma-observer-workdir-')));
  try {
    const stdout = withPhase('observer', () => {
      const outside = relative(root, directory);
      assert.ok(outside.startsWith('../') || isAbsolute(outside));
      const started = performance.now();
      try {
        return execute(process.execPath, [verifier], {
          input: JSON.stringify(request), encoding: 'utf8', timeout: 240000, maxBuffer: 65536,
          cwd: directory,
          env: { PATH: '/usr/local/bin:/usr/bin:/bin', CMA_ATTESTATION_CONFIG: config },
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      } catch (error) { check(false, classifyObserverError(error, performance.now() - started)); }
    });
    const receipt = withPhase('receipt', () => {
      const value = JSON.parse(stdout);
      validateReceipt(value, request);
      return value;
    });
    withPhase('authority_hash', () => { privateFile(verifier, verifierHash); privateFile(config, configHash); });
    return receipt;
  } finally { withPhase('observer', () => rmSync(directory, { recursive: true, force: true })); }
}

function attest(output) {
  withPhase('environment_policy', () => {
    const environment = `${repository()}/environments/staging-e2e-mutation`;
    validatePolicy(github(environment), github(`${environment}/deployment-branch-policies`));
  });
  withPhase('checkout_commit', () => {
    assert.equal(process.env.GITHUB_REF, 'refs/heads/develop');
    assert.equal(commitInput(() => execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()), process.env.REVIEWED_SHA);
  });
  const verifier = process.env.CMA_ATTESTATION_VERIFIER;
  const config = process.env.CMA_ATTESTATION_CONFIG;
  withPhase('authority_hash', () => {
    assert.match(process.env.VERIFIER_SHA256, /^[0-9a-f]{64}$/);
    assert.match(process.env.CONFIG_SHA256, /^[0-9a-f]{64}$/);
  });
  const fixtureBytes = withPhase('fixture_file', () => privateFile(process.env.CMA_E2E_FIXTURE_FILE));
  const request = withPhase('fixture_binding', () => {
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
    return { schema: 'cma-staging-attestation-request-v1', nonce: randomUUID(), expected: { origin: fixture.origin, commit: provenance.commit, nyxidCommit: provenance.nyxidCommit, images } };
  });
  const receipt = runObserver(verifier, process.env.VERIFIER_SHA256, config, process.env.CONFIG_SHA256, request);
  withPhase('fixture_changed', () => assert.equal(hash(privateFile(process.env.CMA_E2E_FIXTURE_FILE)), hash(fixtureBytes)));
  withPhase('attestation_output', () => writeFileSync(output, JSON.stringify({ request, receipt }), { flag: 'wx', mode: 0o600 }));
}

export function validateCampaign(results, journal) {
  withPhase('campaign_results', () => {
    keys(results, ['status', 'cases']);
    assert.equal(results.status, 'passed');
    assert.deepEqual(results.cases.map((row) => row.case).sort(), [...requiredCases].sort());
    for (const row of results.cases) {
      const current = [currentJourneyCases.j5, currentJourneyCases.j6].includes(row.case);
      const connection = Object.values(connectionCaseTitles).includes(row.case);
      keys(row, ['case', 'status', 'durationMs', ...(current ? ['proof'] : []), ...(connection ? ['connectionProof'] : [])]);
      assert.equal(row.status, 'passed');
      assert.ok(Number.isFinite(row.durationMs) && row.durationMs >= 0);
    }
  });
  withPhase('cleanup_journal', () => {
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
      assert.ok(['created', 'cancel_requested', 'launch_observed', 'lifecycle_observed', 'already_absent', 'stop_requested', 'cleanup_accepted', 'cleanup_complete'].includes(row.action));
      assert.ok(['agent-lifecycle', 'launches', 'sandboxes', 'workspaces', 'agents', 'agent-profiles', 'workspace-profiles', 'sandbox-profiles', 'triggers'].includes(row.kind));
      for (const key of ['id', 'subject']) assert.match(row[key], /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}$/);
      if (row.action === 'created') { assert.ok(!owned.has(identity(row))); owned.add(identity(row)); }
      else assert.ok(owned.has(identity(row)), 'cleanup cannot precede ownership');
      if (row.action === 'lifecycle_observed') {
        assert.equal(row.kind, 'agent-lifecycle');
        assert.ok(['queued', 'running', 'waiting_for_connection', 'succeeded', 'failed'].includes(row.state));
        if (row.state === 'succeeded' || row.agent_id !== undefined) {
          assert.match(row.agent_id, /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}$/);
          assert.ok(owned.has(identity({ ...row, kind: 'agents', id: row.agent_id })));
        }
      }
    }
    for (const [index, created] of journal.entries()) {
      if (created.action !== 'created') continue;
      assert.ok(journal.slice(index + 1).some((row) => identity(row) === identity(created) && (
        (created.kind !== 'agent-lifecycle' && ['cleanup_complete', 'already_absent'].includes(row.action)) ||
        (created.kind === 'launches' && row.action === 'launch_observed' && ['ready', 'completed', 'cancelled', 'failed_terminal'].includes(row.state)) ||
        (created.kind === 'agent-lifecycle' && row.action === 'lifecycle_observed' && ['succeeded', 'failed'].includes(row.state))
      )));
    }
  });
  withPhase('campaign_results', () => {
    for (const row of results.cases)
      if ([currentJourneyCases.j5, currentJourneyCases.j6].includes(row.case))
        validatePublicJourneyProof(row.case, row.proof, journal);
    validateConnectionCases(results, journal);
  });
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
  const listing = withPhase('inventory_listing', () => {
    const value = JSON.parse(execFileSync(process.execPath, [join(root, 'web/node_modules/@playwright/test/cli.js'), 'test', '--config', 'e2e/playwright.config.ts', '--list', '--reporter=json'], {
      cwd: join(root, 'web'), env: { ...process.env, CMA_E2E_MODE: 'mutation' }, encoding: 'utf8', timeout: 60000, maxBuffer: 1024 * 1024,
    }));
    assert.deepEqual(value.errors, []);
    return value;
  });
  withPhase('inventory_cases', () => {
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
  });
}

function audit(directory, beforePath, afterPath) {
  const before = withPhase('attestation_files', () => JSON.parse(privateFile(beforePath)));
  const after = withPhase('attestation_files', () => JSON.parse(privateFile(afterPath)));
  withPhase('attestations', () => validateAttestations(before, after));
  withPhase('expected_identity', () => {
    assert.equal(after.request.expected.commit, process.env.REVIEWED_SHA);
    assert.equal(after.request.expected.origin, process.env.CMA_E2E_MUTATION_TARGET);
  });
  const results = withPhase('campaign_results', () => JSON.parse(readFileSync(join(directory, 'results.json'), 'utf8')));
  const journal = withPhase('cleanup_journal', () => readFileSync(join(directory, 'cleanup.ndjson'), 'utf8').trim().split('\n').map((line) => JSON.parse(line)));
  validateCampaign(results, journal);
  const connectionProofs = withPhase('connection_fixtures', () => {
    const fixture = JSON.parse(privateFile(process.env.CMA_E2E_FIXTURE_FILE));
    const proofs = validateConnectionCases(results, journal);
    assert.equal(fixture.origin, after.request.expected.origin);
    assert.equal(fixture.fixtureOwner, process.env.CMA_E2E_FIXTURE_OWNER);
    bindConnectionFixtures(proofs, fixture, after.request.expected.nyxidCommit);
    return proofs;
  });
  withPhase('connection_coverage', () => requireConnectionCoverage(connectionProofs));
  withPhase('audit_output', () => {
    writeFileSync(join(directory, 'attestation.json'), JSON.stringify({ before: before.receipt, after: after.receipt, scope: 'independent-deployment-identity-not-model-proof' }), { flag: 'wx', mode: 0o600 });
    appendFileSync(process.env.GITHUB_OUTPUT, 'publishable=true\n');
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // No signal listeners: synchronous work would delay runner cancellation by up to 240 seconds.
  // Only child cancellation is classified; killing this process emits nothing, as before.
  const [mode, ...args] = process.argv.slice(2);
  try {
    if (mode === 'plan' && args.length === 0) plan();
    else if (mode === 'inventory' && args.length === 0) inventory();
    else if (mode === 'attest' && args.length === 1) attest(...args);
    else if (mode === 'audit' && args.length === 3) audit(...args);
    else check(false, 'invalid_invocation');
  } catch (error) {
    process.stderr.write(formatRefusal(error, mode, process.env.GITHUB_ACTION));
    process.exitCode = 1;
  }
}
