import assert from 'node:assert/strict';
import {execFileSync, spawnSync} from 'node:child_process';
import {mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync} from 'node:fs';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';
import test from 'node:test';
import {sha256, verifyFiles} from './migration-matrix.mjs';
import {validateBindings, verifyInventoryFiles} from './migration-matrix-inventory.mjs';
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
  return spawnSync(process.execPath, [script, ...args], {encoding: 'utf8', timeout: 10000});
}

function rejected(args) {
  const result = cli(args);
  assert.ifError(result.error);
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

for (const schedule of ['A-B-A', 'A-B']) {
  test(`plan replacement ${schedule} cannot mix inventory bindings and execution evidence`, () => {
    fixture(({root, c, paths, args}) => {
      const replacement = structuredClone(c);
      replacement.plan.fixtures.find(row => row.policy === 'retained').writer = replacement.plan.candidate;
      const replacementBytes = JSON.stringify(replacement.plan);
      replacement.bindings.plan_sha256 = sha256(replacementBytes);
      assert.throws(() => validateBindings(replacement.plan, sha256(replacementBytes), replacement.inventory,
        args[5], replacement.source, args[3], replacement.bindings));
      replacement.evidence.plan_sha256 = sha256(replacementBytes);
      for (const row of replacement.evidence.cells) {
        if (row.policy === 'retained') row.writer = replacement.plan.candidate;
        const {raw, ...observation} = row;
        const bytes = JSON.stringify(observation);
        row.raw.sha256 = sha256(bytes);
        writeFileSync(join(root, raw.path), bytes);
      }
      writeFileSync(paths.evidence, JSON.stringify(replacement.evidence));
      const replacementPath = join(root, 'replacement.json');
      writeFileSync(replacementPath, replacementBytes);
      assert.equal(verifyFiles(replacementPath, paths.evidence, args[7], args[8]).cells, 12);
      rejected(args);

      // Schedule actual atomic replacements; reads still return the real file bytes.
      const hook = join(root, 'replace-plan.mjs');
      const record = join(root, 'schedule.json');
      writeFileSync(hook, `
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {syncBuiltinESMExports} from 'node:module';
const [planPath, evidencePath] = process.argv.slice(2);
const replacementPath = ${JSON.stringify(replacementPath)};
const recordPath = ${JSON.stringify(record)};
const schedule = ${JSON.stringify(schedule)};
const originalRead = fs.readFileSync;
const replacement = originalRead(replacementPath);
let captured, replaced = false, restored = false, evidenceRead = false;
function replace(bytes) {
  fs.writeFileSync(planPath + '.next', bytes);
  fs.renameSync(planPath + '.next', planPath);
}
fs.readFileSync = function(path, ...options) {
  const bytes = originalRead(path, ...options);
  if (path === planPath && !replaced) {
    captured = bytes;
    replace(replacement);
    replaced = true;
  } else if (path === evidencePath) {
    assert(replaced);
    evidenceRead = true;
    if (schedule === 'A-B-A') {
      replace(captured);
      restored = true;
    }
  }
  return bytes;
};
syncBuiltinESMExports();
process.on('exit', () => {
  fs.writeFileSync(recordPath, JSON.stringify({replaced, restored, evidenceRead}));
});
`);
      const result = spawnSync(process.execPath, ['--import', hook, script, ...args],
        {encoding: 'utf8', timeout: 10000});
      assert.ifError(result.error);
      assert.deepEqual(JSON.parse(readFileSync(record, 'utf8')),
        {replaced: true, restored: schedule === 'A-B-A', evidenceRead: true});
      assert.equal(readFileSync(paths.plan, 'utf8'),
        schedule === 'A-B-A' ? JSON.stringify(c.plan) : replacementBytes);
      assert.equal(result.status, 1, 'mismatched captured plan and evidence must refuse');
      assert.equal(result.stdout, '');
      assert.equal(result.stderr, failure);
    });
  });
}

function releaseFixture(operation) {
  fixture(({root, c, paths, args}) => {
    const roles = ['historical', 'current', 'candidate', 'recovery'];
    const artifacts = Object.fromEntries(roles.map(role => [role,
      role === 'historical' ? c.plan.fixtures[0].writer : role === 'candidate' ? c.plan.candidate : {
        source: sha256(role).slice(0, 40), image: `registry.example/test/api@sha256:${sha256(role + '-image')}`,
      }]));
    const expectations = roles.flatMap(reader => c.source.collections.flatMap(({name}) =>
      ['retained', 'disabled', 'enabled'].map(policy => {
        const id = `collection-${name}-${policy}`;
        return {id: `${reader}-${id}`, reader, fixture: id,
          expectation: reader === 'historical' && policy === 'enabled' ? 'refuse' : 'accept'};
      })));
    const generatorArgs = ['--inventory', paths.source];
    for (const [name, value] of Object.entries({artifacts, expectations})) {
      const path = join(root, name + '.json');
      writeFileSync(path, JSON.stringify(value));
      generatorArgs.push('--' + name, path);
    }
    rmSync(paths.plan);
    const generated = spawnSync('python3', ['-B', join(directory, 'migration_plan.py'),
      ...generatorArgs, '--output', paths.plan], {encoding: 'utf8', timeout: 10000});
    assert.ifError(generated.error);
    assert.equal(generated.status, 0, generated.stderr);
    assert.equal(generated.stderr, '');
    const bytes = readFileSync(paths.plan), planHash = sha256(bytes);
    assert.deepEqual(JSON.parse(generated.stdout), {mode: 'release-declaration', execution: 'NOT_RUN',
      fixtures: 6, cells: 24, sha256: planHash});
    c.plan = JSON.parse(bytes);
    assert.deepEqual(Object.fromEntries(c.plan.readers.map(row => [row.role, row.artifact])), artifacts);
    assert.equal(new Set(c.plan.readers.map(row => row.artifact.source)).size, 4);
    assert.equal(new Set(c.plan.readers.map(row => row.artifact.image)).size, 4);
    assert.deepEqual(c.plan.cells, expectations);
    c.bindings.plan_sha256 = planHash;
    for (const binding of c.bindings.bindings) {
      const selected = c.plan.fixtures.find(row => row.id === `collection-${binding.collection}-${binding.policy}`);
      assert.deepEqual(selected.writer, artifacts[binding.writer === 'old' ? 'historical' : 'candidate']);
      binding.fixture = selected.id;
      binding.fixture_sha256 = selected.sha256;
    }
    // Synthetic receipts exercise the file contracts; the CLI has executed no images.
    const {raw: _raw, ...template} = c.evidence.cells[0];
    c.evidence.plan_sha256 = planHash;
    c.evidence.cells = c.plan.cells.map(cell => {
      const selected = c.plan.fixtures.find(row => row.id === cell.fixture);
      const observation = {...template, id: cell.id, reader: artifacts[cell.reader], writer: selected.writer,
        fixture_sha256: selected.sha256, policy: selected.policy, outcome: cell.expectation,
        checks: cell.expectation === 'accept' ? template.checks : {
          refused_before_traffic: true, refused_before_write: true, identity_preserved: true,
        }};
      const rawBytes = Buffer.from(JSON.stringify(observation)), path = cell.id + '.json';
      writeFileSync(join(root, path), rawBytes);
      return {...observation, raw: {path, sha256: sha256(rawBytes)}};
    });
    writeFileSync(paths.evidence, JSON.stringify(c.evidence));
    writeFileSync(paths.bindings, JSON.stringify(c.bindings));
    operation({root, c, paths, args, artifacts});
  });
}

test('explicit Python release CLI output binds old and candidate writers through the inventory CLI', () => {
  releaseFixture(({args}) => {
    const result = cli(args);
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stderr, '');
    assert.deepEqual(JSON.parse(result.stdout), {
      scope: 'DECLARED_INVENTORY_AND_EVIDENCE_CONSISTENCY_ONLY', collections: 2, shapes: 2,
      writer_policy_requirements: 6, fixtures: 6, cells: 24, accepted: 22, refused: 2,
    });
  });
});

