import test from 'node:test';
import assert from 'node:assert/strict';
import {admissionRead, validateAdmission} from './admission-read.mjs';
import {admissionReadModel, admissionState} from './admission-read-model.mjs';
const run = h => admissionRead.run(h.ctx);
const messages = e => [e.message, ...(e.errors ?? []).map(messages)].join(' ');

test('changing live capacity counters do not invalidate unchanged durable admission control', async () => {
  const h = admissionReadModel({capacityDrift: true}); await run(h); assert.deepEqual(await h.cleanup(), []);
});
test('a typed refusal cannot conceal a committed admission change', async () => {
  const h = admissionReadModel({changedRefusal: true});
  await assert.rejects(run(h), e => messages(e).includes('admin_admission_durable_changed')); await h.cleanup();
});
test('the real parent write mode is restored before cleanup can succeed', async () => {
  const h = admissionReadModel(); await run(h); assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.writes(), true); assert.deepEqual(h.modes, [false, true]);
});

const posts = h => h.calls.filter(call => call.method === 'POST');
const observed = (h, label) => h.observations.filter(o => o.label === label).map(o => o.value);
const failed = (h, code) => assert.rejects(run(h), error => messages(error).includes(code));

for (const am of ['a', 'b']) {
  test('two original refusals retain exact revision, both-AM no-effect readbacks and raw evidence from ' + am, async () => {
    const h = admissionReadModel({am, capacityDrift: true}); await run(h);
    assert.deepEqual(posts(h).map(p => [p.am, p.body.expected_revision]), [['a', 22], ['b', 22]]);
    assert.ok(posts(h).every(p => p.actor === 'admin' && p.path === '/api/v1/admin/admission/stop'
      && p.allowed.length === 1 && p.allowed[0] === 503 && p.body.reason.trim()));
    const expected = ['GET:' + am, 'GET:' + (am === 'a' ? 'b' : 'a')];
    assert.deepEqual(h.calls.map(p => p.method + ':' + p.am), [
      ...expected, ...expected, 'POST:a', ...expected, 'POST:b', ...expected, 'GET:a', 'GET:b',
    ]);
    assert.equal(observed(h, 'admin_admission_after_refusal').length, 4);
    assert.equal(observed(h, 'admin_admission_refusal').length, 2);
    assert.equal(h.observations.length, h.calls.length);
    const snapshots = h.observations.filter(o => o.value.response.status === 200);
    assert.equal(new Set(snapshots.map(o => o.value.response.body.capacity.active_sessions.observed)).size, 10);
    for (const {value: {am: reached, response}} of h.observations) {
      assert.deepEqual(response.timing, {am: reached, request_id: response.timing.request_id,
        serving: {pod_uid: 'model-' + reached}, serving_after: {pod_uid: 'model-' + reached},
        started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
      assert.match(response.timing.request_id, /^api-\d+$/);
    }
    const count = h.calls.length; assert.deepEqual(await h.cleanup(), []);
    assert.equal(h.calls.length, count); assert.deepEqual(h.modes, [false, true]);
  });
}

for (const revision of [21, 22]) {
  for (const initialWrites of [false, true]) {
    test(`preserve ${revision % 2 ? 'stopped' : 'open'} control and original writes=${initialWrites}`, async () => {
      const h = admissionReadModel({revision, initialWrites}); await run(h);
      assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.state(), h.original);
      assert.equal(h.writes(), initialWrites); assert.equal(posts(h).length, 2);
      assert.deepEqual(h.modes, initialWrites ? [false, true] : []);
    });
  }
}

for (const revision of [0, 1, 20]) {
  test('insufficient seeded history blocks before mode change or dispatch at revision ' + revision, async () => {
    const h = admissionReadModel({revision}); await failed(h, 'admin_admission_history_boundary_not_seeded');
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0); assert.deepEqual(h.modes, []);
  });
}

