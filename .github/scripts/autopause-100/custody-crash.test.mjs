import test from 'node:test';
import assert from 'node:assert/strict';
import {fork} from 'node:child_process';
import {once} from 'node:events';
import {readFile, stat} from 'node:fs/promises';
import {join} from 'node:path';
import {readCustodyJournal} from './custody-reader.mjs';
import {CustodyJournal} from './custody-journal.mjs';
import {binding, temporary} from './fixtures/custody.mjs';

test('crash recovery observes original responsibility without rerunning effects or taking over', {timeout: 20000}, async () => {
  for (const stage of ['registered', 'ready', 'dispatched', 'effect_called', 'visible_ack', 'acknowledged']) {
    await temporary(async (directory, parent) => {
      const child = fork(new URL('./fixtures/custody-child.mjs', import.meta.url), [directory, stage],
        {stdio: ['ignore', 'ignore', 'pipe', 'ipc']});
      let diagnostics = '';
      child.stderr.on('data', chunk => {diagnostics += chunk;});
      const exit = once(child, 'exit');
      let timer;
      try {
        const observed = await Promise.race([
          once(child, 'message').then(([message]) => message),
          exit.then(() => {throw Error('child exited before crash point: ' + diagnostics);}),
          new Promise((_, reject) => {timer = setTimeout(() => reject(Error('crash point timeout')), 5000);}),
        ]);
        clearTimeout(timer);
        assert.equal(observed.stage, stage);
        child.kill('SIGKILL'); await exit;
        const before = await stat(join(directory, 'custody.jsonl'));
        const view = await readCustodyJournal(directory, binding);
        assert.deepEqual(view.unresolved_effect_ids, ['effect_fixture']);
        assert.equal(view.mutations_authorized, false); assert.equal(view.cleanup, 'RETAIN_CUSTODY');
        assert.equal(view.complete, false); assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
        if (stage === 'registered') assert.equal(view.status, 'QUARANTINED');
        if (['dispatched', 'effect_called'].includes(stage)) assert.equal(view.effects[0].plan.state, 'OUTCOME_UNKNOWN');
        await assert.rejects(CustodyJournal.create(directory, binding), /custody_create_failed/);
        await readCustodyJournal(directory, binding);
        const after = await stat(join(directory, 'custody.jsonl'));
        assert.equal(after.size, before.size); assert.equal(after.mtimeMs, before.mtimeMs);
        const expectedCalls = ['effect_called', 'visible_ack', 'acknowledged'].includes(stage) ? 1 : 0;
        let calls = 0;
        try {calls = (await readFile(join(parent, 'synthetic-counter'), 'utf8')).trim().split('\n').length;}
        catch (error) {if (error.code !== 'ENOENT') throw error;}
        assert.equal(calls, expectedCalls);
      } finally {
        clearTimeout(timer);
        if (child.exitCode === null && child.signalCode === null) {child.kill('SIGKILL'); await exit;}
      }
    });
  }
});