test('release bindings cannot substitute a current or recovery reader for either writer', () => {
  for (const role of ['current', 'recovery']) {
    for (const writer of ['old', 'new']) releaseFixture(({args, c, paths, artifacts}) => {
      const binding = c.bindings.bindings.find(row => row.writer === writer);
      c.plan.fixtures.find(row => row.id === binding.fixture).writer = artifacts[role];
      const bytes = JSON.stringify(c.plan);
      c.bindings.plan_sha256 = sha256(bytes);
      writeFileSync(paths.plan, bytes);
      writeFileSync(paths.bindings, JSON.stringify(c.bindings));
      // Check the binding boundary before stale evidence can account for refusal.
      assert.throws(() => validateBindings(c.plan, sha256(bytes), c.inventory, args[5], c.source, args[3], c.bindings));
      rejected(args);
    });
  }
});

test('a passing smaller release campaign cannot omit an independently pinned collection', () => {
  releaseFixture(({args, c, paths}) => {
    const omitted = new Set(c.plan.fixtures.filter(row => row.id.startsWith('collection-beta-')).map(row => row.id));
    c.plan.fixtures = c.plan.fixtures.filter(row => !omitted.has(row.id));
    c.plan.cells = c.plan.cells.filter(row => !omitted.has(row.fixture));
    const retained = new Set(c.plan.cells.map(row => row.id));
    c.evidence.cells = c.evidence.cells.filter(row => retained.has(row.id));
    c.inventory.collections.pop();
    c.bindings.bindings = c.bindings.bindings.filter(row => row.collection === 'alpha');
    const bytes = JSON.stringify(c.plan);
    c.evidence.plan_sha256 = c.bindings.plan_sha256 = sha256(bytes);
    args[5] = c.bindings.inventory_sha256 = sha256(JSON.stringify(c.inventory));
    for (const name of ['plan', 'inventory', 'bindings', 'evidence']) {
      writeFileSync(paths[name], JSON.stringify(c[name]));
    }
    assert.deepEqual(verifyFiles(args[0], args[1], args[7], args[8]), {cells: 12, accepted: 11, refused: 1});
    const scoped = spawnSync(process.execPath, [join(directory, 'migration-matrix.mjs'),
      args[0], args[1], args[7], args[8]], {encoding: 'utf8', timeout: 10000});
    assert.ifError(scoped.error);
    assert.equal(scoped.status, 0, scoped.stderr);
    assert.match(scoped.stdout, /12 cells \(11 accepted, 1 refused\)/);
    rejected(args);
  });
});