test('initial inconsistent durable controls block before any write', async () => {
  const h = admissionReadModel({splitState: true}); await failed(h, 'admin_admission_snapshot_mismatch');
  assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0); assert.deepEqual(h.modes, []);
});
test('initial inconsistent availability blocks before any mode change', async () => {
  const h = admissionReadModel({initialWrites: false, splitGate: true});
  await failed(h, 'admin_admission_snapshot_mismatch');
  assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 0); assert.deepEqual(h.modes, []);
});
for (const option of ['splitGate', 'ignoreMode', 'changedMode']) {
  test(option + ' is detected before either refusal dispatch and gate intent is restored once', async () => {
    const h = admissionReadModel({[option]: true});
    await failed(h, option === 'changedMode' ? 'admin_admission_durable_changed' : 'admin_admission_write_gate');
    const errors = await h.cleanup();
    assert.equal(errors.length, option === 'changedMode' ? 1 : 0);
    if (errors.length) assert.match(messages(errors[0]), /admin_admission_durable_changed/);
    assert.equal(posts(h).length, 0); assert.deepEqual(h.modes, [false, true]); assert.equal(h.writes(), true);
  });
}
test('failed refusal with a committed change remains failed after gate restoration without a repair or second POST', async () => {
  const h = admissionReadModel({changedRefusal: true}); await failed(h, 'admin_admission_durable_changed');
  const errors = await h.cleanup(); assert.equal(errors.length, 1);
  assert.equal(errors[0].errors.length, 2); assert.match(messages(errors[0]), /admin_admission_durable_changed/);
  assert.equal(posts(h).length, 1); assert.equal(h.state().revision, 23);
  assert.deepEqual(h.modes, [false, true]); assert.deepEqual(observed(h, 'admin_admission_final').map(o => o.am), ['a', 'b']);
  const count = h.calls.length; const again = await h.cleanup();
  assert.equal(again[0], errors[0]); assert.equal(h.calls.length, count);
});
for (const [option, code] of [['wrongCode', 'admin_admission_wrong_unavailable_code'], ['accepted', 'status_200'], ['unknownPost', 'post_unknown']]) {
  test(option + ' prevents a second dispatch and never replays the first', async () => {
    const h = admissionReadModel({[option]: true}); await failed(h, code);
    assert.deepEqual(await h.cleanup(), []); assert.equal(posts(h).length, 1);
    assert.deepEqual(h.modes, [false, true]); assert.deepEqual(h.state(), h.original);
    const count = h.calls.length; assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, count);
  });
}
test('a lost disable acknowledgement retains intent and restores the parent gate once', async () => {
  const h = admissionReadModel({unknownMode: true}); await failed(h, 'mode_unknown');
  assert.equal(h.writes(), false); assert.deepEqual(await h.cleanup(), []);
  assert.equal(h.writes(), true); assert.deepEqual(h.modes, [false, true]); assert.equal(posts(h).length, 0);
  assert.deepEqual(observed(h, 'admin_admission_final').map(o => o.am), ['a', 'b']);
});
test('a lost restoration acknowledgement stays failed even if both final readbacks are correct', async () => {
  const h = admissionReadModel({unknownRestore: true}); let originalError;
  await assert.rejects(run(h), error => {originalError = error; return messages(error).includes('restore_unknown');});
  assert.equal(h.writes(), true); assert.equal(posts(h).length, 2);
  assert.deepEqual(observed(h, 'admin_admission_final').map(o => o.am), ['a', 'b']);
  const count = h.calls.length; const errors = await h.cleanup();
  assert.equal(errors[0], originalError); assert.equal(h.calls.length, count); assert.deepEqual(h.modes, [false, true]);
});
test('restoration and independent A read failure retain B evidence without replay', async () => {
  const h = admissionReadModel({unknownRestore: true, restoreReadFailure: true});
  await failed(h, 'admin_admission_read_finality');
  const errors = await h.cleanup(); assert.equal(errors.length, 1); assert.equal(errors[0].errors.length, 2);
  assert.match(messages(errors[0]), /restore_unknown/); assert.match(messages(errors[0]), /restored_read_failed/);
  assert.deepEqual(observed(h, 'admin_admission_final').map(o => o.am), ['b']);
  assert.deepEqual(h.calls.slice(-2).map(p => p.am), ['a', 'b']); assert.deepEqual(h.modes, [false, true]);
});
test('the parent cleanup still runs after finality fails and retains its independent error', async () => {
  const h = admissionReadModel({unknownRestore: true}); let disposed = 0;
  h.f.cleanup = async () => {disposed++; throw Error('parent_disposal_failed');};
  await failed(h, 'restore_unknown'); const errors = await h.cleanup();
  assert.equal(disposed, 1); assert.equal(errors.length, 2);
  assert.match(messages(errors[0]), /restore_unknown/); assert.match(messages(errors[1]), /parent_disposal_failed/);
});
test('interrupted expected-result step still proves the original state outside an active assertion step', async () => {
  const h = admissionReadModel({interruptedStep: true}); await failed(h, 'step_interrupted');
  assert.deepEqual(await h.cleanup(), []); assert.deepEqual(h.modes, []); assert.equal(posts(h).length, 0);
  assert.deepEqual(observed(h, 'admin_admission_final').map(o => o.am), ['a', 'b']);
});
for (const key of ['exclusiveAdmission', 'setWriteAvailability']) {
  test('authentic parent ' + key + ' is mandatory before HTTP or mode transitions', async () => {
    const h = admissionReadModel(); delete h.f[key]; await failed(h, 'admin_admission_availability_fixture_missing');
    assert.deepEqual(await h.cleanup(), []); assert.equal(h.calls.length, 0); assert.deepEqual(h.modes, []);
  });
}

