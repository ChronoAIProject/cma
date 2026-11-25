import assert from 'node:assert/strict';
import test from 'node:test';
import './runtime-image-evidence-compat.test.mjs';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, mkdirSync, writeFileSync, symlinkSync, chmodSync, lstatSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assertAgentShellPath, assertCheckpointHoldReceipt, assertPublicHomeSeed, checks as legacyChecks, diagnosticStages, installedCodexVersion, installRequest, localToolRunner, runtimeToolVersions, sha256, waitForWriterProgress } from './runtime-image-probe.mjs';
import { campaign, checks, combineProofs, evidenceSchema, pins, profilePins, safeFailureStage, source, validateEvidence, validateImage } from './runtime-image-evidence.mjs';
import { assignmentDigest, assertAssignmentReceipt, assignRuntime, assertRuntimeStatus, assertInstallReceipt, assertInstallReplay, bootstrapChecks, bootstrapStages, statusRequest, waitingEnvironment } from './runtime-bootstrap-probe.mjs';

const commit = 'c'.repeat(40);
const expected = pins(readFileSync(new URL('../../api/crates/layer4-runtime/Dockerfile', import.meta.url), 'utf8'));
const image = {
  Id: `sha256:${'1'.repeat(64)}`, Os: 'linux', Architecture: 'amd64',
  Config: { User: '10001:10001', Labels: {
    'org.opencontainers.image.revision': commit,
    'org.opencontainers.image.source': source,
    'dev.chrono.cma.runtime-contract': '3',
    'dev.chrono.cma.runtime-bootstrap': '1',
    'dev.chrono.cma.home-layout': '2',
  } },
};
const proof = { checks, artifacts: {
  runtime: '2'.repeat(64), launcher: expected.CODEX_LAUNCHER_SHA256,
  native: expected.CODEX_NATIVE_SHA256_AMD64, acp: expected.CODEX_ACP_SHA256,
  codexVersion: `codex-cli ${expected.CODEX_VERSION}`, acpVersion: expected.CODEX_ACP_VERSION,
  gh: '3'.repeat(64), ghVersion: expected.GH_VERSION,
  psVersion: '4.0.4', jqVersion: '1.7',
  codeModeHost: expected.CODEX_FORK_CODE_MODE_SHA256_AMD64,
  bwrap: expected.CODEX_FORK_BWRAP_SHA256_AMD64,
  rg: expected.CODEX_FORK_RG_SHA256_AMD64,
} };
const record = () => structuredClone({ schema: evidenceSchema, scope: 'local-contract-3-no-provider-or-model', commit, image, sourceReference: null, result: 'pass', cleanup: { containers: 0, volumes: 0 }, proof });

