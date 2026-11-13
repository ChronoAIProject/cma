import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = fileURLToPath(new URL('../../', import.meta.url));
const workflow = readFileSync(join(root, '.github/workflows/ci.yml'), 'utf8');
const body = workflow.split('      - name: infra/ contains only the fixed file list\n        run: |\n')[1]
  .split('      - name:')[0].split('\n').map(line => line.replace(/^          /, '')).join('\n');
const tracked = spawnSync('git', ['ls-files', 'infra'], { cwd: root, encoding: 'utf8' });
assert.equal(tracked.status, 0);
const files = tracked.stdout.trim().split('\n');
const check = cwd => spawnSync('/bin/bash', ['-e', '-c', body], { cwd, encoding: 'utf8' });

test('actual CI inventory agrees with tracked tree and documented inventory', () => {
  const result = check(root);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  const documented = readFileSync(join(root, 'CLAUDE.md'), 'utf8')
    .split('`infra/` contains these files and only these files:')[1].split('```')[1]
    .trim().split('\n').sort();
  assert.deepEqual(files.toSorted(), documented);
});

test('the unchanged CI check rejects both extra and missing files', () => {
  const temporary = mkdtempSync(join(tmpdir(), 'cma-inventory-'));
  try {
    for (const file of files) {
      mkdirSync(dirname(join(temporary, file)), { recursive: true });
      writeFileSync(join(temporary, file), '');
    }
    assert.equal(check(temporary).status, 0);
    const extra = join(temporary, 'infra/api/unreviewed.json');
    writeFileSync(extra, '{}');
    assert.equal(check(temporary).status, 1);
    rmSync(extra);
    rmSync(join(temporary, 'infra/api/configmap.yaml'));
    assert.equal(check(temporary).status, 1);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});
