import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export function registerRegistryRunnerTests({fixture, requiredPrefixes}) {
  for (const reuse of [false, true]) {
    const mode = reuse ? 'archive' : 'source';
    test(`${mode}: adding one suite file extends the runner without shared-file edits`, () => {
      const f = fixture('success', reuse);
      try {
        const prefix = 'new_independent_suite::mongo_tests::';
        writeFileSync(join(f.directory, 'repo/.github/scripts/mongo-contract-suites/new-suite.json'),
          JSON.stringify({prefix}));
        execFileSync('bash', [...f.args, '--list'], {env: f.env, timeout: 10000, stdio: 'pipe'});
        const calls = f.calls();
        assert.equal(calls.length, 1);
        assert.equal(calls[0].command, 'cargo');
        const filter = calls[0].args[calls[0].args.indexOf('-E') + 1];
        assert.ok(filter.endsWith(`(${[...requiredPrefixes, prefix].sort().map(value => `test(${value})`).join(' | ')})`));
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
      } finally { f.close(); }
    });

    test(`${mode}: invalid registrations fail before Cargo or fixture allocation`, () => {
      for (const entry of [{prefix: requiredPrefixes[0]}, {prefix: 'bad) or all()'}]) {
        const f = fixture('success', reuse);
        try {
          writeFileSync(join(f.directory, 'repo/.github/scripts/mongo-contract-suites/invalid.json'),
            JSON.stringify(entry));
          assert.throws(() => execFileSync('bash', [...f.args, 'count:1/1'], {
            env: f.env, timeout: 10000, stdio: 'pipe',
          }), error => error.status === 1 && /Mongo suite registry failed:/.test(error.stderr));
          assert.equal(existsSync(f.log), false);
          assert.equal(f.containers().length, 0);
          assert.equal(f.volumes().length, 0);
        } finally { f.close(); }
      }
    });
  }
}
