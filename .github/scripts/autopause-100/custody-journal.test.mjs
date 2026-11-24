import test from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import {join} from 'node:path';
import {CustodyJournal} from './custody-journal.mjs';
import {readCustodyJournal} from './custody-reader.mjs';
import {binding, deferred, fixture, observedIO, temporary} from './fixtures/custody.mjs';

test('intent bytes and dispatch are synced before one callback; acknowledgement commits separately', async () => {
  await temporary(async directory => {
    const trace = []; const value = fixture(); let calls = 0;
    const writer = await CustodyJournal.create(directory, binding, {io: observedIO(point => {trace.push(point);})});
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      assert.equal(ready.sequence, 2); assert.equal(ready.effect_id, value.intent.effect_id);
      const result = await writer.dispatch(value.intent.effect_id, ready, async dispatch => {
        calls++;
        assert.equal(trace.at(-1), 'after:sync:directory:dispatch_recorded');
        assert.ok(trace.includes('after:sync:content:body'));
        assert.ok(trace.includes('after:sync:content:key'));
        assert.equal(dispatch.sequence, 3);
        return value.ack;
      });
      assert.equal(calls, 1); assert.equal(result.outcome.sequence, 4);
      assert.notEqual(result.outcome.tail_sha256, result.dispatch.tail_sha256);
      assert.equal(trace.at(-1), 'after:sync:directory:acknowledged');
      assert.deepEqual(await fs.readFile(join(directory, 'effect_fixture.body')), value.content.body);
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.records, 5); assert.equal(view.tail_sha256, result.outcome.tail_sha256);
      assert.equal(view.effects[0].plan.state, 'ACKNOWLEDGED');
      assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
      assert.equal(view.mutations_authorized, false); assert.equal(view.cleanup, 'RETAIN_CUSTODY');
      assert.equal(view.complete, false); assert.equal(view.qualification, 'NOT_ASSESSED');
      assert.equal(JSON.stringify(view).includes('synthetic fixture'), false);
      assert.equal(JSON.stringify(view).includes('fixture-operation-key'), false);
      assert.throws(() => {result.outcome.sequence = 5;}, TypeError);
      assert.throws(() => new CustodyJournal({}, {}), /custody_factory_required/);
    } finally {await writer.close(); await writer.close();}
    await assert.rejects(writer.recordIntent(value.intent, value.content), /custody_writer_unavailable/);
  });
});

test('create/content/intent/dispatch sync failures publish no execution eligibility', async () => {
  const points = ['before:mkdir', 'before:sync:directory:create', 'before:sync:journal:created',
    'before:sync:directory:created', 'before:write:journal:intent', 'before:sync:journal:intent',
    'before:sync:directory:intent', 'before:write:content:body', 'before:sync:content:body',
    'before:sync:directory:content_body', 'before:sync:content:key', 'before:sync:directory:content_key',
    'before:sync:journal:ready', 'before:sync:directory:ready',
    'before:write:journal:dispatch_recorded', 'before:sync:journal:dispatch_recorded',
    'before:sync:directory:dispatch_recorded'];
  for (const point of points) await temporary(async directory => {
    let writer; let calls = 0; let hit = false; const value = fixture();
    const io = observedIO(current => {if (current === point) {hit = true; throw Error('synthetic-private-canary');}});
    try {
      await assert.rejects(async () => {
        writer = await CustodyJournal.create(directory, binding, {io});
        const receipt = await writer.recordIntent(value.intent, value.content);
        await writer.dispatch(value.intent.effect_id, receipt, async () => {calls++; return value.ack;});
      }, error => /^custody_(create|write)_failed$/.test(error.message));
      assert.equal(hit, true, point); assert.equal(calls, 0, point);
      if (writer) await assert.rejects(writer.recordIntent(value.intent, value.content), /custody_writer_unavailable/);
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.cleanup, 'RETAIN_CUSTODY');
      assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
      assert.equal(JSON.stringify(view).includes('synthetic-private-canary'), false);
    } finally {if (writer) await writer.close();}
  });
});

test('post-effect ack write/sync failures preserve dispatch and permanently poison the writer', async () => {
  for (const point of ['before:write:journal:acknowledged', 'before:sync:journal:acknowledged',
    'before:sync:directory:acknowledged']) await temporary(async directory => {
    const value = fixture(); let calls = 0; let dispatch;
    const writer = await CustodyJournal.create(directory, binding,
      {io: observedIO(current => {if (current === point) throw Error('private-result-canary');})});
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, async receipt => {
        calls++; dispatch = receipt; return value.ack;
      }), /custody_write_failed/);
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, () => {calls++;}), /custody_writer_unavailable/);
      await assert.rejects(writer.recordFact(value.intent.effect_id, dispatch, value.ack), /custody_writer_unavailable/);
      assert.equal(calls, 1);
      const view = await readCustodyJournal(directory, binding);
      assert.deepEqual(view.unresolved_effect_ids, [value.intent.effect_id]);
      assert.ok(view.records >= 4); assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
      assert.equal(view.cleanup, 'RETAIN_CUSTODY');
      if (point.startsWith('before:write:')) assert.equal(view.effects[0].plan.state, 'OUTCOME_UNKNOWN');
    } finally {await writer.close();}
  });
});

