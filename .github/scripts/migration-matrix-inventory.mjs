// Bind a reviewed shape denominator to the existing artifact evidence verifier.
// A valid declaration is not proof of source completeness or runtime behavior.
import assert from 'node:assert/strict';
import {lstatSync, readFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
import {sha256, validatePlan, verifyFiles} from './migration-matrix.mjs';

const digest = /^[a-f0-9]{64}$/;
const revision = /^[a-f0-9]{40}$/;
const collectionName = /^[a-z][a-z0-9_]{0,127}$/;
const shapeName = /^[a-z][a-z0-9-]{0,95}$/;
const versions = ['old', 'new'];
const policies = ['retained', 'disabled', 'enabled'];

function object(value) {
  assert(value && typeof value === 'object' && !Array.isArray(value));
}

function fields(value, keys) {
  object(value);
  assert.deepEqual(Object.keys(value).sort(), [...keys].sort());
}

function rows(value, max) {
  assert(Array.isArray(value) && value.length > 0 && value.length <= max);
}

function unique(values) {
  assert.equal(new Set(values).size, values.length);
}

function identity(value) {
  fields(value, ['source', 'tree']);
  assert.match(value.source, revision);
  assert.match(value.tree, revision);
}

function requiredKey(collection, shape, writer, policy) {
  return `${collection}/${shape}/${writer}/${policy}`;
}

export function validateInventory(sourceInventory, sourceHash, inventory) {
  object(sourceInventory);
  assert.match(sourceHash, digest);
  assert.equal(sourceInventory.schema, 'cma-reader-writer-source-inventory-v1');
  for (const role of versions) identity(sourceInventory[role]);
  rows(sourceInventory.collections, 512);
  unique(sourceInventory.collections.map(row => row.name));
  const declared = new Map();
  for (const row of sourceInventory.collections) {
    object(row);
    assert.match(row.name, collectionName);
    const writers = versions.filter(role => row[role] !== null && row[role] !== undefined);
    assert(writers.length > 0);
    for (const role of writers) object(row[role]);
    declared.set(row.name, writers);
  }
  fields(inventory, ['schema', 'source_inventory_sha256', 'sources', 'collections']);
  assert.equal(inventory.schema, 'cma-migration-shape-inventory-v1');
  assert.equal(inventory.source_inventory_sha256, sourceHash);
  fields(inventory.sources, versions);
  for (const role of versions) assert.deepEqual(inventory.sources[role], sourceInventory[role]);
  rows(inventory.collections, 512);
  unique(inventory.collections.map(row => row.name));
  assert.deepEqual(inventory.collections.map(row => row.name).sort(), [...declared.keys()].sort());
  const requirements = new Map();
  let shapes = 0;
  for (const row of inventory.collections) {
    fields(row, ['name', 'review', 'shapes']);
    assert.equal(row.review, 'complete');
    rows(row.shapes, 512);
    unique(row.shapes.map(shape => shape.id));
    const observedWriters = new Set();
    for (const shape of row.shapes) {
      fields(shape, ['id', 'definition_sha256', 'writers']);
      assert.match(shape.id, shapeName);
      assert.match(shape.definition_sha256, digest);
      rows(shape.writers, 2);
      unique(shape.writers.map(writer => writer.role));
      for (const writer of shape.writers) {
        fields(writer, ['role', 'policies']);
        assert(declared.get(row.name).includes(writer.role));
        observedWriters.add(writer.role);
        rows(writer.policies, 3);
        unique(writer.policies);
        for (const policy of writer.policies) {
          assert(policies.includes(policy));
          requirements.set(requiredKey(row.name, shape.id, writer.role, policy), {
            collection: row.name, shape: shape.id, writer: writer.role, policy,
            definition_sha256: shape.definition_sha256,
          });
        }
      }
      shapes += 1;
    }
    // A collection retained by both sources cannot silently omit one writer.
    assert.deepEqual([...observedWriters].sort(), [...declared.get(row.name)].sort());
  }
  assert(shapes <= 4096 && requirements.size <= 16384);
  return {collections: declared.size, shapes, requirements};
}

export function validateBindings(plan, planHash, inventory, inventoryHash, sourceInventory,
  sourceHash, bindings) {
  assert.match(planHash, digest);
  assert.match(inventoryHash, digest);
  const required = validateInventory(sourceInventory, sourceHash, inventory);
  validatePlan(plan);
  assert.equal(plan.candidate.source, sourceInventory.new.source);
  // Reader roles describe campaign stages: the canonical generator labels
  // the new binary as current. Bind the old writer by its pinned source.
  const oldReaders = plan.readers.filter(reader => reader.artifact.source === sourceInventory.old.source);
  assert(oldReaders.length > 0);
  for (const reader of oldReaders) {
    assert.deepEqual(reader.artifact, oldReaders[0].artifact);
  }
  const writers = {old: oldReaders[0].artifact, new: plan.candidate};
  fields(bindings, ['schema', 'inventory_sha256', 'plan_sha256', 'bindings']);
  assert.equal(bindings.schema, 'cma-migration-shape-bindings-v1');
  assert.equal(bindings.inventory_sha256, inventoryHash);
  assert.equal(bindings.plan_sha256, planHash);
  rows(bindings.bindings, 16384);
  const matched = new Set(), fixtures = new Set();
  for (const row of bindings.bindings) {
    fields(row, ['collection', 'shape', 'writer', 'policy', 'fixture', 'fixture_sha256', 'definition_sha256']);
    const key = requiredKey(row.collection, row.shape, row.writer, row.policy);
    assert(required.requirements.has(key) && !matched.has(key));
    const fixture = plan.fixtures.find(item => item.id === row.fixture);
    assert(fixture);
    assert.equal(row.fixture_sha256, fixture.sha256);
    assert.equal(row.definition_sha256, required.requirements.get(key).definition_sha256);
    assert.deepEqual(fixture.writer, writers[row.writer]);
    assert.equal(fixture.policy, row.policy);
    matched.add(key);
    fixtures.add(fixture.id);
  }
  assert.equal(matched.size, required.requirements.size);
  // Extra unrelated fixtures cannot pad a declared full-coverage result.
  assert.equal(fixtures.size, plan.fixtures.length);
  return {collections: required.collections, shapes: required.shapes,
    writer_policy_requirements: required.requirements.size, fixtures: fixtures.size};
}

function bounded(path, expectedHash) {
  const stat = lstatSync(path);
  assert(stat.isFile() && !stat.isSymbolicLink() && stat.size > 0 && stat.size <= 4 * 1024 * 1024);
  const bytes = readFileSync(path);
  assert(bytes.length <= 4 * 1024 * 1024);
  if (expectedHash !== undefined) {
    assert.match(expectedHash, digest);
    assert.equal(sha256(bytes), expectedHash);
  }
  return bytes;
}

// Inventory hashes are separate release-owner inputs, never taken on trust
// from the candidate plan or its evidence producer. The source inventory
// still needs review for completeness; this check preserves its denominator.
export function verifyInventoryFiles(planPath, evidencePath, sourcePath, sourceHash,
  inventoryPath, inventoryHash, bindingsPath, expectedSource, expectedImage) {
  const planBytes = bounded(planPath);
  const source = JSON.parse(bounded(sourcePath, sourceHash));
  const inventory = JSON.parse(bounded(inventoryPath, inventoryHash));
  const bindingBytes = bounded(bindingsPath);
  const bindings = JSON.parse(bindingBytes);
  const plan = JSON.parse(planBytes);
  assert.deepEqual(plan.candidate, {source: expectedSource, image: expectedImage});
  const coverage = validateBindings(plan, sha256(planBytes), inventory, inventoryHash,
    source, sourceHash, bindings);
  const execution = verifyFiles(planPath, evidencePath, expectedSource, expectedImage);
  // Refuse if an input changed between the inventory and evidence passes.
  assert.equal(sha256(bounded(planPath)), sha256(planBytes));
  bounded(sourcePath, sourceHash);
  bounded(inventoryPath, inventoryHash);
  assert.equal(sha256(bounded(bindingsPath)), sha256(bindingBytes));
  return {scope: 'DECLARED_INVENTORY_AND_EVIDENCE_CONSISTENCY_ONLY', ...coverage, ...execution};
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    assert.equal(process.argv.length, 11);
    const result = verifyInventoryFiles(...process.argv.slice(2));
    process.stdout.write(JSON.stringify(result) + '\n');
  } catch {
    process.stderr.write('Migration inventory or evidence is incomplete or inconsistent; raw diagnostics withheld\n');
    process.exitCode = 1;
  }
}
