// Synthetic gate contracts only. These objects are never application evidence.
import {sha256} from './migration-matrix.mjs';

const artifact = version => ({source: sha256(version).slice(0, 40),
  image: `registry.example/test/api@sha256:${sha256(version + '-image')}`});
const start = '2026-11-10T14:00:00.000Z', end = '2026-11-10T14:01:00.000Z';

export function campaign() {
  const old = artifact('old'), candidate = artifact('new');
  const source = {schema: 'cma-reader-writer-source-inventory-v1',
    old: {source: old.source, tree: sha256('old-tree').slice(0, 40)},
    new: {source: candidate.source, tree: sha256('new-tree').slice(0, 40)},
    collections: ['alpha', 'beta'].map(name => ({name, old: {role: 'runtime'}, new: {role: 'runtime'}}))};
  const inventory = {schema: 'cma-migration-shape-inventory-v1',
    source_inventory_sha256: sha256(JSON.stringify(source)),
    sources: {old: source.old, new: source.new},
    collections: source.collections.map(row => ({name: row.name, review: 'complete',
      shapes: [{id: 'ordinary', definition_sha256: sha256(row.name), writers: [
        {role: 'old', policies: ['retained']}, {role: 'new', policies: ['disabled', 'enabled']},
      ]}]}))};
  const readers = ['historical', 'current', 'candidate', 'recovery'].map(role => ({
    id: role, role, artifact: ['historical', 'current'].includes(role) ? old : candidate,
  }));
  const fixtures = ['retained', 'disabled', 'enabled'].map(policy => ({
    id: policy, sha256: sha256(policy), policy, writer: policy === 'retained' ? old : candidate,
  }));
  const cells = readers.flatMap(reader => fixtures.map(fixture => ({
    id: `${reader.id}-${fixture.id}`, reader: reader.id, fixture: fixture.id, expectation: 'accept',
  })));
  const plan = {schema: 'cma-migration-matrix-plan-v1', candidate, readers, fixtures, cells};
  const bindings = {schema: 'cma-migration-shape-bindings-v1',
    inventory_sha256: sha256(JSON.stringify(inventory)), plan_sha256: sha256(JSON.stringify(plan)),
    bindings: inventory.collections.flatMap(row => ['retained', 'disabled', 'enabled'].map(policy => ({
      collection: row.name, shape: 'ordinary', writer: policy === 'retained' ? 'old' : 'new', policy, fixture: policy,
      fixture_sha256: sha256(policy), definition_sha256: row.shapes[0].definition_sha256,
    })))};
  const raw = new Map();
  const evidence = {schema: 'cma-migration-matrix-evidence-v1', plan_sha256: sha256(JSON.stringify(plan)),
    candidate, environment: 'isolated-fixture', configuration_sha256: sha256('config'),
    started_at: start, finished_at: end, cells: cells.map(cell => {
      const fixture = fixtures.find(f => f.id === cell.fixture);
      const observation = {id: cell.id, reader: readers.find(r => r.id === cell.reader).artifact,
        writer: fixture.writer, fixture_sha256: fixture.sha256, policy: fixture.policy,
        execution: 'artifact', outcome: 'accept', attempt: 1, samples: 1,
        started_at: start, finished_at: end, checks: {decode: true, replay: true, restart: true,
          takeover: true, identity_preserved: true, writer_policy: true}};
      const bytes = Buffer.from(JSON.stringify(observation)), path = cell.id + '.json';
      raw.set(path, bytes);
      return {...observation, raw: {path, sha256: sha256(bytes)}};
    })};
  return {source, inventory, plan, bindings, evidence, raw};
}

export function inputs(value) {
  return [value.plan, sha256(JSON.stringify(value.plan)), value.inventory,
    sha256(JSON.stringify(value.inventory)), value.source, sha256(JSON.stringify(value.source)), value.bindings];
}

export function rebind(value) {
  value.inventory.source_inventory_sha256 = sha256(JSON.stringify(value.source));
  value.bindings.inventory_sha256 = sha256(JSON.stringify(value.inventory));
  value.bindings.plan_sha256 = sha256(JSON.stringify(value.plan));
  value.evidence.plan_sha256 = sha256(JSON.stringify(value.plan));
}