test('concurrent append and close refuse while a live reader cannot promote an unsynced ACK', async () => {
  await temporary(async directory => {
    const reached = deferred(); const release = deferred(); const value = fixture();
    const writer = await CustodyJournal.create(directory, binding, {io: observedIO(async point => {
      if (point === 'before:sync:journal:acknowledged') {reached.resolve(); await release.promise;}
    })});
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      let resolved = false;
      const running = writer.dispatch(value.intent.effect_id, ready, async () => value.ack)
        .then(result => {resolved = true; return result;});
      try {
        await reached.promise;
        assert.equal(resolved, false);
        await assert.rejects(writer.recordIntent(value.intent, value.content), /custody_writer_unavailable/);
        await assert.rejects(writer.close(), /custody_writer_busy/);
        const view = await readCustodyJournal(directory, binding);
        assert.equal(view.effects[0].plan.state, 'ACKNOWLEDGED');
        assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
        assert.equal(view.complete, false); assert.equal(view.mutations_authorized, false);
        assert.equal(view.cleanup, 'RETAIN_CUSTODY');
      } finally {release.resolve(); await running;}
      assert.equal(resolved, true);
    } finally {await writer.close();}
  });
});

test('exclusive creation and opaque writer receipts reject forged, cross-effect and repeated dispatch', async () => {
  for (const mode of ['clone', 'cross_effect', 'repeated']) await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding); let calls = 0;
    try {
      await assert.rejects(CustodyJournal.create(directory, binding), /custody_create_failed/);
      const first = fixture('first'); const second = fixture('second');
      const ready = await writer.recordIntent(first.intent, first.content);
      const other = await writer.recordIntent(second.intent, second.content);
      if (mode === 'repeated') await writer.dispatch('first', ready, async () => {calls++; return first.ack;});
      const receipt = mode === 'clone' ? {...ready} : mode === 'cross_effect' ? other : ready;
      await assert.rejects(writer.dispatch('first', receipt, async () => {calls++; return first.ack;}), /custody_write_failed/);
      assert.equal(calls, mode === 'repeated' ? 1 : 0);
    } finally {await writer.close();}
  });
});

test('unknown effects stop new dispatch; late duplicate facts preserve identity and conflicts quarantine', async () => {
  await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding); const value = fixture();
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      const result = await writer.dispatch(value.intent.effect_id, ready,
        async () => ({type: 'unavailable', reason: 'timeout'}));
      await writer.recordFact(value.intent.effect_id, result.dispatch, value.ack);
      await writer.recordFact(value.intent.effect_id, result.dispatch, value.ack);
      await writer.recordFact(value.intent.effect_id, result.dispatch,
        {...value.ack, receipt: {...value.ack.receipt, command_id: 'conflicting'}});
      await assert.rejects(writer.recordFact(value.intent.effect_id, result.dispatch, value.ack), /custody_writer_unavailable/);
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.effects[0].plan.state, 'QUARANTINED');
      assert.deepEqual(view.unresolved_effect_ids, [value.intent.effect_id]);
    } finally {await writer.close();}
  });
  await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding); const first = fixture('first'); const next = fixture('next');
    try {
      const ready = await writer.recordIntent(first.intent, first.content);
      const pending = await writer.recordIntent(next.intent, next.content);
      await writer.dispatch('first', ready, async () => ({type: 'unavailable', reason: 'receipt_missing'}));
      let calls = 0;
      await assert.rejects(writer.dispatch('next', pending, async () => {calls++; return next.ack;}), /custody_write_failed/);
      assert.equal(calls, 0);
    } finally {await writer.close();}
  });
});

test('thrown or malformed callback outcomes cannot be resent or leak their contents', async () => {
  for (const malformed of [false, true]) await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding); const value = fixture(); let calls = 0;
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, async () => {
        calls++;
        if (malformed) return {type: 'unknown', private: 'private-result-canary'};
        throw Error('private-result-canary');
      }), {message: 'custody_write_failed'});
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.effects[0].plan.state, 'OUTCOME_UNKNOWN');
      assert.equal(JSON.stringify(view).includes('private-result-canary'), false);
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, () => {calls++;}), /custody_writer_unavailable/);
      assert.equal(calls, 1);
    } finally {await writer.close();}
  });
});
