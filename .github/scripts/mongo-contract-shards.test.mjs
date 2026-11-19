import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

export function registerShardTests({fixture, ...isolated}) {
  test('archive: nine CI lanes execute the complete inventory exactly once', () => {
    const selected = [];
    const partitions = [];
    for (const shard of ['isolated', ...Array.from({length: 8}, (_, i) => `${i + 1}/8`)]) {
      const f = fixture('shard-success', true);
      try {
        execFileSync('bash', [...f.args, '--shard', shard], {env: f.env, timeout: 30000, stdio: 'pipe'});
        const runs = f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'run');
        assert.equal(f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'list').length, 1);
        for (const row of runs) {
          assert.ok(row.args.includes('--binaries-metadata'));
          assert.equal(row.args[row.args.indexOf('--test-threads') + 1], '1');
          assert.ok(row.args.includes('--no-fail-fast'));
          if (shard === 'isolated') assert.ok(!row.args.includes('--partition'));
          else partitions.push(row.args[row.args.indexOf('--partition') + 1]);
        }
        for (const file of readdirSync(f.directory).filter(name => name.startsWith('calls.ndjson.selected-'))) {
          selected.push(...JSON.parse(readFileSync(join(f.directory, file))));
        }
        assert.equal(runs.length, f.calls().filter(row => row.command === 'docker' && row.args[0] === 'run').length);
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
      } finally { f.close(); }
    }
    assert.deepEqual(partitions.sort(), Array.from({length: 9}, (_, i) => `count:${i + 1}/9`).sort());
    assert.equal(selected.length, 326);
    assert.equal(new Set(selected).size, 326);
    for (const name of Object.values(isolated)) assert.ok(selected.includes(name));
  });

  test('archive: a failed shard still completes its remaining partitions and cleans up', () => {
    const f = fixture('shard-error', true);
    try {
      assert.throws(() => execFileSync('bash', [...f.args, '--shard', '1/8'], {env: f.env, timeout: 30000, stdio: 'pipe'}), error => error.status === 1);
      assert.deepEqual(f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'run')
        .map(row => row.args[row.args.indexOf('--partition') + 1]).sort(), ['count:1/9', 'count:9/9']);
      assert.equal(f.containers().length, 0);
      assert.equal(f.volumes().length, 0);
    } finally { f.close(); }
  });

  test('archive: invalid shard coordinates refuse before allocating fixtures', () => {
    for (const shard of ['0/8', '9/8', '1/7', '01/8', 'all', '']) {
      const f = fixture('shard-success', true);
      try {
        assert.throws(() => execFileSync('bash', [...f.args, '--shard', shard], {env: f.env, timeout: 5000, stdio: 'pipe'}));
        assert.equal(f.containers().length, 0);
        assert.equal(f.volumes().length, 0);
      } finally { f.close(); }
    }
  });

  test('readiness: transient ping and primary failures retry before contracts run', () => {
    const f = fixture('readiness-retry', true);
    try {
      execFileSync('bash', [...f.args, '--scale'], {env: f.env, timeout: 10000, stdio: 'pipe'});
      const calls = f.calls();
      const probes = calls.filter(row => row.command === 'docker' && row.args[0] === 'exec');
      assert.equal(probes.filter(row => row.args.at(-1).includes('ping')).length, 2);
      assert.equal(probes.filter(row => row.args.at(-1).includes('isWritablePrimary')).length, 2);
      const execution = calls.findIndex(row => row.command === 'cargo' && row.args[1] === 'run');
      assert.ok(execution > calls.indexOf(probes.at(-1)));
      assert.equal(f.containers().length, 0);
      assert.equal(f.volumes().length, 0);
    } finally { f.close(); }
  });
}
