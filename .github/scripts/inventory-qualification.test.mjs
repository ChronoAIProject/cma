import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('qualification evidence rejects failed diagnostics, incomplete cohorts and identity drift', () => {
  execFileSync('python3', ['-B', fileURLToPath(new URL('inventory_qualification_tests.py', import.meta.url))], { stdio: 'pipe' });
});

export function registerQualificationTests({ fixture }) {
  for (const scenario of ['success', 'observation-error', 'cleanup-error']) {
    test(`qualification uses release, zero retries and owned cleanup: ${scenario}`, () => {
      const f = fixture(scenario);
      try {
        const evidence = join(f.directory, 'evidence'); mkdirSync(evidence);
        writeFileSync(join(evidence, 'build.json'), '{}');
        const env = { ...f.env, CMA_INVENTORY_QUALIFICATION_DIR: evidence };
        const run = () => execFileSync('bash', [...f.args, '--observation-budget'], { env, timeout: 15000, stdio: 'pipe' });
        if (scenario === 'success') run();
        else assert.throws(run, error => error.status === (scenario === 'cleanup-error' ? 125 : 7));
        const calls = f.calls().filter(row => row.command === 'cargo');
        assert.equal(calls.length, 1);
        assert.deepEqual(calls[0].args.slice(0, 8), ['nextest', 'run', '--locked', '--release', '-p', 'cma', '--bin', 'cma']);
        const args = calls[0].args;
        assert.equal(args[args.indexOf('--retries') + 1], '0');
        assert.equal(args[args.indexOf('--no-tests') + 1], 'fail');
        assert.ok(args.includes('--no-capture') && args.includes('--no-fail-fast'));
        const receipt = JSON.parse(readFileSync(join(evidence, 'fixture.json')));
        assert.equal(receipt.first_test_exit, scenario === 'observation-error' ? 7 : 0);
        assert.equal(receipt.owned_cleanup, scenario !== 'cleanup-error');
        assert.equal(receipt.samples.length, 2);
        if (scenario !== 'cleanup-error') {
          assert.deepEqual(f.containers(), []);
          assert.deepEqual(f.volumes(), []);
        }
      } finally { f.close(); }
    });
  }
  test('ordinary archives and missing build evidence cannot enter qualification fixtures', () => {
    for (const reuse of [true, false]) {
      const f = fixture('success', reuse);
      try {
        const evidence = join(f.directory, 'evidence'); mkdirSync(evidence);
        if (reuse) writeFileSync(join(evidence, 'build.json'), '{}');
        assert.throws(() => execFileSync('bash', [...f.args, '--observation-budget'], {
          env: { ...f.env, CMA_INVENTORY_QUALIFICATION_DIR: evidence }, timeout: 15000, stdio: 'pipe',
        }), error => error.status === 2);
        assert.equal(existsSync(f.log), false);
      } finally { f.close(); }
    }
  });
  test('qualification listing uses the same release selector before starting Mongo', () => {
    const f = fixture('success');
    try {
      const evidence = join(f.directory, 'evidence'); mkdirSync(evidence);
      writeFileSync(join(evidence, 'build.json'), '{}');
      execFileSync('bash', [...f.args, '--list-qualification', '--observation-budget'], {
        env: { ...f.env, CMA_INVENTORY_QUALIFICATION_DIR: evidence }, timeout: 15000, stdio: 'pipe',
      });
      assert.ok(f.calls().every(row => row.command === 'cargo'));
      assert.equal(f.calls()[0].args[1], 'list');
      assert.ok(f.calls()[0].args.includes('--release'));
    } finally { f.close(); }
  });
}
