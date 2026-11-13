import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { pins, validateEvidence } from './runtime-image-evidence.mjs';

// Reviewer #1742 reproduction: a complete pre-change v1 record bound to its
// original commit/image, with the eight original artifact fields. This frozen
// synthetic fixture follows fa292b834; it is not deployment acceptance evidence.
const historical = JSON.parse(readFileSync(new URL('./fixtures/runtime-image-evidence-v1.json', import.meta.url), 'utf8'));
const expected = pins(readFileSync(new URL('../../infra/runtime/Dockerfile', import.meta.url), 'utf8'));
const current = () => {
  const value = structuredClone(historical);
  value.schema = 'cma-runtime-image-v2';
  Object.assign(value.proof.artifacts, { psVersion: '4.0.4', jqVersion: '1.7' });
  return value;
};
const read = value => validateEvidence(value, historical.commit, historical.image.Id, expected);

test('historical v1 passes its original artifact rules and valid v2 passes the new rules', () => {
  assert.ok(!Object.hasOwn(historical.proof.artifacts, 'psVersion'));
  assert.ok(!Object.hasOwn(historical.proof.artifacts, 'jqVersion'));
  assert.doesNotThrow(() => read(historical));
  assert.doesNotThrow(() => read(current()));
});

test('v2 requires both tool versions and rejects malformed versions', () => {
  for (const field of ['psVersion', 'jqVersion']) {
    const missing = current();
    delete missing.proof.artifacts[field];
    assert.throws(() => read(missing));
    for (const version of ['', 'not-a-version', null, 1]) {
      const invalid = current();
      invalid.proof.artifacts[field] = version;
      assert.throws(() => read(invalid));
    }
  }
  assert.throws(() => read({ ...historical, schema: 'cma-runtime-image-v2' }));
});

test('unknown schemas fail without fallback and v1 retains its exact artifact shape', () => {
  for (const schema of ['cma-runtime-image-v3', '', null, 1, '__proto__']) {
    assert.throws(() => read({ ...current(), schema }), /unsupported runtime image evidence schema/);
  }
  const absent = current();
  delete absent.schema;
  assert.throws(() => read(absent));
  assert.throws(() => read({ ...current(), schema: 'cma-runtime-image-v1' }));
});

test('both schema profiles preserve original commit, image, artifact, probe and cleanup fences', () => {
  for (const original of [historical, current()]) {
    for (const mutate of [
      v => { v.commit = '0'.repeat(40); },
      v => { v.image.Id = `sha256:${'0'.repeat(64)}`; },
      v => { v.image.Config.Labels['org.opencontainers.image.revision'] = '0'.repeat(40); },
      v => { v.proof.artifacts.native = '0'.repeat(64); },
      v => { v.proof.checks.pop(); },
      v => { v.cleanup.containers = 1; },
      v => { v.result = 'fail'; },
    ]) {
      const changed = structuredClone(original);
      mutate(changed);
      assert.throws(() => read(changed));
    }
    const reference = `ghcr.io/chronoaiproject/cma-runtime@sha256:${'9'.repeat(64)}`;
    const published = structuredClone(original);
    published.sourceReference = reference;
    published.image.RepoDigests = [reference];
    validateEvidence(published, historical.commit, historical.image.Id, expected, reference);
    published.image.RepoDigests = [];
    assert.throws(() => validateEvidence(published, historical.commit, historical.image.Id, expected, reference));
  }
});

test('verify CLI dispatches historical v1 and v2 and refuses incomplete or unknown profiles', () => {
  const directory = mkdtempSync(join(tmpdir(), 'cma-evidence-compat-'));
  try {
    // Only image inspection is simulated. The real verify entrypoint parses and
    // validates each record; no Docker daemon, container or network is used.
    writeFileSync(join(directory, 'docker'), `#!${process.execPath}\n` +
      `const assert = require('node:assert/strict');\n` +
      `assert.deepEqual(process.argv.slice(2), ['image', 'inspect', 'runtime:historical']);\n` +
      `process.stdout.write(${JSON.stringify(JSON.stringify([historical.image]))});\n`, { mode: 0o700 });
    const script = fileURLToPath(new URL('./runtime-image-evidence.mjs', import.meta.url));
    const missingPs = current();
    delete missingPs.proof.artifacts.psVersion;
    const missingJq = current();
    delete missingJq.proof.artifacts.jqVersion;
    for (const [value, status] of [
      [historical, 0], [current(), 0], [missingPs, 1], [missingJq, 1],
      [{ ...historical, schema: 'cma-runtime-image-v2' }, 1],
      [{ ...current(), schema: 'cma-runtime-image-v3' }, 1],
    ]) {
      const output = join(directory, 'evidence.json');
      writeFileSync(output, JSON.stringify(value), { mode: 0o600 });
      const result = spawnSync(process.execPath, [script, 'verify', 'runtime:historical', historical.commit, output], {
        cwd: dirname(script), env: { ...process.env, PATH: `${directory}:${process.env.PATH}` },
        encoding: 'utf8', timeout: 10000,
      });
      assert.ifError(result.error);
      assert.equal(result.signal, null);
      assert.equal(result.status, status, result.stderr);
      if (status === 0) assert.match(result.stdout, /contract and exact cleanup passed/);
      else assert.equal(result.stderr, 'Runtime image evidence is not passing; raw diagnostics withheld\n');
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
