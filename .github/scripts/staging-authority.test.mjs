import { hash, fixtures, canaries, publicDiagnostic, line, noCanaries, harness } from './staging-authority-cli.fixture.mjs';
import { campaignFixture } from './public-journey-evidence.fixture.mjs';
import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { chmodSync, existsSync, lstatSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { AuthorityError, check, classifyObserverError, formatRefusal, observerCodes, phases, refusalCodes, withPhase, privateFile, requiredCases, runObserver, validateAttestations, validateCampaign, validatePolicy, validateReceipt } from './staging-authority.mjs';

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
  const { results, journal: proofJournal } = campaignFixture(requiredCases);
  const identity = { run: 'run-test', at: new Date().toISOString(), kind: 'sandboxes', id: 'sbx_test', subject: 'owner' };
  const journal = [{ ...identity, action: 'created' }, { ...identity, action: 'cleanup_complete' }, ...proofJournal];
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

test('task cleanup requires an owned terminal operation and its discovered Agent cleanup', () => {
  const { results, journal: proofJournal } = campaignFixture(requiredCases);
  const operation = { run: 'run-test', at: new Date().toISOString(), kind: 'agent-lifecycle', id: 'operation-test', subject: 'owner' };
  const agent = { ...operation, kind: 'agents', id: 'agent-test' };
  const journal = [
    { ...operation, action: 'created' },
    { ...agent, action: 'created' },
    { ...operation, action: 'lifecycle_observed', state: 'succeeded', agent_id: agent.id },
    { ...agent, action: 'cleanup_complete' },
    ...proofJournal,
  ];
  validateCampaign(results, journal);
  for (const state of ['queued', 'running', 'waiting_for_connection'])
    assert.throws(() => validateCampaign(results, journal.map((row, index) => index === 2 ? { ...row, state } : row)));
  assert.throws(() => validateCampaign(results, journal.filter((_, index) => index !== 2)));
  assert.throws(() => validateCampaign(results, journal.filter((_, index) => index !== 3)));
  assert.throws(() => validateCampaign(results, journal.map((row, index) => index === 2 ? { ...row, agent_id: 'other-agent' } : row)));
  assert.throws(() => validateCampaign(results, journal.map((row, index) => index === 2 ? { ...row, subject: 'other-owner' } : row)));
  validateCampaign(results, [journal[0], { ...operation, action: 'lifecycle_observed', state: 'failed' }, ...proofJournal]);
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

const scenarios = [
  ['plan', 'plan_reviewed_commit', (h) => { h.env.REVIEWED_SHA = canaries[2]; }],
  ['plan', 'plan_develop_head', (h) => { h.state.head = 'd'.repeat(40); }],
  ['plan', 'plan_authority_hash', (h) => { h.env.CONFIG_SHA256 = ''; }],
  ['attest', 'environment_policy', (h) => { h.state.policy.can_admins_bypass = true; }],
  ['attest', 'checkout_commit', (h) => { h.env.REVIEWED_SHA = canaries[2]; }],
  ['attest', 'authority_hash', (h) => { h.env.VERIFIER_SHA256 = ''; }],
  ['attest', 'authority_hash', (h) => { h.env.CONFIG_SHA256 = 'a'.repeat(64); }],
  ['attest', 'authority_hash', (h) => h.setObserver(h.observer + `writeFileSync(${JSON.stringify(h.path('config.json'))}, 'changed');`)],
  ['attest', 'fixture_file', (h) => { h.env.CMA_E2E_FIXTURE_FILE = h.path('absent'); }],
  ['attest', 'fixture_binding', (h) => { h.env.CMA_E2E_MUTATION_TARGET = `https://${canaries[0]}.example.test`; }],
  ['attest', 'fixture_binding', (h) => { h.env.CMA_E2E_FIXTURE_OWNER = canaries[1]; }],
  ['attest', 'fixture_binding', (h) => h.write('fixture.json', { ...h.fixture, fixtureOwner: canaries[3] })],
  ['attest', 'fixture_binding', (h) => h.write('fixture.json', canaries[3])],
  ['attest', 'observer', (h) => h.setObserver(`process.stdout.write('${canaries[4]}'); process.stderr.write('${canaries[5]}'); process.exit(1);`)],
  ['attest', 'observer', (h) => h.setObserver(`process.stderr.write('${canaries[5]}\\nDeployment observation incomplete (invalid_schema); raw diagnostics withheld.\\n'); process.exit(1);`), 'observer:invalid_schema'],
  ['attest', 'observer', (h) => h.setObserver('process.kill(process.pid, "SIGTERM");'), 'observer_signal:SIGTERM'],
  ['attest', 'receipt', (h) => h.setObserver(`process.stdout.write('${canaries[4]}'); process.stderr.write('${canaries[5]}');`)],
  ['attest', 'receipt', (h) => h.setObserver(h.observer.replace('receipt.nonce = request.nonce;', `receipt.nonce = '${canaries[4]}';`))],
  ['attest', 'fixture_changed', (h) => h.setObserver(h.observer + `writeFileSync(${JSON.stringify(h.path('fixture.json'))}, '${canaries[3]}');`)],
  ['attest', 'attestation_output', (h) => h.write('attested.json', 'existing')],
  ['audit', 'attestation_files', (h) => h.write('before.json', canaries[3])],
  ['audit', 'attestations', (h) => h.write('before.json', { ...h.before, receipt: { ...h.before.receipt, origin: canaries[4] } })],
  ['audit', 'expected_identity', (h) => { h.env.REVIEWED_SHA = canaries[2]; }],
  ['audit', 'campaign_results', (h) => h.write('results.json', { status: 'passed', cases: [] })],
  ['audit', 'campaign_results', (h) => h.write('results.json', canaries[3])],
  ['audit', 'cleanup_journal', (h) => h.write('cleanup.ndjson', canaries[3])],
  ['audit', 'cleanup_journal', (h) => h.write('cleanup.ndjson', h.journal[0])],
  ['audit', 'connection_coverage', (h) => h.write('attestation.json', 'existing')],
  ['audit', 'connection_coverage', (h) => { h.env.GITHUB_OUTPUT = h.path('absent/output'); }],
  ['inventory', 'inventory_listing', (h) => { h.state.listing.errors = [canaries[3]]; }],
  ['inventory', 'inventory_listing', (h) => { h.state.fail = process.execPath; }],
  ['inventory', 'inventory_listing', (h) => { h.state.listingRaw = canaries[4]; }],
  ['inventory', 'inventory_cases', (h) => { h.state.listing.suites = []; }],
  ['plan', 'plan_develop_head', (h) => { h.state.fail = 'gh'; }, 'github_api'],
  ['plan', 'plan_develop_head', (h) => { h.state.ghRaw = canaries[4]; }, 'github_api'],
  ['plan', 'plan_develop_head', (h) => { h.state.ghRaw = '{}'; }, 'github_api'],
  ['plan', 'plan_develop_head', (h) => { h.state.ghRaw = '{"object":{}}'; }, 'github_api'],
  ['plan', 'plan_develop_head', (h) => { h.state.head = canaries[4]; }, 'github_api'],
  ['attest', 'checkout_commit', (h) => { h.state.fail = 'git'; }, 'github_api'],
  ['attest', 'checkout_commit', (h) => { h.state.head = ''; }, 'github_api'],
  ['plan', 'plan_output', (h) => { h.env.GITHUB_OUTPUT = h.path('absent/output'); }],
];
for (const [index, [mode, phase, mutate, code = phase]] of scenarios.entries()) test(`CLI refuses ${mode}/${phase}/${code} scenario ${index}`, (t) => {
  const h = harness(t); mutate(h);
  const result = h.run(mode);
  assert.equal(result.status, 1); assert.equal(result.stdout, ''); assert.equal(result.stderr, line(mode, phase, code));
});

for (const [mode, program, phase, code, diagnostic] of [
  ['attest', 'git', 'checkout_commit', 'github_api', publicDiagnostic],
  ['inventory', process.execPath, 'inventory_listing', 'inventory_listing', publicDiagnostic],
  ['plan', 'gh', 'plan_develop_head', 'github_api', ''],
]) test(`CLI preserves child stderr behavior for ${mode}`, (t) => {
  const h = harness(t); h.state.diagnosticProgram = program;
  const result = h.run(mode);
  assert.equal(result.status, 1); assert.equal(result.stdout, '');
  assert.equal(result.stderr, diagnostic + line(mode, phase, code));
});

test('CLI invocation and step metadata are bounded and never echo arguments', (t) => {
  const h = harness(t);
  for (const step of ['attestation', 'final_attestation', 'A-0_'.repeat(16), '', 'with spaces', 'a'.repeat(65), '非ASCII', 'trailing\n']) {
    h.env.GITHUB_ACTION = step;
    const allowed = ['attestation', 'final_attestation', 'A-0_'.repeat(16)].includes(step) ? step : '';
    const result = h.run(canaries[3]);
    assert.equal(result.status, 1); assert.equal(result.stdout, '');
    assert.equal(result.stderr, line('invalid', 'invocation', 'invalid_invocation', allowed));
  }
  delete h.env.GITHUB_ACTION;
  for (const mode of ['plan', 'inventory', 'attest', 'audit']) {
    const result = h.run(mode, [canaries[3], canaries[4]]);
    assert.equal(result.status, 1); assert.equal(result.stdout, '');
    assert.equal(result.stderr, line(mode, 'invocation', 'invalid_invocation', ''));
  }
});

for (const mode of ['plan', 'inventory', 'attest', 'audit']) test(`CLI preserves ${mode} output policy`, (t) => {
  const h = harness(t), result = h.run(mode);
  if (mode === 'audit') {
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, line('audit', 'connection_coverage'));
    assert.equal(readFileSync(h.path('output'), 'utf8'), '');
    assert.equal(existsSync(h.path('attestation.json')), false);
    return;
  }
  assert.equal(result.status, 0); assert.equal(result.stdout, ''); assert.equal(result.stderr, '');
  const output = mode === 'plan' ? `sha=${h.env.REVIEWED_SHA}\nverifier=${h.env.VERIFIER_SHA256}\nconfig=${h.env.CONFIG_SHA256}\n` : '';
  assert.equal(readFileSync(h.path('output'), 'utf8'), output);
  if (mode === 'attest') {
    const path = h.path('attested.json');
    const bytes = readFileSync(path, 'utf8'), value = JSON.parse(bytes);
    assert.equal(lstatSync(path).mode & 0o777, 0o600);
    assert.deepEqual(value.request.expected, h.before.request.expected);
    assert.equal(bytes, JSON.stringify({ request: value.request, receipt: { ...h.before.receipt, nonce: value.request.nonce } }));
  }
});

const observerLine = (code) => `Deployment observation incomplete (${code}); raw diagnostics withheld.`;
const observerFailures = [
  ...observerCodes.map((code) => [{ stderr: observerLine(code) }, `observer:${code}`]),
  [{ stderr: observerLine('disallowed') }, 'observer'],
  [{ stderr: observerLine('invalid_schema') + ' extra' }, 'observer'],
  [{ stderr: ' ' + observerLine('invalid_schema') }, 'observer'],
  [{ stderr: observerLine('invalid_schema') + '\u2028' }, 'observer'],
  [{ stderr: observerLine('invalid_schema') + '\r' }, 'observer'],
  [{ stderr: observerLine('invalid_schema') + '\n  ' }, 'observer:invalid_schema'],
  [{ stderr: `${observerLine('invalid_schema')}\n${canaries[5]}` }, 'observer'],
  [{ stderr: `${canaries[5]}\n${observerLine('invalid_schema')}\n\n` }, 'observer:invalid_schema'],
  [{ stderr: Buffer.from(observerLine('invalid_schema') + '\r\n') }, 'observer:invalid_schema'],
  [{ code: 'ETIMEDOUT', signal: 'SIGTERM' }, 'observer_timeout'],
  ...['SIGTERM', 'SIGINT', 'SIGKILL', 'SIGHUP'].map((signal) => [{ signal }, `observer_signal:${signal}`]),
  [{ signal: canaries[5] }, 'observer_signal'], [{ stderr: canaries[5], code: 1 }, 'observer'],
];
for (const [index, [fields, code]] of observerFailures.entries()) test(`observer error ${index} emits only ${code}`, (t) => {
  const h = harness(t);
  assert.throws(() => runObserver(h.env.CMA_ATTESTATION_VERIFIER, h.env.VERIFIER_SHA256, h.env.CMA_ATTESTATION_CONFIG, h.env.CONFIG_SHA256, h.before.request, () => {
    throw Object.assign(Error(canaries[5]), { stdout: canaries[4], ...fields });
  }), (error) => {
    const stderr = formatRefusal(error, 'attest', 'attestation');
    assert.equal(stderr, line('attest', 'observer', code)); noCanaries('', stderr); return true;
  });
});
test('observer timeout uses elapsed time only for SIGTERM and trusts a final enumerated code first', () => {
  assert.equal(classifyObserverError({ signal: 'SIGTERM' }, 240000), 'observer_timeout');
  assert.equal(classifyObserverError({ signal: 'SIGTERM' }, 239999), 'observer_signal:SIGTERM');
  assert.equal(classifyObserverError({ signal: 'SIGKILL' }, 240000), 'observer_signal:SIGKILL');
  assert.equal(classifyObserverError({ code: 'ETIMEDOUT', stderr: observerLine('observation_deadline') }), 'observer:observation_deadline');
});

test('unclassified raw errors and forged tokens cannot enter the public formatter', () => {
  for (const error of [Error(canaries[5]), new assert.AssertionError({ actual: canaries[3], expected: canaries[4] }), { code: canaries[5], phase: 'receipt' }, Object.assign(Error(canaries[5]), { phase: 'receipt' }), new AuthorityError(canaries[5])]) {
    const stderr = formatRefusal(error, canaries[0], undefined);
    assert.equal(stderr, line('invalid', 'unclassified', 'unclassified', '')); noCanaries('', stderr);
  }
  for (const phase of phases.filter((value) => value !== 'invocation')) {
    assert.throws(() => withPhase(phase, () => assert.fail(canaries[3])), (error) => {
      assert.equal(formatRefusal(error, 'attest', 'attestation'), line('attest', phase)); return true;
    });
  }
  assert.throws(() => withPhase('observer', () => check(false, 'observer_timeout')), { message: 'observer_timeout', phase: 'observer' });
  assert.ok(Object.isFrozen(phases) && Object.isFrozen(refusalCodes) && Object.isFrozen(observerCodes));
  const forged = Object.assign(new AuthorityError('github_api'), { phase: canaries[3] });
  assert.equal(formatRefusal(forged, 'plan', undefined), line('plan', 'unclassified', 'github_api', ''));
});

test('phase metadata belongs to each error across nested and successive exported calls', (t) => {
  const capture = (action) => {
    let failure;
    assert.throws(action, (error) => { assert.ok(error instanceof AuthorityError); failure = error; return true; });
    return failure;
  };
  const raw = capture(() => withPhase('receipt', () => { throw Error(canaries[3]); }));
  assert.equal(raw.phase, 'receipt');
  assert.equal(formatRefusal(raw, 'attest', 'attestation'), line('attest', 'receipt', 'receipt'));

  const classified = capture(() => withPhase('observer', () => check(false, 'github_api')));
  assert.equal(classified.phase, 'observer');
  assert.equal(formatRefusal(classified, 'attest', 'attestation'), line('attest', 'observer', 'github_api'));

  const nested = capture(() => withPhase('observer', () => withPhase('receipt', () => check(false, 'github_api'))));
  assert.equal(nested.phase, 'receipt');
  assert.equal(formatRefusal(nested, 'attest', 'attestation'), line('attest', 'receipt', 'github_api'));
  assert.equal(capture(() => withPhase('audit_output', () => { throw nested; })), nested);

  const h = harness(t);
  const campaign = capture(() => validateCampaign({ status: 'passed', cases: [] }, h.journal));
  const observe = (execute) => runObserver(h.env.CMA_ATTESTATION_VERIFIER, h.env.VERIFIER_SHA256, h.env.CMA_ATTESTATION_CONFIG, h.env.CONFIG_SHA256, h.before.request, execute);
  const receipt = capture(() => observe(() => canaries[4]));
  const observer = capture(() => observe(() => { throw { code: 'ETIMEDOUT' }; }));
  assert.deepEqual(observe(() => JSON.stringify(h.before.receipt)), h.before.receipt);
  validateCampaign(h.results, h.journal);
  assert.equal(formatRefusal(receipt, 'attest', 'attestation'), line('attest', 'receipt'));
  assert.equal(formatRefusal(campaign, 'audit', 'audit'), line('audit', 'campaign_results', 'campaign_results', 'audit'));
  assert.equal(formatRefusal(observer, 'attest', 'attestation'), line('attest', 'observer', 'observer_timeout'));

  const invalid = new AuthorityError('invalid_invocation');
  assert.equal(formatRefusal(invalid, 'invalid', undefined), line('invalid', 'invocation', 'invalid_invocation', ''));
  assert.equal(formatRefusal(new AuthorityError('github_api'), 'attest', 'attestation'), line('attest', 'unclassified', 'github_api'));
  assert.equal(formatRefusal(Error(canaries[4]), 'attest', 'attestation'), line('attest', 'unclassified', 'unclassified'));
});

test('observer allowlist equals literal second arguments to check plus the observer fallback', () => {
  const source = readFileSync(new URL('./deployment-observer.mjs', import.meta.url), 'utf8');
  const codes = new Set(['command_or_input_incomplete']);
  // Balance call delimiters while skipping quoted strings, templates and regular expressions.
  for (const match of source.matchAll(/\bcheck\s*\(/g)) {
    let depth = 0, start;
    for (let index = match.index + match[0].length; index < source.length; index++) {
      const char = source[index];
      if (['\'', '"', '`', '/'].includes(char)) {
        const quote = char;
        while (++index < source.length) {
          if (source[index] === '\\') index++;
          else if (source[index] === quote) break;
        }
      } else if ('([{'.includes(char)) depth++;
      else if (')]};'.includes(char)) {
        if (char === ')' && depth === 0) {
          const literal = source.slice(start, index).trim().match(/^(['"])([a-z_]{1,64})\1$/);
          if (start !== undefined && literal) codes.add(literal[2]);
          break;
        }
        depth--;
      } else if (char === ',' && depth === 0) start = index + 1;
    }
  }
  assert.deepEqual([...observerCodes].sort(), [...codes].sort());
});
