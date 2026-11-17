import assert from 'node:assert/strict';
import test from 'node:test';
import {sha256, validatePlan, validateEvidence} from './migration-matrix.mjs';
import {validateInventory, validateBindings} from './migration-matrix-inventory.mjs';
import {campaign, inputs, rebind} from './migration-matrix-inventory.fixture.mjs';

test('complete reviewed shapes require both writers and every declared policy', () => {
  const c = campaign();
  assert.deepEqual(validateBindings(...inputs(c)), {
    collections: 2, shapes: 2, writer_policy_requirements: 6, fixtures: 3,
  });
  assert.equal(validateEvidence(c.plan, sha256(JSON.stringify(c.plan)), c.evidence,
    path => c.raw.get(path)).accepted, 12);
});

test('a consistent scoped campaign cannot hide an omitted collection', () => {
  const c = campaign();
  c.inventory.collections.pop();
  c.bindings.bindings = c.bindings.bindings.filter(b => b.collection === 'alpha');
  rebind(c);
  assert.doesNotThrow(() => validatePlan(c.plan));
  assert.equal(validateEvidence(c.plan, sha256(JSON.stringify(c.plan)), c.evidence,
    path => c.raw.get(path)).cells, 12);
  assert.throws(() => validateBindings(...inputs(c)));
});

test('unknown, empty, missing and duplicate shapes never become full coverage', () => {
  const mutations = [
    c => {c.inventory.collections[0].review = 'unresolved';},
    c => {c.inventory.collections[0].shapes = [];},
    c => {c.inventory.collections[0].shapes.push(structuredClone(c.inventory.collections[0].shapes[0]));},
    c => {c.inventory.collections.push(structuredClone(c.inventory.collections[0]));},
    c => {c.inventory.collections[0].shapes[0].definition_sha256 = 'partial';},
    c => {c.inventory.collections[0].shapes[0].writers.pop();},
    c => {c.inventory.collections[0].shapes[0].writers[1].policies = [];},
    c => {c.inventory.collections[0].shapes[0].writers[1].policies = ['enabled', 'enabled'];},
    c => {c.inventory.collections[0].shapes[0].writers[1].policies = ['unknown'];},
    c => {c.inventory.collections[0].shapes[0].writers[1].role = 'historical';},
    c => {c.inventory.collections[0].shapes[0].writers[1].role = 'old';},
    c => {c.inventory.collections[0].shapes[0].extra = true;},
    c => {c.inventory.sources.new.tree = 'moving-ref';},
    c => {c.inventory.schema = 'unknown';},
  ];
  for (const mutate of mutations) {
    const c = campaign(); mutate(c); rebind(c);
    assert.throws(() => validateBindings(...inputs(c)), String(mutate));
  }
});

test('every independent source row remains required, including removed collections', () => {
  const c = campaign();
  c.source.collections[1].new = null;
  c.inventory.collections[1].shapes[0].writers.pop();
  c.bindings.bindings = c.bindings.bindings.filter(b => b.collection !== 'beta' || b.writer === 'old');
  rebind(c);
  assert.equal(validateBindings(...inputs(c)).writer_policy_requirements, 4);
  c.inventory.collections.pop(); rebind(c);
  assert.throws(() => validateBindings(...inputs(c)));
});

test('a newly introduced collection requires new writer policies without invented old data', () => {
  const c = campaign();
  c.source.collections[1].old = null;
  c.inventory.collections[1].shapes[0].writers.shift();
  c.bindings.bindings = c.bindings.bindings.filter(b => b.collection !== 'beta' || b.writer === 'new');
  rebind(c);
  assert.equal(validateBindings(...inputs(c)).writer_policy_requirements, 5);
});

