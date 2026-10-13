import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, mkdtempSync, readFileSync, rmSync, mkdirSync, writeFileSync, symlinkSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assertAgentShellPath, assertCheckpointHoldReceipt, checks, diagnosticStages, installedCodexVersion, installRequest, waitForWriterProgress } from './runtime-image-probe.mjs';
import { campaign, pins, safeFailureStage, source, validateEvidence, validateImage } from './runtime-image-evidence.mjs';

const commit = 'c'.repeat(40);
const expected = pins(readFileSync(new URL('../../infra/runtime/Dockerfile', import.meta.url), 'utf8'));
const image = {
  Id: `sha256:${'1'.repeat(64)}`, Os: 'linux', Architecture: 'amd64',
  Config: { User: '10001:10001', Labels: {
    'org.opencontainers.image.revision': commit,
    'org.opencontainers.image.source': source,
    'dev.chrono.cma.runtime-contract': '3',
    'dev.chrono.cma.runtime-bootstrap': '1',
  } },
};
const proof = { checks, artifacts: {
  runtime: '2'.repeat(64), launcher: expected.CODEX_LAUNCHER_SHA256,
  native: expected.CODEX_NATIVE_SHA256_AMD64, acp: expected.CODEX_ACP_SHA256,
  codexVersion: `codex-cli ${expected.CODEX_VERSION}`, acpVersion: expected.CODEX_ACP_VERSION,
  gh: '3'.repeat(64), ghVersion: expected.GH_VERSION,
} };
const record = () => structuredClone({ schema: 'cma-runtime-image-v1', scope: 'local-contract-3-no-provider-or-model', commit, image, sourceReference: null, result: 'pass', cleanup: { containers: 0, volumes: 0 }, proof });

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
    (v) => { v.proof.artifacts.native = '0'.repeat(64); },
    (v) => { v.proof.artifacts.runtime = ''; },
    (v) => { v.proof.artifacts.gh = ''; },
    (v) => { v.proof.artifacts.ghVersion = '0.0.0'; },
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

test('campaign pins the execution ID and refuses probe, deletion and inventory failures', () => {
  for (const failure of ['', 'probe', 'cleanup', 'inventory']) {
    const directory = mkdtempSync(join(tmpdir(), 'cma-runtime-evidence-test-'));
    let container = false;
    let volume = false;
    let name;
    const calls = [];
    const command = (args) => {
      calls.push(args);
      if (args[0] === 'image') return JSON.stringify([image]);
      if (args[0] === 'volume' && args[1] === 'create') { name = args.at(-1); volume = true; return name; }
      if (args[0] === 'create') { assert.equal(args.at(-1), image.Id); container = true; return 'a'.repeat(64); }
      if (args[0] === 'start') return name;
      if (args[0] === 'exec') { if (failure === 'probe') throw Error('probe failed'); return JSON.stringify(proof); }
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
      assert.equal(evidence.result, failure ? 'fail' : 'pass');
      assert.ok(calls.find((args) => args[0] === 'ps'));
      if (failure === 'probe') assert.deepEqual(evidence.cleanup, { containers: 0, volumes: 0 });
      if (failure === 'cleanup' || failure === 'inventory') assert.equal(evidence.cleanup, null);
    } finally { rmSync(directory, { recursive: true }); }
  }
});
