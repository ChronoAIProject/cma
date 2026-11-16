import test from 'node:test';
import assert from 'node:assert/strict';
import {admissionStop, admissionResume} from './admission-write.mjs';

// A stateful HTTP contract model exercises the real case and cleanup callbacks.
// It does not establish live AM, runtime or browser acceptance.
function fixture({stopBeforeThird = false, loseResume = 0, applied = false, malformed = false} = {}) {
  let revision = 0, stop = null, history = [], calls = 0, workSequence = 0;
  let armed = false, withheld = false, resumedAttempts = 0, sandboxCount = 0;
  const cleanups = [], requests = [];
  const snapshot = () => ({revision, operator_stop: structuredClone(stop),
    last_change: structuredClone(history[0] ?? null), history: structuredClone(history.slice(0, 20)),
    writes_available: true});
  const control = () => {const value = snapshot(); delete value.history; return value;};
  const change = (stopped, reason) => {
    const old = structuredClone(stop); revision++;
    stop = stopped ? {since: revision, revision, actor: 'fixture-admin', reason} : null;
    history.unshift({revision, at: revision, actor: 'fixture-admin', reason,
      old_stop: old, new_stop: structuredClone(stop)});
  };
  const request = async input => {
    requests.push(structuredClone(input));
    const {method, path, body} = input;
    if (method === 'GET') return {status: 200, body: snapshot()};
    if (path === '/api/v1/sandboxes') return stop
      ? {status: 503, body: {code: 'admission_paused'}}
      : {status: 201, body: {id: 'fixture-sandbox-' + ++sandboxCount}};
    if (path.endsWith('/stop') && stop) return {status: 200, body: control()};
    if (body.expected_revision !== revision) return {status: 409, body: {code: 'revision_conflict'}};
    if (path.endsWith('/stop')) {change(true, body.reason); return {status: 200, body: control()};}
    assert.ok(path.endsWith('/resume')); resumedAttempts++;
    if (resumedAttempts === loseResume) {
      if (applied) change(false, body.reason);
      if (malformed) return {status: 200, body: {operator_stop: null, revision: revision + 10}};
      throw Error('lost_resume_ack');
    }
    change(false, body.reason);
    if (armed) {withheld = true; throw Error('withheld_resume_ack');}
    return {status: 200, body: control()};
  };
  const setup = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveAdmission: true,
    cleanup: async () => {}, sampleExistingWork: async () => ({work_id: 'fixture-work',
      running: true, sequence: ++workSequence, restarts: 0}),
    custody: async () => ({cleanup: async () => {}}),
    freshCreate: {take: async () => ({key: 'fixture-create-' + ++calls, body: {name: 'fixture'}})},
    faults: {withholdNextSuccessfulResponse: async () => {
      armed = true;
      return {cleanup: async () => {armed = false;}, receipt: async () => ({forwarded_writes: 1,
        response_withheld: withheld, following_requests: [{method: 'GET'}]})};
    }}};
  const ctx = {am: 'a', phase: 'before_restart', iteration: 1,
    fixture: () => ({take: async () => setup}), request,
    defer: (label, callback) => cleanups.push({label, callback}),
    observe: async (_label, callback) => callback(),
    assert: (value, code) => assert.ok(value, code),
    step: async (name, callback) => {
      if (stopBeforeThird && name === 'step_3') throw Error('fixture_step_failed');
      await callback();
    }};
  return {ctx, requests, cleanups, snapshot, get resumedAttempts() {return resumedAttempts;},
    restore: () => cleanups.find(entry => entry.label === 'admin_admission_restore').callback(),
    async cleanup() {
      const errors = [];
      for (const {callback} of cleanups.toReversed()) {
        try {await callback();} catch (error) {errors.push(error);}
      }
      return errors;
    },
    foreignStop() {stop = {...stop, reason: 'another operator'};},
  };
}

for (const applied of [false, true]) {
  test('lost restore acknowledgement stays failed with exactly one resume: applied=' + applied, async () => {
    const f = fixture({stopBeforeThird: true, loseResume: 1, applied});
    await assert.rejects(admissionStop.run(f.ctx), /fixture_step_failed/);
    await assert.rejects(f.restore(), /lost_resume_ack/);
    await assert.rejects(f.restore(), /lost_resume_ack/);
    const errors = await f.cleanup();
    assert.ok(errors.some(error => /lost_resume_ack/.test(error.message)));
    assert.equal(f.resumedAttempts, 1);
    assert.equal(f.snapshot().operator_stop === null, applied);
  });
}

for (const definition of [admissionStop, admissionResume]) {
  test(definition.id + ' never compensates by replaying an uncertain case resume', async () => {
    const f = fixture({loseResume: 1});
    await assert.rejects(definition.run(f.ctx), /lost_resume_ack/);
    const errors = await f.cleanup();
    assert.equal(f.resumedAttempts, 1);
    assert.notEqual(f.snapshot().operator_stop, null);
    assert.ok(errors.some(error => /admin_admission_resume_outcome_unknown/.test(error.message)));
  });

  test(definition.id + ' accepts readback of a committed uncertain resume without another dispatch', async () => {
    const f = fixture({loseResume: 1, applied: true});
    await assert.rejects(definition.run(f.ctx), /lost_resume_ack/);
    assert.deepEqual(await f.cleanup(), []);
    assert.equal(f.resumedAttempts, 1); assert.equal(f.snapshot().operator_stop, null);
  });

  test(definition.id + ' keeps normal stop/resume and explicitly withheld-success coverage working', async () => {
    const f = fixture();
    await definition.run(f.ctx);
    assert.deepEqual(await f.cleanup(), []);
    assert.equal(f.resumedAttempts, definition.id.endsWith('073') ? 1 : 2);
    assert.equal(f.snapshot().operator_stop, null);
  });
}

test('an inconsistent successful resume response cannot authorize a cleanup replay', async () => {
  const f = fixture({loseResume: 1, malformed: true});
  await assert.rejects(admissionResume.run(f.ctx), /admin_resume_not_committed/);
  const errors = await f.cleanup();
  assert.equal(f.resumedAttempts, 1);
  assert.ok(errors.some(error => /admin_admission_resume_outcome_unknown/.test(error.message)));
});

test('cleanup still refuses a foreign stop without dispatching any resume', async () => {
  const f = fixture({stopBeforeThird: true});
  await assert.rejects(admissionStop.run(f.ctx), /fixture_step_failed/);
  f.foreignStop();
  await assert.rejects(f.restore(), /admin_admission_foreign_stop/);
  assert.equal(f.resumedAttempts, 0);
  assert.ok((await f.cleanup()).some(error => /admin_admission_foreign_stop/.test(error.message)));
});

test('concurrent restoration callers join one resume dispatch', async () => {
  const f = fixture({stopBeforeThird: true});
  await assert.rejects(admissionStop.run(f.ctx), /fixture_step_failed/);
  await Promise.all([f.restore(), f.restore(), f.restore()]);
  assert.deepEqual(await f.cleanup(), []);
  assert.equal(f.requests.filter(r => r.path.endsWith('/resume')).length, 1);
  assert.equal(f.snapshot().operator_stop, null);
});