test('source denominator identity and collection definitions are validated', () => {
  const mutations = [
    c => {c.source.schema = 'unknown';},
    c => {c.source.old.source = 'develop';},
    c => {c.source.new.tree = 'unknown';},
    c => {c.source.collections = [];},
    c => {c.source.collections.push(structuredClone(c.source.collections[0]));},
    c => {c.source.collections[0].old = null; c.source.collections[0].new = null;},
    c => {c.source.collections[0].name = '../foreign';},
    c => {c.source.collections[0].old = true;},
    c => {c.source.collections = new Array(513).fill(c.source.collections[0]);},
  ];
  for (const mutate of mutations) {
    const c = campaign(); mutate(c); rebind(c);
    assert.throws(() => validateBindings(...inputs(c)), String(mutate));
  }
});

test('mapping omissions, duplicates, foreign shapes and stale hashes are refused', () => {
  const mutations = [
    c => {c.bindings.bindings.pop();},
    c => {c.bindings.bindings[1] = structuredClone(c.bindings.bindings[0]);},
    c => {c.bindings.bindings[0].collection = 'foreign';},
    c => {c.bindings.bindings[0].shape = 'missing';},
    c => {c.bindings.bindings[0].writer = 'new';},
    c => {c.bindings.bindings[0].policy = 'enabled';},
    c => {c.bindings.bindings[0].fixture = 'missing';},
    c => {c.bindings.bindings[0].fixture = 'enabled';},
    c => {c.bindings.bindings[0].fixture_sha256 = sha256('wrong');},
    c => {c.bindings.bindings[0].definition_sha256 = sha256('wrong');},
    c => {c.bindings.bindings[0].extra = 'unsupported';},
    c => {c.bindings.inventory_sha256 = sha256('wrong');},
    c => {c.bindings.plan_sha256 = sha256('wrong');},
    c => {c.inventory.source_inventory_sha256 = sha256('wrong');},
    c => {c.inventory.sources.old = {...c.source.old, tree: sha256('different').slice(0, 40)};},
  ];
  for (const mutate of mutations) {
    const c = campaign(); mutate(c);
    assert.throws(() => validateBindings(...inputs(c)), String(mutate));
  }
});

test('unchanged hashes cannot bind an unrelated candidate or current writer image', () => {
  const c = campaign();
  c.plan.readers.find(r => r.role === 'current').artifact = {
    ...c.plan.readers[0].artifact, image: 'registry.example/test/api@sha256:' + sha256('other'),
  };
  rebind(c);
  assert.throws(() => validateBindings(...inputs(c)));
  const other = campaign();
  other.plan.candidate = {...other.plan.candidate, source: sha256('unrelated').slice(0, 40)};
  other.plan.readers.find(r => r.role === 'candidate').artifact = other.plan.candidate;
  rebind(other);
  assert.throws(() => validateBindings(...inputs(other)));
});

test('an unrelated fixture cannot pad the evidence denominator', () => {
  const c = campaign();
  c.plan.fixtures.push({...c.plan.fixtures[0], id: 'padding'});
  for (const reader of c.plan.readers) c.plan.cells.push({id: reader.id + '-padding',
    reader: reader.id, fixture: 'padding', expectation: 'accept'});
  rebind(c);
  assert.doesNotThrow(() => validatePlan(c.plan));
  assert.throws(() => validateBindings(...inputs(c)));
});

test('reviewed inventories can exceed 128 collections without omitting rows', () => {
  const c = campaign();
  for (let i = 0; i < 160; i++) {
    const name = 'collection_' + i;
    c.source.collections.push({...structuredClone(c.source.collections[0]), name});
    c.inventory.collections.push({...structuredClone(c.inventory.collections[0]), name});
    for (const mapping of c.bindings.bindings.slice(0, 3)) c.bindings.bindings.push({...mapping, collection: name});
  }
  rebind(c);
  assert.equal(validateBindings(...inputs(c)).collections, 162);
  assert.equal(validateInventory(c.source, sha256(JSON.stringify(c.source)), c.inventory).requirements.size, 486);
});
