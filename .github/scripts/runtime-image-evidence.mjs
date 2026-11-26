import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { readFileSync, writeFileSync, lstatSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { checks as legacyChecks, diagnosticStages, toolVersionPattern } from './runtime-image-probe.mjs';
import { bootstrapChecks, bootstrapStages, validateBootstrapProof } from './runtime-bootstrap-probe.mjs';
import { prepareProbe } from './omarchy-contract-source.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const source = 'https://github.com/ChronoAIProject/cma';
export const checks = [...legacyChecks, ...bootstrapChecks];
export const evidenceSchema = 'cma-runtime-image-v3';
const companionPins = { codeModeHost: 'CODEX_FORK_CODE_MODE_SHA256_AMD64', bwrap: 'CODEX_FORK_BWRAP_SHA256_AMD64', rg: 'CODEX_FORK_RG_SHA256_AMD64' };
const digest = /^sha256:[0-9a-f]{64}$/;
const revision = /^[0-9a-f]{40}$/;
const fields = (value, keys) => assert.deepEqual(Object.keys(value).sort(), [...keys].sort());

export function pins(dockerfile) {
  const result = {};
  const names = ['CODEX_VERSION', 'CODEX_ACP_VERSION', 'CODEX_NATIVE_SHA256_AMD64', 'CODEX_LAUNCHER_SHA256', 'CODEX_ACP_SHA256', 'GH_VERSION'];
  if (/^ARG CODEX_FORK_RELEASE=/m.test(dockerfile)) names.push(...Object.values(companionPins));
  for (const name of names) {
    const matches = [...dockerfile.matchAll(new RegExp(`^ARG ${name}=([^\\s]+)$`, 'gm'))];
    assert.equal(matches.length, 1);
    result[name] = matches[0][1];
  }
  return result;
}

// The Omarchy desktop image ships the desktop Codex build. Its contract uses the
// same probes, with only these three Dockerfile pins taken from DESKTOP_* arguments.
export const profiles = ['default', 'omarchy'];
export function profilePins(dockerfile, profile = 'default') {
  assert.ok(profiles.includes(profile));
  const result = pins(dockerfile);
  if (profile === 'omarchy') {
    for (const name of ['CODEX_VERSION', 'CODEX_NATIVE_SHA256_AMD64', 'CODEX_LAUNCHER_SHA256']) {
      const matches = [...dockerfile.matchAll(new RegExp(`^ARG DESKTOP_${name}=([^\\s]+)$`, 'gm'))];
      assert.equal(matches.length, 1);
      result[name] = matches[0][1];
    }
  }
  return result;
}

export function validateImage(image, commit, expectedId) {
  assert.match(commit, revision);
  assert.match(image.Id, digest);
  if (expectedId) assert.equal(image.Id, expectedId);
  assert.equal(image.Os, 'linux');
  assert.equal(image.Architecture, 'amd64');
  assert.equal(image.Config.User, '10001:10001');
  assert.equal(image.Config.Labels['org.opencontainers.image.revision'], commit);
  assert.equal(image.Config.Labels['org.opencontainers.image.source'], source);
  assert.equal(image.Config.Labels['dev.chrono.cma.runtime-contract'], '3');
  assert.equal(image.Config.Labels['dev.chrono.cma.runtime-bootstrap'], '1');
  assert.equal(image.Config.Labels['dev.chrono.cma.home-layout'], '2');
}

function hasCompanionPins(expected) {
  return Object.values(companionPins).every(pin => Object.hasOwn(expected, pin));
}

function validateProbeChecks(proof, expected, requiredChecks, requireToolVersions = true) {
  fields(proof, ['checks', 'artifacts']);
  assert.deepEqual(proof.checks, requiredChecks);
  fields(proof.artifacts, ['runtime', 'launcher', 'native', 'acp', 'codexVersion', 'acpVersion', 'gh', 'ghVersion',
    ...(requireToolVersions ? ['psVersion', 'jqVersion'] : []),
    ...(hasCompanionPins(expected) ? Object.keys(companionPins) : [])]);
  assert.match(proof.artifacts.runtime, /^[0-9a-f]{64}$/);
  assert.equal(proof.artifacts.launcher, expected.CODEX_LAUNCHER_SHA256);
  assert.equal(proof.artifacts.native, expected.CODEX_NATIVE_SHA256_AMD64);
  if (hasCompanionPins(expected)) {
    for (const [name, pin] of Object.entries(companionPins)) {
      assert.match(expected[pin], /^[a-f0-9]{64}$/);
      assert.equal(proof.artifacts[name], expected[pin]);
    }
  }
  assert.equal(proof.artifacts.acp, expected.CODEX_ACP_SHA256);
  assert.equal(proof.artifacts.codexVersion, `codex-cli ${expected.CODEX_VERSION}`);
  assert.equal(proof.artifacts.acpVersion, expected.CODEX_ACP_VERSION);
  assert.match(proof.artifacts.gh, /^[0-9a-f]{64}$/);
  assert.equal(proof.artifacts.ghVersion, expected.GH_VERSION);
  if (requireToolVersions) {
    assert.match(proof.artifacts.psVersion, toolVersionPattern);
    assert.match(proof.artifacts.jqVersion, toolVersionPattern);
  }
}

export function validateProbe(proof, expected) { validateProbeChecks(proof, expected, checks); }

export function combineProofs(legacy, bootstrap, expected) {
  validateProbeChecks(legacy, expected, legacyChecks);
  validateBootstrapProof(bootstrap, legacy.artifacts.runtime);
  const proof = { checks: [...legacy.checks, ...bootstrap.checks], artifacts: legacy.artifacts };
  validateProbe(proof, expected);
  return proof;
}

export function validateEvidence(value, commit, expectedId, expected, sourceReference = null) {
  fields(value, ['schema', 'scope', 'commit', 'image', 'sourceReference', 'result', 'cleanup', 'proof']);
  assert.equal(value.scope, 'local-contract-3-no-provider-or-model');
  assert.equal(value.commit, commit);
  assert.equal(value.sourceReference, sourceReference);
  if (sourceReference !== null) {
    assert.match(sourceReference, /^ghcr\.io\/chronoaiproject\/cma-runtime@sha256:[0-9a-f]{64}$/);
    assert.ok(value.image.RepoDigests.includes(sourceReference));
  }
  assert.equal(value.result, 'pass');
  assert.deepEqual(value.cleanup, { containers: 0, volumes: 0 });
  validateImage(value.image, commit, expectedId);
  // Verify retained evidence using its original profile. New campaigns always
  // emit v2 and still require both tools; unknown profiles never fall back.
  switch (value.schema) {
    case 'cma-runtime-image-v1':
      assert.equal(hasCompanionPins(expected), false);
      validateProbeChecks(value.proof, expected, checks, false);
      break;
    case 'cma-runtime-image-v2':
      assert.equal(hasCompanionPins(expected), false);
      validateProbe(value.proof, expected);
      break;
    case evidenceSchema:
      assert.equal(hasCompanionPins(expected), true, "unsupported runtime image evidence schema for this source");
      validateProbe(value.proof, expected);
      break;
    default:
      assert.fail('unsupported runtime image evidence schema');
  }
}

const docker = (args) => execFileSync('docker', args, { encoding: 'utf8', timeout: 180000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const inspect = (id) => JSON.parse(docker(['image', 'inspect', id]))[0];

export function safeFailureStage(stage, error) {
  if (stage === 'bootstrap-probe') {
    const match = String(error?.stderr ?? '').match(/^runtime bootstrap probe failed at ([a-z-]+); raw runtime data withheld$/m);
    return match && bootstrapStages.includes(match[1]) ? `bootstrap-probe:${match[1]}` : 'bootstrap-probe';
  }
  if (stage !== 'probe') return stage;
  const match = String(error?.stderr ?? '').match(/^runtime image probe failed at ([a-z-]+); raw runtime data withheld$/m);
  return match && diagnosticStages.includes(match[1]) ? `probe:${match[1]}` : 'probe';
}

export function campaign(imageRef, commit, output, command = docker, profile = process.env.CMA_RUNTIME_EVIDENCE_PROFILE || 'default') {
  assert.ok(profiles.includes(profile));
  assert.match(commit, revision);
  assert.ok(imageRef && !imageRef.startsWith('-'));
  assert.ok(resolve(output) === output && !lstatSync(dirname(output)).isSymbolicLink());
  const expected = profilePins(readFileSync(process.env.CMA_RUNTIME_EVIDENCE_DOCKERFILE || resolve(root, 'api/crates/layer4-runtime/Dockerfile'), 'utf8'), profile);
  const sourceReference = imageRef.includes('@') ? imageRef : null;
  const evidence = { schema: hasCompanionPins(expected) ? evidenceSchema : 'cma-runtime-image-v2', scope: 'local-contract-3-no-provider-or-model', commit, image: null, sourceReference, result: 'inconclusive', cleanup: null, proof: null };
  writeFileSync(output, `${JSON.stringify(evidence)}\n`, { mode: 0o600, flag: 'wx' });
  const identity = process.env.CMA_RUNTIME_EVIDENCE_ID || `cma-runtime-ci-${randomUUID()}`;
  assert.match(identity, /^cma-runtime-ci-[0-9a-f-]{36}$/);
  const label = `dev.chrono.cma.evidence=${identity}`;
  let stage = 'image-admission';
  const scripts = resolve(root, '.github/scripts');
  const mount = (path, name) => ['--mount', `type=bind,source=${path},target=/opt/${name},readonly`];
  const omarchy = profile === 'omarchy';
  const liveAliases = expected.CODEX_VERSION === '0.165.0';
  let prepared = null;
  try {
    const image = JSON.parse(command(['image', 'inspect', imageRef]))[0];
    validateImage(image, commit);
    if (sourceReference !== null) {
      assert.match(sourceReference, /^ghcr\.io\/chronoaiproject\/cma-runtime@sha256:[0-9a-f]{64}$/);
      assert.ok(image.RepoDigests.includes(sourceReference));
    }
    evidence.image = { Id: image.Id, RepoDigests: sourceReference ? [sourceReference] : [], Os: image.Os, Architecture: image.Architecture, Config: { User: image.Config.User, Labels: Object.fromEntries(['org.opencontainers.image.revision', 'org.opencontainers.image.source', 'dev.chrono.cma.runtime-contract', 'dev.chrono.cma.runtime-bootstrap', 'dev.chrono.cma.home-layout'].map((key) => [key, image.Config.Labels[key]])) } };
    if (liveAliases) {
      // Both 0.165.0 profiles need the measured live-process alias layout.
      stage = 'probe-preparation';
      prepared = join(mkdtempSync(join(tmpdir(), 'cma-runtime-probe-')), 'runtime-image-probe.mjs');
      prepareProbe(prepared, scripts, hasCompanionPins(expected));
    }
    stage = 'volume-create';
    command(['volume', 'create', '--label', label, identity]);
    stage = 'container-create';
    command(['create', '--name', identity, '--label', label, '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges=true', '--pids-limit', '128', '--memory', '1g', '--cpus', '2', '--tmpfs', '/tmp:rw,nosuid,nodev,size=536870912,mode=1777', '--tmpfs', '/var/lib/cma-runtime:rw,nosuid,nodev,size=67108864,uid=10001,gid=10001,mode=700', '--mount', `type=volume,source=${identity},target=/home/cma`, ...mount(prepared ?? join(scripts, 'runtime-image-probe.mjs'), 'runtime-image-probe.mjs'), ...mount(join(scripts, 'runtime-github-probe.mjs'), 'runtime-github-probe.mjs'), ...mount(join(scripts, 'runtime-bootstrap-probe.mjs'), 'runtime-bootstrap-probe.mjs'), ...(liveAliases ? mount(join(scripts, 'omarchy-codex-aliases.mjs'), 'omarchy-codex-aliases.mjs') : []), ...(omarchy ? ['omarchy-keyring-probe.py', 'omarchy-chromium-config-probe.py'].flatMap((name) => mount(join(scripts, name), name)) : []), '--env', `CHRONO_SANDBOX_ID=${identity}`, '--env', 'CHRONO_SANDBOX_REACHES=chrono-llm-public,ornn-api', '--env', 'CHRONO_PLATFORM_BASE_URL=http://127.0.0.1:9192', ...(hasCompanionPins(expected) ? ['--env', 'CMA_RUNTIME_NATIVE_PACKAGE_PROOF=1'] : []), image.Id]);
    stage = 'container-start';
    command(['start', identity]);
    stage = 'probe';
    const legacy = JSON.parse(command(['exec', identity, 'node', '/opt/runtime-image-probe.mjs']));
    if (omarchy) {
      stage = 'omarchy-keyring';
      command(['exec', identity, '/usr/bin/python', '-I', '/opt/omarchy-keyring-probe.py']);
      assert.equal(resolve(command(['exec', '--user', '0', identity, '/usr/local/bin/pacman-conf', 'GPGDir']).trim()), '/etc/pacman.d/gnupg');
    }
    stage = 'proof-validation';
    validateProbeChecks(legacy, expected, legacyChecks);
    stage = 'bootstrap-probe';
    const bootstrap = JSON.parse(command(['exec', identity, 'node', '/opt/runtime-bootstrap-probe.mjs']));
    if (omarchy) {
      stage = 'omarchy-chromium';
      command(['exec', identity, '/usr/bin/python', '-I', '/opt/omarchy-chromium-config-probe.py']);
    }
    stage = 'proof-validation';
    evidence.proof = combineProofs(legacy, bootstrap, expected);
    evidence.result = 'pass';
  } catch (error) {
    evidence.result = 'fail';
    process.stderr.write(`Runtime image campaign failed at ${safeFailureStage(stage, error)}; raw diagnostics withheld\n`);
  } finally {
    try {
      const containers = command(['ps', '--all', '--quiet', '--filter', `label=${label}`]).split(/\s+/).filter(Boolean);
      for (const id of containers) {
        assert.match(id, /^[0-9a-f]{12,64}$/);
        command(['rm', '--force', id]);
      }
      const volumes = command(['volume', 'ls', '--quiet', '--filter', `label=${label}`]).split(/\s+/).filter(Boolean);
      for (const id of volumes) {
        assert.equal(id, identity);
        command(['volume', 'rm', id]);
      }
      assert.equal(command(['ps', '--all', '--quiet', '--filter', `label=${label}`]), '');
      assert.equal(command(['volume', 'ls', '--quiet', '--filter', `label=${label}`]), '');
      evidence.cleanup = { containers: 0, volumes: 0 };
    } catch { evidence.result = 'fail'; process.stderr.write('Runtime image campaign failed at cleanup; raw diagnostics withheld\n'); }
    if (prepared) rmSync(dirname(prepared), { recursive: true, force: true });
    writeFileSync(output, `${JSON.stringify(evidence)}\n`, { mode: 0o600 });
  }
  validateEvidence(evidence, commit, evidence.image?.Id, expected, sourceReference);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [mode, imageRef, commit, output] = process.argv.slice(2);
    assert.ok(['run', 'verify'].includes(mode) && output && process.argv.length === 6);
    if (mode === 'run') campaign(imageRef, commit, output);
    // The verify CLI uses the same explicit v1/v2 dispatch as library readers.
    else validateEvidence(JSON.parse(readFileSync(output, 'utf8')), commit, inspect(imageRef).Id, profilePins(readFileSync(process.env.CMA_RUNTIME_EVIDENCE_DOCKERFILE || resolve(root, 'api/crates/layer4-runtime/Dockerfile'), 'utf8'), process.env.CMA_RUNTIME_EVIDENCE_PROFILE || 'default'), imageRef.includes('@') ? imageRef : null);
    process.stdout.write('Runtime image contract and exact cleanup passed\n');
  } catch { process.stderr.write('Runtime image evidence is not passing; raw diagnostics withheld\n'); process.exitCode = 1; }
}