test('public seed requires materialized NyxID aliases and preserves their exact content', () => {
  const root = mkdtempSync(join(tmpdir(), 'cma-public-seed-test-'));
  const source = join(root, 'image-home');
  const seed = join(root, 'public-seed');
  const skill = join(source, '.codex', 'skills', 'nyxid');
  try {
    mkdirSync(join(skill, 'references'), { recursive: true });
    mkdirSync(join(skill, 'scripts'));
    writeFileSync(join(skill, 'references', 'post-install.md'), 'public install instructions');
    writeFileSync(join(skill, 'scripts', 'inspect.sh'), 'public executable script', { mode: 0o755 });
    symlinkSync('references/post-install.md', join(skill, 'POST_INSTALL.md'));
    symlinkSync('scripts', join(skill, 'tools'));
    assert.throws(() => assertPublicHomeSeed(source), /public home seed must contain only ordinary entries/);
    execFileSync('cp', ['-aL', source, seed], { stdio: ['ignore', 'pipe', 'pipe'] });
    assertPublicHomeSeed(seed);
    assert.ok(lstatSync(join(skill, 'POST_INSTALL.md')).isSymbolicLink());
    assert.ok(lstatSync(join(skill, 'tools')).isSymbolicLink());
    const copied = join(seed, '.codex', 'skills', 'nyxid');
    // GNU cp may preserve alias hardlinks; replace one entry to simulate drift.
    rmSync(join(copied, 'POST_INSTALL.md'));
    writeFileSync(join(copied, 'POST_INSTALL.md'), 'changed instructions');
    assert.throws(() => assertPublicHomeSeed(seed));
    writeFileSync(join(copied, 'POST_INSTALL.md'), readFileSync(join(skill, 'references', 'post-install.md')));
    rmSync(join(copied, 'tools', 'inspect.sh'));
    writeFileSync(join(copied, 'tools', 'inspect.sh'), readFileSync(join(skill, 'scripts', 'inspect.sh')), { mode: 0o644 });
    assert.throws(() => assertPublicHomeSeed(seed));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('writer progress waits for scheduling and refuses stopped or truncated counters', async () => {
  let value = '1';
  const started = performance.now();
  const writer = setTimeout(() => { value = '2'; }, 250);
  try {
    assert.equal(await waitForWriterProgress(() => value), '2');
    assert.ok(performance.now() - started >= 200);
  } finally { clearTimeout(writer); }
  for (const frozen of ['1', '', 'invalid']) {
    await assert.rejects(waitForWriterProgress(() => frozen, { timeoutMs: 30, intervalMs: 5 }),
      /background writer did not advance/);
  }
  const observations = ['1', '', '1', '2'];
  assert.equal(await waitForWriterProgress(() => observations.shift(), { intervalMs: 1 }), '2');
});

test('closed runtime evidence requires every real probe and exact cleanup', () => {
  validateEvidence(record(), commit, image.Id, expected);
  for (const mutate of [
    (v) => { v.result = 'skipped'; }, (v) => { v.proof = null; },
    (v) => { v.proof.checks.pop(); }, (v) => { v.proof.checks = []; },
    (v) => { v.proof.checks = legacyChecks; },
    (v) => { v.proof.artifacts.native = '0'.repeat(64); },
    (v) => { v.schema = 'cma-runtime-image-v2'; },
    (v) => { delete v.proof.artifacts.codeModeHost; },
    (v) => { v.proof.artifacts.bwrap = '0'.repeat(64); },
    (v) => { v.proof.artifacts.rg = '0'.repeat(64); },
    (v) => { v.proof.artifacts.runtime = ''; },
    (v) => { v.proof.artifacts.gh = ''; },
    (v) => { v.proof.artifacts.ghVersion = '0.0.0'; },
    (v) => { delete v.proof.artifacts.psVersion; }, (v) => { delete v.proof.artifacts.jqVersion; },
    (v) => { v.proof.artifacts.psVersion = ''; }, (v) => { v.proof.artifacts.jqVersion = 'gojq 0.12.16'; },
    (v) => { v.cleanup = null; }, (v) => { v.cleanup.containers = 1; },
    (v) => { v.image.Id = `sha256:${'9'.repeat(64)}`; },
    (v) => { v.image.Config.Labels['org.opencontainers.image.source'] = 'https://example.invalid'; },
    (v) => { v.image.Config.Labels['org.opencontainers.image.revision'] = 'd'.repeat(40); },
    (v) => { v.scope = 'live-provider'; }, (v) => { v.stdout = 'unreviewed diagnostics'; },
  ]) {
    const changed = record();
    mutate(changed);
    assert.throws(() => validateEvidence(changed, commit, image.Id, expected));
  }
});

// Outputs measured on the pinned Debian trixie base with procps 2:4.0.4-9 and jq 1.7.1-6+deb13u4.
const toolOutputs = (overrides = {}) => (command, args, input = '') => {
  const key = [command, ...args].join(' ');
  if (key in overrides) return overrides[key];
  const table = {
    'ps --version': { status: 0, stdout: 'ps from procps-ng 4.0.4\n' },
    'jq --version': { status: 0, stdout: 'jq-1.7\n' },
    'ps -o pid= -p 1': { status: 0, stdout: '      1\n' },
    'ps -o pid= -p 4194304': { status: 1, stdout: '' },
    'jq -ce .items | map(.n) | add': { status: 0, stdout: input === '{"items":[{"n":1},{"n":2}]}' ? '3\n' : '' },
    'jq -e .missing': { status: input === '{}' ? 1 : 0, stdout: 'null\n' },
  };
  assert.ok(key in table, `unexpected tool invocation: ${key}`);
  return table[key];
};

test('repository harness tools are required by name with procps ps and jq semantics', () => {
  assert.deepEqual(runtimeToolVersions(toolOutputs()), { psVersion: '4.0.4', jqVersion: '1.7' });
  // The real local runner reports a command absent from PATH; that must fail.
  const absent = `cma-absent-tool-${'0'.repeat(8)}`;
  assert.equal(localToolRunner(absent, ['--version']).status, null);
  for (const missing of ['ps', 'jq']) {
    const run = toolOutputs();
    assert.throws(() => runtimeToolVersions((command, args, input) =>
      command === missing ? localToolRunner(absent, args, input) : run(command, args, input)),
    new RegExp(`${missing} is not available on PATH`));
  }
  for (const [overrides, message] of [
    [{ 'ps --version': { status: 0, stdout: 'BusyBox v1.36.1 multi-call binary.\n' } }, /ps is not the required implementation/],
    [{ 'ps --version': { status: 1, stdout: '' } }, /ps is not available on PATH/],
    [{ 'jq --version': { status: 0, stdout: 'gojq 0.12.16 (rev: HEAD/go1.22.0)\n' } }, /jq is not the required implementation/],
    [{ 'ps -o pid= -p 1': { status: 1, stdout: '' } }, /ps did not report a live process/],
    ...[0, 2, null].map((status) => [
      { 'ps -o pid= -p 4194304': { status, stdout: '' } }, /ps did not return the absent-process status/,
    ]),
    [{ 'jq -ce .items | map(.n) | add': { status: 0, stdout: '[1,2]\n' } }, /jq did not filter JSON/],
    ...[0, 2, null].map((status) => [
      { 'jq -e .missing': { status, stdout: 'null\n' } }, /jq -e did not return the null-result status/,
    ]),
  ]) assert.throws(() => runtimeToolVersions(toolOutputs(overrides)), message);
});

test('published OCI root identity is bound to the opaque local execution image ID', () => {
  const reference = `ghcr.io/chronoaiproject/cma-runtime@sha256:${'9'.repeat(64)}`;
  const value = record();
  value.sourceReference = reference;
  value.image.RepoDigests = [reference];
  validateEvidence(value, commit, image.Id, expected, reference);
  value.image.RepoDigests = [image.Id];
  assert.throws(() => validateEvidence(value, commit, image.Id, expected, reference));
  value.image.RepoDigests = [reference];
  assert.throws(() => validateEvidence(value, commit, image.Id, expected, `${reference.slice(0, -1)}0`));
});

test('image admission refuses another architecture, user or runtime contract', () => {
  for (const mutate of [
    (v) => { v.Architecture = 'arm64'; }, (v) => { v.Config.User = 'root'; },
    (v) => { v.Config.Labels['dev.chrono.cma.runtime-contract'] = '2'; },
    (v) => { v.Config.Labels['dev.chrono.cma.runtime-bootstrap'] = '0'; },
    (v) => { delete v.Config.Labels['dev.chrono.cma.runtime-bootstrap']; },
    (v) => { v.Config.Labels['dev.chrono.cma.home-layout'] = '1'; },
    (v) => { delete v.Config.Labels['dev.chrono.cma.home-layout']; },
  ]) {
    const changed = structuredClone(image);
    mutate(changed);
    assert.throws(() => validateImage(changed, commit));
  }
});

test('install digest is deterministic and binds the exact command bytes', () => {
  assert.deepEqual(installRequest(['true']), installRequest(['true']));
  assert.notEqual(installRequest(['true']).request_sha256, installRequest(['true ']).request_sha256);
  assert.notEqual(installRequest(['a', 'bc']).request_sha256, installRequest(['ab', 'c']).request_sha256);
});

test('probe failure diagnostics emit only known static stages', () => {
  for (const stage of diagnosticStages) {
    assert.equal(safeFailureStage('probe', { stderr: Buffer.from(`runtime image probe failed at ${stage}; raw runtime data withheld\n`) }), `probe:${stage}`);
  }
  for (const stderr of [
    'raw credential data',
    'runtime image probe failed at unknown-sensitive-value; raw runtime data withheld',
    'runtime image probe failed at checkpoint-request; raw runtime data withheld SECRET',
    'prefix runtime image probe failed at checkpoint-request; raw runtime data withheld',
    'runtime image probe failed at checkpoint-request\nSECRET; raw runtime data withheld',
  ]) assert.equal(safeFailureStage('probe', { stderr }), 'probe');
  assert.equal(safeFailureStage('probe', Error('raw child error')), 'probe');
  assert.equal(safeFailureStage('image-admission', { stderr: 'raw image data' }), 'image-admission');
  for (const stage of bootstrapStages) {
    assert.equal(safeFailureStage('bootstrap-probe', { stderr: `runtime bootstrap probe failed at ${stage}; raw runtime data withheld\n` }), `bootstrap-probe:${stage}`);
  }
  for (const stderr of [
    'raw credential data',
    'runtime bootstrap probe failed at secret-value; raw runtime data withheld',
    'runtime bootstrap probe failed at bootstrap-retained-installation; raw runtime data withheld SECRET',
    'prefix runtime bootstrap probe failed at bootstrap-install-once; raw runtime data withheld',
  ]) assert.equal(safeFailureStage('bootstrap-probe', { stderr }), 'bootstrap-probe');
});

const assignment = () => ({ version: 1, pod_uid: 'original-pod', environment: {
  CHRONO_SANDBOX_ID: 'sbx_original', CHRONO_RUNTIME_GENERATION: '1',
  CHRONO_RUNTIME_HOME_IDENTITY: 'a'.repeat(43), CHRONO_RUNTIME_CONTROL_TOKEN: 'b'.repeat(64),
  CHRONO_RUNTIME_HOME: '/volume/home', CHRONO_RUNTIME_STATE_ROOT: '/volume/state', TMPDIR: '/volume/transfer',
} });
const assignmentReceipt = (request) => ({ version: 1, pod_uid: request.pod_uid, request_sha256: assignmentDigest(request) });

test('bootstrap receipt binds canonical assignment, Pod identity and every environment value', () => {
  const request = assignment();
  const receipt = assignmentReceipt(request);
  // Fixed vector for the Rust serde field order, sorted environment and domain prefix.
  assert.equal(receipt.request_sha256, '16252bf0b874804890d33e014e105b62e7291ec458c0a19339d8a667325a26c1');
  assertAssignmentReceipt(receipt, request);
  assert.equal(assignmentDigest({ ...request, environment: Object.fromEntries(Object.entries(request.environment).reverse()) }), receipt.request_sha256);
  for (const mutate of [
    (v) => { v.pod_uid = 'replacement-pod'; },
    ...Object.keys(request.environment).map((name) => (v) => { v.environment[name] += '-changed'; }),
  ]) {
    const changed = structuredClone(request);
    mutate(changed);
    assert.notEqual(assignmentDigest(changed), receipt.request_sha256);
    assert.throws(() => assertAssignmentReceipt(receipt, changed));
  }
  for (const changed of [
    { ...receipt, version: 2 }, { ...receipt, pod_uid: 'other' },
    { ...receipt, request_sha256: '0'.repeat(64) }, { ...receipt, untrusted: true },
    { pod_uid: receipt.pod_uid, request_sha256: receipt.request_sha256 },
  ]) assert.throws(() => assertAssignmentReceipt(changed, request));
});

test('assignment is sent only through stdin and waiting process environment excludes tenant credentials', () => {
  const request = assignment();
  const environment = waitingEnvironment(request.pod_uid, '/runtime/assignment.sock', '/tmp/fresh-boot-one');
  assert.deepEqual(Object.keys(environment).sort(), ['CHRONO_RUNTIME_BOOTSTRAP_SOCKET', 'CHRONO_RUNTIME_POD_UID', 'LANG', 'PATH', 'TMPDIR']);
  const second = waitingEnvironment('replacement-pod', environment.CHRONO_RUNTIME_BOOTSTRAP_SOCKET, '/tmp/fresh-boot-two');
  assert.notEqual(second.TMPDIR, environment.TMPDIR);
  const success = { status: 0, signal: null, stdout: JSON.stringify(assignmentReceipt(request)), stderr: '' };
  const command = (program, args, options) => {
    assert.equal(program, '/usr/local/bin/layer4-runtime');
    assert.deepEqual(args, ['--assign-runtime']);
    assert.deepEqual(options.env, environment);
    assert.deepEqual(JSON.parse(options.input), request);
    assert.deepEqual(options.stdio, ['pipe', 'pipe', 'pipe']);
    assert.ok(!JSON.stringify([args, options.env]).includes(request.environment.CHRONO_RUNTIME_CONTROL_TOKEN));
    return success;
  };
  assertAssignmentReceipt(assignRuntime(request, environment, true, command), request);
  assignRuntime(request, environment, false, () => ({ status: 2, signal: null, stdout: '', stderr: 'runtime assignment was rejected' }));
  for (const result of [
    { ...success, error: Error('timeout') }, { ...success, signal: 'SIGKILL' },
    { ...success, status: null }, { ...success, stdout: '{}' },
    { ...success, stderr: request.environment.CHRONO_RUNTIME_CONTROL_TOKEN },
    { ...success, stderr: 'CHRONO_RUNTIME_CONTROL_TOKEN' },
  ]) assert.throws(() => assignRuntime(request, environment, true, () => result));
  for (const result of [
    success, { status: 2, stdout: '', stderr: 'runtime assignment socket is unavailable' },
    { status: 2, stdout: 'untrusted', stderr: 'runtime assignment was rejected' },
  ]) assert.throws(() => assignRuntime(request, environment, false, () => result));
});

test('restart status requires installed state, changed boot and exact clean runtime identity', () => {
  const request = assignment();
  request.environment.CHRONO_RUNTIME_GENERATION = '2';
  const status = { ...statusRequest(request), boot_id: '2'.repeat(32), installation_ready: true,
    credential_ready: false, restore_pending: false, agent_processes: 0, agent_skill_install: true, agent_model_catalog: true };
  assert.equal(assertRuntimeStatus(status, request, true, '1'.repeat(32)), status.boot_id);
  for (const mutate of [
    (v) => { v.installation_ready = false; }, (v) => { v.boot_id = '1'.repeat(32); },
    (v) => { v.boot_id = 'invalid'; }, (v) => { v.sandbox_id = 'other'; },
    (v) => { v.generation = 1; }, (v) => { v.home_identity = 'b'.repeat(43); },
    (v) => { v.credential_ready = true; }, (v) => { v.restore_pending = true; },
    (v) => { v.agent_processes = 1; }, (v) => { v.extra = true; },
    (v) => { v.agent_skill_install = false; }, (v) => { delete v.agent_skill_install; },
    (v) => { v.agent_model_catalog = false; }, (v) => { delete v.agent_model_catalog; },
  ]) {
    const changed = structuredClone(status);
    mutate(changed);
    assert.throws(() => assertRuntimeStatus(changed, request, true, '1'.repeat(32)));
  }
  assert.throws(() => assertRuntimeStatus(status, request, false));
});

test('installation proof refuses failed commands, receipt drift and rerun marker', () => {
  const request = installRequest(['true']);
  const receipt = { t: 'install', operation_id: request.operation_id, request_sha256: request.request_sha256,
    commands: [{ index: 0, exit_code: 0, stdout_sha256: sha256(''), stderr_sha256: sha256('') }] };
  assertInstallReceipt(receipt, request);
  assertInstallReplay(structuredClone(receipt), receipt, 'installed\n');
  for (const mutate of [
    (v) => { v.t = 'failure'; }, (v) => { v.operation_id = 'other'; },
    (v) => { v.request_sha256 = '0'.repeat(64); }, (v) => { v.commands = []; },
    (v) => { v.commands[0].index = 1; }, (v) => { v.commands[0].exit_code = 1; },
    (v) => { v.commands[0].stdout_sha256 = 'raw output'; }, (v) => { v.extra = true; },
  ]) {
    const changed = structuredClone(receipt);
    mutate(changed);
    assert.throws(() => assertInstallReceipt(changed, request));
    assert.throws(() => assertInstallReplay(changed, receipt, 'installed\n'));
  }
  for (const marker of ['', 'installed', 'installed\ninstalled\n']) {
    assert.throws(() => assertInstallReplay(receipt, receipt, marker));
  }
});

test('combined image evidence requires bootstrap and restart proof from the same executable', () => {
  const legacy = { ...proof, checks: legacyChecks };
  const bootstrap = { checks: bootstrapChecks, runtime: proof.artifacts.runtime };
  assert.deepEqual(combineProofs(legacy, bootstrap, expected), proof);
  for (const check of bootstrapChecks) {
    assert.throws(() => combineProofs(legacy, { ...bootstrap, checks: bootstrapChecks.filter((value) => value !== check) }, expected));
  }
  assert.throws(() => combineProofs(legacy, { ...bootstrap, runtime: '9'.repeat(64) }, expected));
  assert.throws(() => combineProofs(legacy, { ...bootstrap, raw: 'unreviewed data' }, expected));
  assert.throws(() => combineProofs(proof, bootstrap, expected));
});

test('Codex version inspection isolates helper symlinks and preserves the sandbox home', () => {
  const home = mkdtempSync(join(tmpdir(), 'cma-runtime-version-test-'));
  const scratchPaths = [];
  try {
    mkdirSync(join(home, '.codex'));
    writeFileSync(join(home, '.codex', 'retained'), 'retained');
    symlinkSync('retained', join(home, '.codex', 'retained-link'));
    const command = (program, args, options) => {
      assert.equal(program, 'codex');
      assert.deepEqual(args, ['--version']);
      assert.deepEqual(options.stdio, ['ignore', 'pipe', 'pipe']);
      assert.equal(options.env.HOME, process.env.HOME);
      const scratch = options.env.CODEX_HOME;
      assert.notEqual(scratch, process.env.CODEX_HOME);
      assert.ok(!scratch.startsWith(`${home}/`));
      scratchPaths.push(scratch);
      mkdirSync(join(scratch, 'tmp'));
      symlinkSync('/usr/local/bin/codex', join(scratch, 'tmp', 'apply_patch'));
      return `codex-cli ${expected.CODEX_VERSION}\n`;
    };
    assert.equal(installedCodexVersion(home, command), `codex-cli ${expected.CODEX_VERSION}`);
    assert.equal(installedCodexVersion(home, command), `codex-cli ${expected.CODEX_VERSION}`);
    assert.notEqual(scratchPaths[0], scratchPaths[1]);
    assert.ok(scratchPaths.every((path) => !existsSync(path)));
    for (const mutate of [
      () => writeFileSync(join(home, '.codex', 'retained'), 'changed'),
      () => chmodSync(join(home, '.codex', 'retained'), 0o400),
      () => symlinkSync('/usr/local/bin/codex', join(home, '.codex', 'escaped')),
    ]) {
      assert.throws(() => installedCodexVersion(home, (...args) => { const version = command(...args); mutate(); return version; }));
      assert.ok(scratchPaths.every((path) => !existsSync(path)));
    }
    assert.throws(() => installedCodexVersion(home, (...args) => { command(...args); throw Error('raw child failure'); }));
    assert.ok(scratchPaths.every((path) => !existsSync(path)));
  } finally { rmSync(home, { recursive: true, force: true }); }
});

test('Agent shell probe requires exact inherited paths, both shell modes, and empty/unset fallbacks', () => {
  const agentHome = '/home/cma/.cma/agents/ci-agent/home';
  const toolDirectory = '/home/cma/arbitrary tools/bin with spaces';
  const calls = [];
  const command = (program, args, options) => {
    assert.equal(program, '/bin/bash');
    assert.equal(options.env.HOME, agentHome);
    assert.equal(options.env.CODEX_HOME, agentHome);
    assert.deepEqual(options.stdio, ['ignore', 'pipe', 'pipe']);
    calls.push(args);
    if (args[0] === '--noprofile') return '/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games';
    assert.ok(options.env.PATH.includes(toolDirectory));
    return JSON.stringify({ home: agentHome, path: options.env.PATH, tool: 'cma-shell-path-ok\n' });
  };
  assertAgentShellPath(agentHome, toolDirectory, command);
  assert.deepEqual(calls.slice(0, 4).map((args) => args[0]), ['-c', '-lc', '-c', '-lc']);
  assert.equal(calls.length, 6);
  assert.ok(calls[4][3].startsWith('PATH=""\n'));
  assert.ok(calls[5][3].startsWith('unset PATH\n'));
  assert.throws(() => assertAgentShellPath(agentHome, toolDirectory, (program, args, options) => {
    const output = command(program, args, options);
    if (args[0] !== '-lc') return output;
    return JSON.stringify({ ...JSON.parse(output), path: '/usr/bin:/bin' });
  }));
  assert.throws(() => assertAgentShellPath(agentHome, toolDirectory, (program, args, options) => {
    const output = command(program, args, options);
    return args[0] === '--noprofile' ? '' : output;
  }));
});

test('checkpoint diagnostics distinguish refusal, receipt and writer stages without returning runtime values', () => {
  const hold = { t: 'checkpoint_for_teardown', writer_resume_token: 'a'.repeat(64) };
  const receipt = { ...hold, checkpoint: { writers: 'held' } };
  const stages = [];
  assertCheckpointHoldReceipt(receipt, hold, (stage) => stages.push(stage));
  assert.deepEqual(stages, ['checkpoint-receipt-type', 'checkpoint-receipt-writers', 'checkpoint-receipt-token']);
  for (const [value, expectedStage] of [
    [{ t: 'failure', code: 'writer_fence_failed', detail: 'SECRET' }, 'checkpoint-refused-writer-fence-failed'],
    [{ t: 'failure', code: 'checkpoint_io_failed', detail: 'SECRET' }, 'checkpoint-refused-checkpoint-io-failed'],
    [{ t: 'failure', code: 'checkpoint_symlink_escape', detail: 'checkpoint symlink .codex/tmp/arg0/sensitive-path/apply_patch escapes the sandbox home' }, 'checkpoint-refused-codex-temporary-symlink-escape'],
    [{ t: 'failure', code: 'SECRET', detail: 'SECRET' }, 'checkpoint-receipt-type'],
    [{ ...receipt, checkpoint: { writers: 'resumed' } }, 'checkpoint-receipt-writers'],
    [{ ...receipt, writer_resume_token: 'SECRET' }, 'checkpoint-receipt-token'],
  ]) {
    let actualStage;
    assert.throws(() => assertCheckpointHoldReceipt(value, hold, (stage) => { actualStage = stage; }));
    assert.equal(actualStage, expectedStage);
    assert.ok(diagnosticStages.includes(actualStage));
  }
});

test('campaign pins the execution ID and refuses either probe, incomplete restart, deletion and inventory failures', () => {
  for (const failure of ['', 'probe', 'bootstrap', 'missing-restart', 'runtime-mismatch', 'cleanup', 'inventory']) {
    const directory = mkdtempSync(join(tmpdir(), 'cma-runtime-evidence-test-'));
    let container = false;
    let volume = false;
    let name;
    const calls = [];
    const command = (args) => {
      calls.push(args);
      if (args[0] === 'image') return JSON.stringify([image]);
      if (args[0] === 'volume' && args[1] === 'create') { name = args.at(-1); volume = true; return name; }
      if (args[0] === 'create') {
        assert.equal(args.at(-1), image.Id);
        assert.ok(args.some((value) => value.endsWith('target=/opt/runtime-bootstrap-probe.mjs,readonly')));
        container = true;
        return 'a'.repeat(64);
      }
      if (args[0] === 'start') return name;
      if (args[0] === 'exec') {
        assert.equal(args[1], name);
        assert.equal(args[2], 'node');
        if (args[3] === '/opt/runtime-image-probe.mjs') {
          if (failure === 'probe') throw Error('probe failed');
          return JSON.stringify({ ...proof, checks: legacyChecks });
        }
        assert.equal(args[3], '/opt/runtime-bootstrap-probe.mjs');
        if (failure === 'bootstrap') throw Error('bootstrap failed');
        return JSON.stringify({ checks: failure === 'missing-restart' ? bootstrapChecks.slice(0, -1) : bootstrapChecks,
          runtime: failure === 'runtime-mismatch' ? '9'.repeat(64) : proof.artifacts.runtime });
      }
      if (args[0] === 'ps') { if (failure === 'inventory') throw Error('inventory unavailable'); return container ? 'a'.repeat(12) : ''; }
      if (args[0] === 'rm') { if (failure === 'cleanup') throw Error('delete ambiguous'); container = false; return ''; }
      if (args[0] === 'volume' && args[1] === 'ls') return volume ? name : '';
      if (args[0] === 'volume' && args[1] === 'rm') { volume = false; return ''; }
      throw Error('unexpected Docker operation');
    };
    const output = join(directory, 'evidence.json');
    try {
      if (failure) assert.throws(() => campaign('runtime:test', commit, output, command));
      else campaign('runtime:test', commit, output, command);
      const evidence = JSON.parse(readFileSync(output, 'utf8'));
      assert.equal(evidence.schema, evidenceSchema);
      assert.equal(evidence.result, failure ? 'fail' : 'pass');
      assert.ok(calls.find((args) => args[0] === 'ps'));
      if (['probe', 'bootstrap', 'missing-restart', 'runtime-mismatch'].includes(failure)) assert.deepEqual(evidence.cleanup, { containers: 0, volumes: 0 });
      if (failure === 'cleanup' || failure === 'inventory') assert.equal(evidence.cleanup, null);
      if (!failure) assert.equal(calls.filter((args) => args[0] === 'exec').length, 2);
    } finally { rmSync(directory, { recursive: true }); }
  }
});

test('Omarchy profile uses the same native producer and its distinct image probes', () => {
  const dockerfile = readFileSync(new URL('../../api/crates/layer4-runtime/Dockerfile', import.meta.url), 'utf8');
  const desktop = profilePins(dockerfile, 'omarchy');
  assert.deepEqual(profilePins(dockerfile), expected);
  for (const key of ['CODEX_ACP_VERSION', 'CODEX_ACP_SHA256', 'GH_VERSION']) assert.equal(desktop[key], expected[key]);
  for (const key of ['CODEX_VERSION', 'CODEX_NATIVE_SHA256_AMD64', 'CODEX_LAUNCHER_SHA256']) {
    assert.equal(desktop[key], dockerfile.match(new RegExp(`^ARG DESKTOP_${key}=(\\S+)$`, 'm'))[1]);
    assert.equal(desktop[key], expected[key]);
  }
  assert.throws(() => profilePins(dockerfile, 'desktop'));
  const desktopProof = { checks: legacyChecks, artifacts: { ...proof.artifacts, launcher: desktop.CODEX_LAUNCHER_SHA256,
    native: desktop.CODEX_NATIVE_SHA256_AMD64, codexVersion: `codex-cli ${desktop.CODEX_VERSION}` } };
  for (const profile of ['default', 'omarchy', 'omarchy-wrong-pins', 'omarchy-gpgdir']) {
    const omarchy = profile.startsWith('omarchy');
    const directory = mkdtempSync(join(tmpdir(), 'cma-runtime-evidence-profile-'));
    const calls = [];
    let name, created, prepared;
    const command = (args) => {
      calls.push(args);
      if (args[0] === 'image') return JSON.stringify([image]);
      if (args[0] === 'volume' && args[1] === 'create') { name = args.at(-1); return name; }
      if (args[0] === 'create') {
        created = args;
        const probe = args.find((value) => value.endsWith('target=/opt/runtime-image-probe.mjs,readonly'));
        prepared = probe.slice('type=bind,source='.length, probe.indexOf(',target='));
        assert.ok(readFileSync(prepared, 'utf8').includes("from './omarchy-codex-aliases.mjs'"));
        assert.ok(readFileSync(prepared, 'utf8').includes("exclusions: ['.cma/agents/ci-agent/home/tmp/arg0', '.codex/tmp/arg0']"));
        return 'a'.repeat(64);
      }
      if (args[0] === 'start') return name;
      if (args[0] === 'exec' && args.includes('/opt/runtime-image-probe.mjs')) {
        return JSON.stringify(profile === 'omarchy-wrong-pins'
          ? { ...desktopProof, artifacts: { ...desktopProof.artifacts, native: '0'.repeat(64) } }
          : omarchy ? desktopProof : { ...proof, checks: legacyChecks });
      }
      if (args[0] === 'exec' && args.includes('/opt/runtime-bootstrap-probe.mjs')) return JSON.stringify({ checks: bootstrapChecks, runtime: proof.artifacts.runtime });
      if (args[0] === 'exec' && args.includes('/usr/local/bin/pacman-conf')) {
        assert.deepEqual(args.slice(1, 4), ['--user', '0', name]);
        return profile === 'omarchy-gpgdir' ? '/home/cma/.gnupg\n' : '/etc/pacman.d/gnupg\n';
      }
      if (args[0] === 'exec') { assert.ok(omarchy); return ''; }
      if (args[0] === 'ps' || (args[0] === 'volume' && args[1] === 'ls')) return '';
      throw Error('unexpected Docker operation');
    };
    const output = join(directory, 'evidence.json');
    try {
      const run = () => campaign('runtime:test', commit, output, command, profile.split('-')[0] === 'omarchy' ? 'omarchy' : 'default');
      if (profile === 'default' || profile === 'omarchy') run(); else assert.throws(run);
      const evidence = JSON.parse(readFileSync(output, 'utf8'));
      assert.equal(evidence.result, ['default', 'omarchy'].includes(profile) ? 'pass' : 'fail');
      assert.equal(evidence.schema, evidenceSchema);
      assert.ok(created.some((value) => value.endsWith('target=/opt/omarchy-codex-aliases.mjs,readonly')));
      const extras = ['omarchy-keyring-probe.py', 'omarchy-chromium-config-probe.py'];
      for (const file of extras) assert.equal(created.some((value) => value.endsWith(`target=/opt/${file},readonly`)), omarchy);
      const execs = calls.filter((args) => args[0] === 'exec').map((args) => args.at(-1));
      assert.deepEqual(execs, profile === 'default'
        ? ['/opt/runtime-image-probe.mjs', '/opt/runtime-bootstrap-probe.mjs']
        : profile === 'omarchy-gpgdir'
          ? ['/opt/runtime-image-probe.mjs', '/opt/omarchy-keyring-probe.py', 'GPGDir']
          : profile === 'omarchy-wrong-pins'
            ? ['/opt/runtime-image-probe.mjs', '/opt/omarchy-keyring-probe.py', 'GPGDir']
            : ['/opt/runtime-image-probe.mjs', '/opt/omarchy-keyring-probe.py', 'GPGDir', '/opt/runtime-bootstrap-probe.mjs', '/opt/omarchy-chromium-config-probe.py']);
      assert.equal(existsSync(prepared), false);
    } finally { rmSync(directory, { recursive: true }); }
  }
  assert.throws(() => campaign('runtime:test', commit, join(tmpdir(), 'unused.json'), () => { throw Error('no Docker'); }, 'desktop'));
});
