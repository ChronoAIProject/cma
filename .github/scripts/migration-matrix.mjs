// Validate a reviewed matrix against retained artifact-execution receipts.
// This verifies evidence consistency; it does not execute or approve an image.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, realpathSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

const hashPattern = /^[a-f0-9]{64}$/;
const sourcePattern = /^[a-f0-9]{40}$/;
const idPattern = /^[a-z][a-z0-9-]{0,95}$/;
// Matrix evidence may bind either the production Artifact Registry image or a
// private registry mirror. A tag, local config ID, and bare sha256 are not
// executable provenance and must remain invalid.
const imagePattern = /^(?:[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::[0-9]+)?\/)+[a-z0-9][a-z0-9._\/-]*@sha256:[a-f0-9]{64}$/;
const roles = ['historical', 'current', 'candidate', 'recovery'];
const policies = ['retained', 'disabled', 'enabled'];
const maxCells = 4096;
const requiredChecks = {
  accept: ['decode', 'replay', 'restart', 'takeover', 'identity_preserved', 'writer_policy'],
  refuse: ['refused_before_traffic', 'refused_before_write', 'identity_preserved'],
};
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

function fields(value, names) {
  assert(value && typeof value === 'object' && !Array.isArray(value));
  assert.deepEqual(Object.keys(value).sort(), [...names].sort());
}

function bounded(values, maximum = 256) {
  assert(Array.isArray(values) && values.length > 0 && values.length <= maximum);
}

function unique(values) {
  assert.equal(new Set(values).size, values.length);
}

function artifact(value) {
  fields(value, ['source', 'image']);
  assert.match(value.source, sourcePattern);
  // An OCI root digest, not a tag or a local Docker config ID. The registry
  // name is deliberately generic because production uses Artifact Registry.
  assert.match(value.image, imagePattern);
}