test('valid admission snapshots cover zero, short history and exact twenty-row retention', () => {
  for (const revision of [0, 1, 2, 19, 20, 21, 22, 50]) assert.equal(validateAdmission(admissionState(revision)), true);
});
test('UTF-8 reasons use byte limits and allow valid non-ASCII text at the boundary', () => {
  for (const reason of ['a'.repeat(512), 'é'.repeat(256), '🙂'.repeat(128)]) {
    const value = admissionState(1); value.history[0].reason = reason; value.history[0].new_stop.reason = reason;
    value.last_change = structuredClone(value.history[0]); value.operator_stop = structuredClone(value.history[0].new_stop);
    assert.equal(validateAdmission(value), true);
  }
});
const invalid = [
  ['missing revision', v => {delete v.revision;}],
  ['unsafe revision', v => {v.revision = Number.MAX_SAFE_INTEGER + 1;}],
  ['negative revision', v => {v.revision = -1;}],
  ['fractional revision', v => {v.revision = 22.5;}],
  ['implicit availability', v => {delete v.writes_available;}],
  ['nonboolean availability', v => {v.writes_available = 'true';}],
  ['missing history', v => {delete v.history;}],
  ['missing row', v => {v.history.pop();}],
  ['too many rows', v => {v.history.push(structuredClone(v.history.at(-1)));}],
  ['null last change', v => {v.last_change = null;}],
  ['null head and last change', v => {v.last_change = null; v.history[0] = null;}],
  ['null older row', v => {v.history[10] = null;}],
  ['last change mismatch', v => {v.last_change.at++;}],
  ['current stop mismatch', v => {v.operator_stop = structuredClone(v.history[0].old_stop);}],
  ['duplicate revision', v => {v.history[5].revision++;}],
  ['gapped revision', v => {v.history[5].revision--;}],
  ['reversed ordering', v => {v.history.reverse();}],
  ['changed linkage', v => {v.history[0].old_stop.actor = 'different'; v.last_change = structuredClone(v.history[0]);}],
];
for (const [name, corrupt] of invalid) {
  test('malformed durable snapshot refuses ' + name, () => {
    const value = admissionState(); corrupt(value); assert.equal(validateAdmission(value), false);
  });
}
// The oldest retained entry has no following history row. Validate its own
// stop/change invariants independently rather than relying only on linkage.
const invalidChanges = [
  ['blank actor', c => {c.actor = '';}],
  ['negative timestamp', c => {c.at = -1;}],
  ['unsafe timestamp', c => {c.at = Number.MAX_SAFE_INTEGER + 1;}],
  ['fractional timestamp', c => {c.at = 1.2;}],
  ['blank reason', c => {c.reason = ' ';}],
  ['overlong reason bytes', c => {c.reason = 'é'.repeat(257);}],
  ['control in reason', c => {c.reason = 'reason\u0085';}],
  ['no transition', c => {c.new_stop = null;}],
  ['two stops', c => {c.old_stop = structuredClone(c.new_stop);}],
  ['missing old stop', c => {delete c.old_stop;}],
  ['missing new stop', c => {delete c.new_stop;}],
  ['stop revision mismatch', c => {c.new_stop.revision--;}],
  ['stop timestamp mismatch', c => {c.new_stop.since++;}],
  ['stop actor mismatch', c => {c.new_stop.actor = 'different';}],
  ['stop reason mismatch', c => {c.new_stop.reason = 'different';}],
];
for (const [name, corrupt] of invalidChanges) {
  test('malformed stop change refuses ' + name, () => {
    const value = admissionState(); corrupt(value.history.at(-1)); assert.equal(validateAdmission(value), false);
  });
}
for (const [field, value] of [['revision', 0], ['revision', 99], ['revision', 1.5], ['actor', ''],
  ['since', -1], ['since', Number.MAX_SAFE_INTEGER + 1], ['reason', ''], ['reason', 'x'.repeat(513)], ['reason', 'x\n']]) {
  test('old stop independently rejects ' + field + '=' + JSON.stringify(value), () => {
    const state = admissionState(21); state.history.at(-1).old_stop[field] = value;
    assert.equal(validateAdmission(state), false);
  });
}
test('zero revision requires both stop and last change to be explicitly null', () => {
  for (const field of ['operator_stop', 'last_change']) {
    const value = admissionState(0); delete value[field]; assert.equal(validateAdmission(value), false);
    value[field] = {}; assert.equal(validateAdmission(value), false);
  }
});
test('null and primitive snapshots return false without throwing', () => {
  for (const value of [null, undefined, '', 42, [], {}]) assert.equal(validateAdmission(value), false);
});
