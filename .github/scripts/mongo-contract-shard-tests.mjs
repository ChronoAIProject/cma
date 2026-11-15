import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';

export function registerShardTests(fixture) {
  for (const reuse of [false, true]) {
    const mode = reuse ? 'archive' : 'source';
    test(`${mode}: two Mongo shards cover every test once and preserve failures and fixture limits`, () => {
      for (const scenario of ['batches', 'batches-error']) {
        const selected = [];
        for (const shard of [1, 2]) {
          const f = fixture(scenario, reuse);
          try {
            f.env.CMA_MONGO_SHARD = `${shard}/2`;
            const run = () => execFileSync('bash', f.args, {env: f.env, timeout: 30000, stdio: 'pipe'});
            if (scenario === 'batches-error' && shard === 1) assert.throws(run, error => error.status === 1);
            else run();
            const isolated = shard === 1 ? [0, -2, -4] : [-1, -3, -5];
            const ordinary = shard === 1 ? [1, 3, 5] : [2, 4];
            const calls = f.calls();
            assert.equal(calls.filter(row => row.command === 'cargo' && row.args[1] === 'list').length, 1);
            const cargo = calls.filter(row => row.command === 'cargo' && row.args[1] === 'run');
            assert.equal(cargo.length, isolated.length + ordinary.length);
            assert.ok(cargo.slice(0, isolated.length).every(row => !row.args.includes('--partition')));
            assert.deepEqual(cargo.slice(isolated.length).map(row => row.args[row.args.indexOf('--partition') + 1]).sort(),
              ordinary.map(index => `count:${index}/5`));
            for (const index of [...isolated, ...ordinary]) {
              assert.ok(existsSync(`${f.log}.finished-${index}`), 'all assigned contracts finish even after a test failure');
              const cases = JSON.parse(readFileSync(`${f.log}.selected-${index}`));
              assert.ok(cases.length > 0 && cases.length <= 40);
              selected.push(...cases);
            }
            let active = 0;
            let maximum = 0;
            let starts = 0;
            for (const row of calls) {
              if (row.command === 'docker' && row.args[0] === 'run') {
                if (starts <= isolated.length) assert.equal(active, 0, 'isolated fixtures finish cleanup before the next phase');
                starts++;
                maximum = Math.max(maximum, ++active);
              }
              if (row.command === 'docker' && row.args[0] === 'volume' && row.args[1] === 'rm') --active;
              assert.ok(active >= 0 && active <= 2, 'each runner keeps at most two fixtures');
            }
            assert.equal(maximum, 2);
            assert.equal(active, 0);
            assert.equal(f.containers().length, 0);
            assert.equal(f.volumes().length, 0);
            assert.ok(existsSync(f.foreignVolume));
          } finally { f.close(); }
        }
        assert.equal(selected.length, 167);
        assert.equal(new Set(selected).size, 167, 'the two runners cover the entire corpus exactly once');
      }
    });

    test(`${mode}: invalid Mongo shards fail before allocating fixtures or enumerating tests`, () => {
      const f = fixture('batches', reuse);
      try {
        for (const shard of ['', '0/2', '2/1', '3/2', '1/3', '01/2', '1/2\n', '1/2 extra']) {
          assert.throws(() => execFileSync('bash', f.args, {
            env: {...f.env, CMA_MONGO_SHARD: shard}, timeout: 5000, stdio: 'pipe',
          }), error => error.status === 1);
        }
        assert.equal(existsSync(f.log), false);
      } finally { f.close(); }
    });

    test(`${mode}: Mongo shards reject incomplete selection and stop after uncertain isolated cleanup`, () => {
      for (const [shard, scenario] of [[1, 'list-omitted-module'], [2, 'list-no-scale'], [1, 'cleanup-error'], [2, 'owner-cleanup-error']]) {
        const f = fixture(scenario, reuse);
        try {
          assert.throws(() => execFileSync('bash', f.args, {
            env: {...f.env, CMA_MONGO_SHARD: `${shard}/2`}, timeout: 15000, stdio: 'pipe',
          }), error => error.status === 1);
          const cargo = f.calls().filter(row => row.command === 'cargo' && row.args[1] === 'run');
          assert.equal(cargo.length, scenario.startsWith('list-') ? 0 : 1);
          assert.ok(cargo.every(row => !row.args.includes('--partition')));
          assert.ok(existsSync(f.foreignVolume));
        } finally { f.close(); }
      }
    });
  }
}
