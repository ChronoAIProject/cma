import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('Mongo runner fails before cleanup scope on entropy errors and proves exact disposable cleanup', () => {
  for (const scenario of ['entropy-error', 'entropy-malformed', 'success', 'test-error', 'cleanup-error']) {
    const directory = mkdtempSync(join(tmpdir(), 'cma-mongo-runner-test-'));
    const log = join(directory, 'calls.ndjson');
    const cli = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const scenario = process.env.CMA_RUNNER_SCENARIO;
const log = process.env.CMA_RUNNER_LOG;
if (command === 'openssl') {
  if (scenario === 'entropy-error') process.exit(1);
  console.log(scenario === 'entropy-malformed' ? 'bad-suffix' : 'a'.repeat(24));
  process.exit(0);
}
fs.appendFileSync(log, JSON.stringify({command,args,uri:process.env.CHRONO_TEST_MONGODB_URI})+'\\n');
if (command === 'cargo') process.exit(scenario === 'test-error' ? 7 : 0);
if (args[0] === 'run') { fs.writeFileSync(log+'.container','present'); console.log('b'.repeat(64)); }
else if (args[0] === 'exec') console.log('1');
else if (args[0] === 'port') console.log('127.0.0.1:32777');
else if (args[0] === 'ps') console.log(fs.existsSync(log+'.container') ? 'b'.repeat(12) : '');
else if (args[0] === 'rm') {
  if (scenario === 'cleanup-error') process.exit(1);
  fs.unlinkSync(log+'.container');
} else process.exit(9);
`;
    for (const name of ['openssl', 'docker', 'cargo']) writeFileSync(join(directory, name), cli, { mode: 0o755 });
    const run = () => execFileSync('bash', [new URL('./mongo-contract-tests.sh', import.meta.url).pathname], {
      env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, CMA_RUNNER_SCENARIO: scenario, CMA_RUNNER_LOG: log, CHRONO_TEST_MONGODB_URI: 'must-not-use-caller-uri', CHRONO_TEST_MONGODB_FAILPOINT_URI: 'must-not-use-caller-uri' },
      timeout: 10000, stdio: 'pipe',
    });
    try {
      if (scenario === 'success') run(); else assert.throws(run);
      if (scenario.startsWith('entropy-')) { assert.equal(existsSync(log), false); continue; }
      const calls = readFileSync(log, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
      const cargo = calls.find((row) => row.command === 'cargo');
      assert.equal(cargo.uri, 'mongodb://127.0.0.1:32777/?directConnection=true&replicaSet=cma_ci');
      assert.ok(cargo.args.includes('--run-ignored') && cargo.args.includes('only'));
      assert.equal(cargo.args[cargo.args.indexOf('--test-threads') + 1], '1');
      assert.ok(calls.filter((row) => row.args[0] === 'ps').every((row) => row.args.includes(`label=dev.chrono.cma.mongo-contract=cma-mongo-ci-${'a'.repeat(24)}`)));
      assert.deepEqual(calls.find((row) => row.args[0] === 'rm').args, ['rm', '--force', 'b'.repeat(12)]);
      assert.equal(existsSync(`${log}.container`), scenario === 'cleanup-error');
    } finally { rmSync(directory, { recursive: true }); }
  }
});
