import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const callers = [
  ['mongo-contract-selection-tests.mjs', 'backend-selection-proof.py'],
  ['mongo-contract-completeness.test.mjs', 'mongo-contract-completeness.py'],
];

function copyCallers(root) {
  const scripts = join(root, '.github/scripts');
  mkdirSync(scripts, { recursive: true });
  for (const name of [
    ...callers.flat(), 'mongo-contract-test-fixture.mjs', 'mongo-contract-tests.sh',
    'mongo-contract-registry.py', 'mongo-contract-test-prefixes.mjs',
    'mongo-contract-exceptions.json', 'mongo-contract-timings.json', 'desktop-replica-contract.py',
    'image-manager-mongo-contract.sh',
  ]) cpSync(new URL(name, import.meta.url), join(scripts, name));
  cpSync(new URL('mongo-contract-suites', import.meta.url), join(scripts, 'mongo-contract-suites'), { recursive: true });
  mkdirSync(join(root, 'api/.config'), { recursive: true });
  cpSync(new URL('../../api/.config/nextest.toml', import.meta.url), join(root, 'api/.config/nextest.toml'));
  const entry = join(scripts, 'python-callers.test.mjs');
  writeFileSync(entry, `
import * as inputs from './mongo-contract-test-fixture.mjs';
import { registerSelectionTests } from './mongo-contract-selection-tests.mjs';
import './mongo-contract-completeness.test.mjs';
registerSelectionTests(inputs);
`);
  return { scripts, entry };
}

function execute(entry) {
  const env = { ...process.env, PYTHONDONTWRITEBYTECODE: '1' };
  delete env.NODE_TEST_CONTEXT;
  const result = spawnSync(process.execPath, ['--test', '--test-reporter=tap', entry], {
    encoding: 'utf8', timeout: 60_000,
    env,
  });
  assert.equal(result.error, undefined);
  assert.equal(result.signal, null);
  return { ...result, output: result.stdout + result.stderr };
}

function passingTests(result) {
  assert.equal(result.status, 0, result.output);
  assert.match(result.output, /# fail 0\b/);
  const names = [...result.stdout.matchAll(/^ok \d+ - (.+)$/gm)].map(match => match[1]);
  assert.ok(names.some(name => name.includes('selection proof moves only history')));
  assert.ok(names.some(name => name.includes('metadata completeness accepts selected contracts')));
  return names;
}

test('real Python selection and completeness callers preserve results under spaces and reject encoded paths', () => {
  const temporary = mkdtempSync(join(tmpdir(), 'cma-mongo-path-'));
  const originals = callers.map(([name]) => readFileSync(new URL(name, import.meta.url), 'utf8'));
  try {
    const normal = copyCallers(join(temporary, 'normal'));
    const spaced = copyCallers(join(temporary, 'with spaces'));
    assert.deepEqual(passingTests(execute(spaced.entry)), passingTests(execute(normal.entry)));
    for (const [index, [caller, helper]] of callers.entries()) {
      const path = join(spaced.scripts, caller);
      const original = originals[index];
      const boundary = `fileURLToPath(new URL('${helper}', import.meta.url))`;
      assert.ok(original.includes(boundary), `${caller} retains its real Python boundary`);
      writeFileSync(path, original.replaceAll(boundary, `new URL('${helper}', import.meta.url).pathname`));
      try {
        const result = execute(spaced.entry);
        assert.notEqual(result.status, 0, result.output);
        assert.match(result.output, /FileNotFoundError: \[Errno 2\]/);
        assert.ok(result.output.includes(`with%20spaces/.github/scripts/${helper}`), result.output);
      } finally {
        writeFileSync(path, original);
      }
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
  callers.forEach(([name], index) => assert.equal(readFileSync(new URL(name, import.meta.url), 'utf8'), originals[index]));
});