function timestamp(value) {
  assert.equal(typeof value, 'string');
  assert.match(value, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  const time = Date.parse(value);
  assert(Number.isFinite(time));
  assert.equal(new Date(time).toISOString(), value);
  return time;
}

export function validatePlan(plan) {
  fields(plan, ['schema', 'candidate', 'readers', 'fixtures', 'cells']);
  assert.equal(plan.schema, 'cma-migration-matrix-plan-v1');
  artifact(plan.candidate);
  const component = plan.candidate.image.split('@')[0];
  bounded(plan.readers, 32);
  // Every reader must exercise every fixture within the same cell budget.
  bounded(plan.fixtures, Math.floor(maxCells / plan.readers.length));
  bounded(plan.cells, maxCells);
  unique(plan.readers.map(row => row.id));
  unique(plan.fixtures.map(row => row.id));
  unique(plan.cells.map(row => row.id));
  for (const reader of plan.readers) {
    fields(reader, ['id', 'role', 'artifact']);
    assert.match(reader.id, idPattern);
    assert(roles.includes(reader.role));
    artifact(reader.artifact);
    assert.equal(reader.artifact.image.split('@')[0], component);
    if (reader.role === 'candidate') assert.deepEqual(reader.artifact, plan.candidate);
  }
  for (const role of roles) assert(plan.readers.some(row => row.role === role));
  assert.equal(plan.readers.filter(row => row.role === 'candidate').length, 1);
  for (const fixture of plan.fixtures) {
    fields(fixture, ['id', 'sha256', 'writer', 'policy']);
    assert.match(fixture.id, idPattern);
    assert.match(fixture.sha256, hashPattern);
    artifact(fixture.writer);
    assert.equal(fixture.writer.image.split('@')[0], component);
    assert(policies.includes(fixture.policy));
  }
  for (const policy of policies) assert(plan.fixtures.some(row => row.policy === policy));
  for (const policy of ['disabled', 'enabled']) {
    assert(plan.fixtures.some(row => row.policy === policy
      && row.writer.source === plan.candidate.source && row.writer.image === plan.candidate.image));
  }
  const pairs = [];
  for (const cell of plan.cells) {
    fields(cell, ['id', 'reader', 'fixture', 'expectation']);
    assert.match(cell.id, idPattern);
    const reader = plan.readers.find(row => row.id === cell.reader);
    assert(reader && plan.fixtures.some(row => row.id === cell.fixture));
    assert(Object.hasOwn(requiredChecks, cell.expectation));
    if (['candidate', 'recovery'].includes(reader.role)) assert.equal(cell.expectation, 'accept');
    pairs.push(`${cell.reader}/${cell.fixture}`);
  }
  unique(pairs);
  // A caller cannot drop an inconvenient old-reader/new-writer cell.
  assert.equal(pairs.length, plan.readers.length * plan.fixtures.length);
  return plan;
}

function checks(value, expectation) {
  fields(value, requiredChecks[expectation]);
  for (const result of Object.values(value)) assert.equal(result, true);
}

export function validateEvidence(plan, planHash, evidence, readArtifact) {
  validatePlan(plan);
  assert.match(planHash, hashPattern);
  fields(evidence, ['schema', 'plan_sha256', 'candidate', 'environment', 'configuration_sha256',
    'started_at', 'finished_at', 'cells']);
  assert.equal(evidence.schema, 'cma-migration-matrix-evidence-v1');
  assert.equal(evidence.plan_sha256, planHash);
  assert.deepEqual(evidence.candidate, plan.candidate);
  assert(['isolated-fixture', 'production'].includes(evidence.environment));
  assert.match(evidence.configuration_sha256, hashPattern);
  const start = timestamp(evidence.started_at), end = timestamp(evidence.finished_at);
  assert(end >= start);
  bounded(evidence.cells, maxCells);
  unique(evidence.cells.map(row => row.id));
  assert.equal(evidence.cells.length, plan.cells.length);
  for (const row of evidence.cells) {
    fields(row, ['id', 'reader', 'writer', 'fixture_sha256', 'policy', 'execution', 'outcome',
      'attempt', 'samples', 'started_at', 'finished_at', 'checks', 'raw']);
    const cell = plan.cells.find(value => value.id === row.id);
    assert(cell);
    const reader = plan.readers.find(value => value.id === cell.reader);
    const fixture = plan.fixtures.find(value => value.id === cell.fixture);
    assert.deepEqual(row.reader, reader.artifact);
    assert.deepEqual(row.writer, fixture.writer);
    assert.equal(row.fixture_sha256, fixture.sha256);
    assert.equal(row.policy, fixture.policy);
    assert.equal(row.execution, 'artifact');
    assert.equal(row.outcome, cell.expectation);
    // Keep failed/retried campaigns as evidence, but do not certify them as clean passes.
    assert.equal(row.attempt, 1);
    assert(Number.isSafeInteger(row.samples) && row.samples > 0);
    const cellStart = timestamp(row.started_at), cellEnd = timestamp(row.finished_at);
    assert(cellStart >= start && cellEnd <= end && cellEnd >= cellStart);
    checks(row.checks, cell.expectation);
    fields(row.raw, ['path', 'sha256']);
    assert.match(row.raw.sha256, hashPattern);
    assert.equal(typeof readArtifact, 'function');
    const bytes = readArtifact(row.raw.path);
    assert.equal(sha256(bytes), row.raw.sha256);
    const raw = JSON.parse(bytes.toString('utf8'));
    // The retained receipt must bind the complete observation, not just an arbitrary log.
    const { raw: _reference, ...observation } = row;
    assert.deepEqual(raw, observation);
  }
  return { cells: evidence.cells.length, accepted: evidence.cells.filter(row => row.outcome === 'accept').length,
    refused: evidence.cells.filter(row => row.outcome === 'refuse').length };
}

function boundedFile(path) {
  const stat = lstatSync(path);
  assert(stat.isFile() && !stat.isSymbolicLink() && stat.size > 0 && stat.size <= 4 * 1024 * 1024);
  return readFileSync(path);
}

function containedFile(root, relative) {
  assert.equal(typeof relative, 'string');
  const parts = relative.split('/');
  assert(parts.length > 0 && parts.length <= 8);
  assert(parts.every(part => /^[a-zA-Z0-9_-][a-zA-Z0-9_.-]{0,127}$/.test(part)));
  let path = root;
  for (const part of parts) {
    path = join(path, part);
    assert(!lstatSync(path).isSymbolicLink());
  }
  return boundedFile(path);
}

export function verifyFiles(planPath, evidencePath, expectedSource, expectedImage) {
  return verifyEvidenceFiles(boundedFile(planPath), evidencePath, expectedSource, expectedImage);
}

export function verifyEvidenceFiles(planBytes, evidencePath, expectedSource, expectedImage) {
  const plan = validatePlan(JSON.parse(planBytes.toString('utf8')));
  // The release's selected candidate is independent of the evidence producer.
  assert.deepEqual(plan.candidate, { source: expectedSource, image: expectedImage });
  const evidence = JSON.parse(boundedFile(evidencePath).toString('utf8'));
  const root = realpathSync(dirname(evidencePath));
  return validateEvidence(plan, sha256(planBytes), evidence, path => containedFile(root, path));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    assert.equal(process.argv.length, 6);
    const summary = verifyFiles(...process.argv.slice(2));
    process.stdout.write(`Migration matrix evidence consistent: ${summary.cells} cells (${summary.accepted} accepted, ${summary.refused} refused)\n`);
  } catch {
    process.stderr.write('Migration matrix evidence is incomplete or inconsistent; raw diagnostics withheld\n');
    process.exitCode = 1;
  }
}