test('independent source and shape pins cannot be replaced by matching producer files', () => {
  for (const index of [3, 5, 7, 8]) fixture(({args}) => {
    args[index] = index === 8 ? 'registry.example/test/api@sha256:' + sha256('other') : sha256('other');
    assert.throws(() => verifyInventoryFiles(...args));
    rejected(args);
  });
});

test('file inputs refuse malformed, empty, excessive, symlink and nonregular content', () => {
  for (const field of ['plan', 'source', 'inventory', 'bindings', 'evidence']) {
    for (const mode of ['malformed', 'empty', 'oversize', 'symlink', 'directory', 'fifo']) fixture(({args, paths, root}) => {
      const path = paths[field];
      if (mode === 'symlink') {
        rmSync(path); symlinkSync(join(root, 'source.json'), path);
      } else if (mode === 'directory' || mode === 'fifo') {
        rmSync(path);
        if (mode === 'directory') mkdirSync(path);
        else execFileSync('mkfifo', [path]);
      } else {
        writeFileSync(path, mode === 'malformed' ? '{untrusted private payload' :
          mode === 'empty' ? '' : 'x'.repeat(4 * 1024 * 1024 + 1));
      }
      rejected(args);
      assert.throws(() => verifyInventoryFiles(...args));
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

test('raw observations remain bounded regular files with exact bytes and contents', () => {
  for (const mode of ['directory', 'fifo', 'oversize', 'changed-bytes', 'changed-observation']) {
    fixture(({args, paths, c, root}) => {
      const row = c.evidence.cells[0], path = join(root, row.raw.path);
      if (mode === 'directory' || mode === 'fifo') {
        rmSync(path);
        if (mode === 'directory') mkdirSync(path);
        else execFileSync('mkfifo', [path]);
      } else {
        const bytes = mode === 'oversize' ? 'x'.repeat(4 * 1024 * 1024 + 1) :
          JSON.stringify({untrusted: 'private-input-marker'});
        writeFileSync(path, bytes);
        if (mode === 'changed-observation') {
          row.raw.sha256 = sha256(bytes);
          writeFileSync(paths.evidence, JSON.stringify(c.evidence));
        }
      }
      rejected(args);
      assert.throws(() => verifyInventoryFiles(...args));
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
