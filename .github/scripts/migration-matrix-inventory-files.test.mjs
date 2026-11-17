import assert from 'node:assert/strict';
import {execFileSync, spawnSync} from 'node:child_process';
import {mkdtempSync, rmSync, symlinkSync, writeFileSync} from 'node:fs';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';
import test from 'node:test';
import {sha256} from './migration-matrix.mjs';
import {verifyInventoryFiles} from './migration-matrix-inventory.mjs';
import {campaign} from './migration-matrix-inventory.fixture.mjs';

const directory = dirname(fileURLToPath(import.meta.url));
const script = join(directory, 'migration-matrix-inventory.mjs');
const failure = 'Migration inventory or evidence is incomplete or inconsistent; raw diagnostics withheld\n';

function fixture(operation) {
  const root = mkdtempSync(join(directory, '.matrix-inventory-'));
  try {
    const c = campaign(), paths = {};
    for (const name of ['plan', 'evidence', 'source', 'inventory', 'bindings']) {
      paths[name] = join(root, name + '.json');
      writeFileSync(paths[name], JSON.stringify(c[name]));
    }
    for (const [path, bytes] of c.raw) writeFileSync(join(root, path), bytes);
    const args = [paths.plan, paths.evidence, paths.source, sha256(JSON.stringify(c.source)),
      paths.inventory, sha256(JSON.stringify(c.inventory)), paths.bindings,
      c.plan.candidate.source, c.plan.candidate.image];
    operation({root, c, paths, args});
  } finally {
    rmSync(root, {recursive: true, force: true});
  }
}

function cli(args) {
  return spawnSync(process.execPath, [script, ...args], {encoding: 'utf8'});
}

function rejected(args) {
  const result = cli(args);
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, failure);
}

test('CLI composes full declared inventory and existing artifact evidence checks', () => {
  fixture(({args}) => {
    const result = verifyInventoryFiles(...args);
    assert.equal(result.scope, 'DECLARED_INVENTORY_AND_EVIDENCE_CONSISTENCY_ONLY');
    assert.equal(result.collections, 2);
    assert.equal(result.writer_policy_requirements, 6);
    assert.equal(result.cells, 12);
    assert.equal(result.accepted, 12);
    assert.deepEqual(JSON.parse(execFileSync(process.execPath, [script, ...args], {encoding: 'utf8'})), result);
  });
});

test('independent source and shape pins cannot be replaced by matching producer files', () => {
  for (const index of [3, 5, 7, 8]) fixture(({args}) => {
    args[index] = index === 8 ? 'registry.example/test/api@sha256:' + sha256('other') : sha256('other');
    assert.throws(() => verifyInventoryFiles(...args));
    rejected(args);
  });
});

test('file inputs refuse malformed, empty, excessive and symlink content', () => {
  for (const field of ['plan', 'source', 'inventory', 'bindings', 'evidence']) {
    for (const mode of ['malformed', 'empty', 'oversize', 'symlink']) fixture(({args, paths, root}) => {
      const path = paths[field];
      if (mode === 'symlink') {
        rmSync(path); symlinkSync(join(root, 'source.json'), path);
      } else {
        writeFileSync(path, mode === 'malformed' ? '{untrusted private payload' :
          mode === 'empty' ? '' : 'x'.repeat(4 * 1024 * 1024 + 1));
      }
      assert.throws(() => verifyInventoryFiles(...args));
      rejected(args);
    });
  }
});

test('missing raw evidence and unsafe raw paths remain rejected', () => {
  for (const path of ['missing.json', '../private.json', '/etc/passwd', 'nested/../../outside', 'link.json']) {
    fixture(({args, paths, c, root}) => {
      const previous = c.evidence.cells[0].raw.path;
      c.evidence.cells[0].raw.path = path;
      if (path === 'link.json') symlinkSync(join(root, previous), join(root, path));
      writeFileSync(paths.evidence, JSON.stringify(c.evidence));
      assert.throws(() => verifyInventoryFiles(...args));
      rejected(args);
    });
  }
});

test('inventory cannot turn failed boots, retries or changed raw observations into PASS', () => {
  const mutations = [
    c => {c.evidence.cells[0].checks.restart = false;},
    c => {c.evidence.cells[0].attempt = 2;},
    c => {c.evidence.cells[0].outcome = 'failed_boot';},
    c => {c.evidence.cells.pop();},
    c => {c.evidence.cells[0].samples = 0;},
    c => {c.evidence.cells[0].reader.source = sha256('other').slice(0, 40);},
  ];
  for (const mutate of mutations) fixture(({args, paths, c}) => {
    mutate(c); writeFileSync(paths.evidence, JSON.stringify(c.evidence));
    assert.throws(() => verifyInventoryFiles(...args));
    rejected(args);
  });
});

test('unknown options, absent files and truncated argument lists are sanitized', () => {
  rejected([]);
  fixture(({args, paths}) => {
    rmSync(paths.inventory);
    rejected(args);
    rejected([...args, '--skip-inventory']);
  });
});
