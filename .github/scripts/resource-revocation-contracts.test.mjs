import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { checkReleaseCampaign, evaluateReleaseTrace, main, releaseSchedules, validBinding } from './resource-revocation-contracts.mjs';

import { hash, binding, schedules, observation } from './resource-revocation-contracts.fixture.mjs';
import './resource-revocation-matrix.test.mjs';

const delayed = schedules.find(row => row.actions.indexOf('effective_commit') < row.actions.indexOf('chunk_0_handoff_attempt'));
const evaluate = input => evaluateReleaseTrace(delayed, input, binding);

test('enumerates all 1716 unique interleavings without changing either partial order', () => {
  assert.equal(schedules.length, 1716);
  assert.equal(new Set(schedules.map(row => row.id)).size, 1716);
  const release = ['register_body_attempt', 'read_source_attempt', 'final_check_attempt', 'chunk_0_handoff_attempt', 'chunk_1_handoff_attempt', 'close_body'];
  const revoke = ['revoke_request', 'freeze_commit', 'deliver_barrier', 'close_release_gate', 'durable_release_ack', 'effective_commit', 'final_response'];
  for (const row of schedules) {
    assert.equal(row.id, hash(JSON.stringify(row.actions)));
    assert.deepEqual(row.actions.filter(action => release.includes(action)), release);
    assert.deepEqual(row.actions.filter(action => revoke.includes(action)), revoke);
  }
  assert.deepEqual(releaseSchedules(), schedules);
});

test('valid synthetic observations exercise every schedule but cannot qualify candidate coverage', () => {
  const report = checkReleaseCampaign(binding, schedules.map(row => observation(row)));
  assert.equal(report.synthetic_schedules_passed, 1716);
  assert.equal(report.distinct_candidate_schedules_passed, 0);
  assert.equal(report.status, 'blocked');
  assert.equal(report.qualification, 'not_assessed');
  assert.equal(report.activation_authorized, false);
  assert.equal(report.not_observed.length, 0);
});

test('candidate-shaped reports can pass the trace contract without asserting R3 qualification', () => {
  const report = checkReleaseCampaign(binding, schedules.map(row => observation(row, 'candidate')));
  assert.equal(report.status, 'pass');
  assert.equal(report.distinct_candidate_schedules_passed, 1716);
  assert.equal(report.activation_authorized, false);
  assert.equal(report.qualification, 'not_assessed');
});

test('absent or prepared observations remain unexecuted', () => {
  const empty = checkReleaseCampaign(binding, []);
  assert.equal(empty.status, 'blocked');
  assert.equal(empty.not_observed.length, 1716);
  const prepared = observation(delayed);
  prepared.execution = 'not_executed';
  assert.deepEqual(evaluate(prepared).reasons, ['schedule_not_executed']);
  assert.equal(main(['plan']).exit, 2);
  assert(main(['plan']).report.schedules.every(row => row.execution === 'not_executed'));
});

test('999 observed schedules and repeated schedules never satisfy minimum coverage', () => {
  const rows = schedules.slice(0, 999).map(row => observation(row, 'candidate'));
  const report = checkReleaseCampaign(binding, [...rows, rows[0]]);
  assert.equal(report.status, 'blocked');
  assert.equal(report.distinct_candidate_schedules_passed, 999);
  assert(report.errors.includes('duplicate_schedule'));
  assert(report.errors.includes('fewer_than_1000_distinct_candidate_schedules'));
});

test('different schedules cannot count the same retained operation or release twice', () => {
  for (const key of ['operation', 'release']) {
    const rows = schedules.slice(0, 2).map(row => observation(row, 'candidate'));
    rows[1].identity[key] = rows[0].identity[key];
    const report = checkReleaseCampaign(binding, rows);
    assert.equal(report.status, 'blocked');
    assert.equal(report.distinct_candidate_schedules_passed, 1);
    assert.deepEqual(report.attempts[1].reasons, ['reused_retained_identity']);
  }
});

for (const key of Object.keys(binding)) test(`refuses stale ${key} binding`, () => {
  const row = observation(delayed);
  row.binding[key] = key === 'route' ? 'source_read' : key === 'source_revision' ? 'b'.repeat(40) : hash('other');
  assert.deepEqual(evaluate(row).reasons, ['candidate_binding_mismatch']);
});

for (const [name, mutate, reason] of [
  ['handler return', row => { row.boundary = 'handler_return'; }, 'wrong_byte_boundary'],
  ['client receive', row => { row.boundary = 'client_received'; }, 'wrong_byte_boundary'],
  ['missing identity', row => { delete row.identity.api_boot; }, 'missing_retained_identity'],
  ['wrong schedule', row => { row.schedule_id = hash('other'); }, 'missing_or_wrong_schedule'],
  ['missing final observation', row => { row.events.pop(); }, 'incomplete_observation_window'],
  ['reordered events', row => { [row.events[0], row.events[1]] = [row.events[1], row.events[0]]; }, 'invalid_event'],
  ['negative counter', row => { row.events.at(-1).private_bytes_total = -1; }, 'invalid_event'],
  ['string counter', row => { row.events.at(-1).private_bytes_total = '0'; }, 'invalid_event'],
  ['unsafe counter', row => { row.events.at(-1).private_bytes_total = Number.MAX_SAFE_INTEGER + 1; }, 'invalid_event'],
  ['clock reversal', row => { row.events.at(-1).elapsed_ms = 0; }, 'nonmonotonic_observation'],
  ['missing receipt', row => { delete row.events.find(event => event.action === 'effective_commit').receipt_sha256; }, 'missing_durable_receipt'],
  ['missing independent owner', row => { row.events.at(-1).owner_bytes_total = 0; }, 'missing_independent_owner_positive'],
]) test(`blocks ${name}`, () => {
  const row = observation(delayed);
  mutate(row);
  assert.equal(evaluate(row).status, 'blocked');
  assert.deepEqual(evaluate(row).reasons, [reason]);
});

test('actual late bytes fail regardless of a claimed successful status or later passing duplicate', () => {
  const bad = observation(delayed, 'candidate');
  bad.status = 'pass';
  bad.events.at(-1).private_bytes_total++;
  const result = evaluate(bad);
  assert.equal(result.status, 'fail');
  assert(result.reasons.includes('private_bytes_after_effective'));
  assert.equal(result.counters.private_bytes_after_effective, 1);
  const report = checkReleaseCampaign(binding, [bad, observation(delayed, 'candidate')]);
  assert.equal(report.status, 'fail');
  assert.equal(report.failed_attempts, 1);
  assert.equal(report.attempts.length, 2);
  assert(report.errors.includes('duplicate_schedule'));
});

test('late transport flush between gate closure and effective commit fails', () => {
  const row = observation(delayed);
  const ack = row.events.findIndex(event => event.action === 'durable_release_ack');
  for (let index = ack; index < row.events.length; index++) row.events[index].private_bytes_total++;
  const result = evaluate(row);
  assert.equal(result.status, 'fail');
  assert(result.reasons.includes('private_bytes_after_local_close'));
});

test('remaining buffered private data at durable acknowledgment fails', () => {
  const row = observation(delayed);
  row.events.find(event => event.action === 'durable_release_ack').buffered_private_bytes = 4;
  assert.deepEqual(evaluate(row).reasons, ['private_buffer_survives_ack']);
});

test('a passing denial without the promised shared positive stays blocked', () => {
  const row = observation(schedules[0]);
  row.events.forEach(event => { event.private_bytes_total = 0; event.private_bytes_by_chunk = [0, 0]; });
  assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['missing_shared_release_positive']);
});

test('shared bytes after a refused final authorization check fail', () => {
  const row = observation(schedules[0]);
  row.events.find(event => event.action === 'final_check_attempt').authorized = false;
  assert(evaluateReleaseTrace(schedules[0], row, binding).reasons.includes('private_bytes_from_refused_final_check'));
});

test('private-byte claims require observed private source data', () => {
  const row = observation(schedules[0]);
  row.events.find(event => event.action === 'read_source_attempt').source_private_bytes = 0;
  assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['missing_private_source_positive']);
  delete row.events.find(event => event.action === 'final_check_attempt').authorized;
  assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['missing_source_or_authority_observation']);
});

test('denial-only attempts cannot stand in for a body paused between private chunks', () => {
  const rows = schedules.map(schedule => observation(schedule, 'candidate'));
  for (let index = 0; index < rows.length; index++) {
    const actions = schedules[index].actions;
    if (actions.indexOf('chunk_0_handoff_attempt') < actions.indexOf('effective_commit') &&
        actions.indexOf('effective_commit') < actions.indexOf('chunk_1_handoff_attempt'))
      rows[index].events.forEach(event => { event.private_bytes_total = 0; event.private_bytes_by_chunk = [0, 0]; });
  }
  const report = checkReleaseCampaign(binding, rows);
  assert.equal(report.status, 'blocked');
  assert(report.errors.includes('missing_pause_between_chunks'));
});

test('a refused final check cannot stand in for a body paused before its first chunk', () => {
  const rows = schedules.map(schedule => observation(schedule, 'candidate'));
  for (let index = 0; index < rows.length; index++) {
    const actions = schedules[index].actions;
    if (actions.indexOf('effective_commit') < actions.indexOf('chunk_0_handoff_attempt'))
      rows[index].events.find(event => event.action === 'final_check_attempt').authorized = false;
  }
  const report = checkReleaseCampaign(binding, rows);
  assert.equal(report.status, 'blocked');
  assert(report.errors.includes('missing_pause_before_first_chunk'));
});

const between = actions => actions.indexOf('chunk_0_handoff_attempt') < actions.indexOf('effective_commit') &&
  actions.indexOf('effective_commit') < actions.indexOf('chunk_1_handoff_attempt');
function moveBytesToSource(row) {
  const total = row.events.at(-1).private_bytes_total;
  const source = row.events.findIndex(event => event.action === 'read_source_attempt');
  row.events.forEach((event, index) => {
    event.private_bytes_total = index < source ? 0 : total;
    event.private_bytes_by_chunk = [event.private_bytes_total, 0];
  });
}
const firstChunkDelta = row => {
  const index = row.events.findIndex(event => event.action === 'chunk_0_handoff_attempt');
  return row.events[index].private_bytes_total - row.events[index - 1].private_bytes_total;
};

test('source-read bytes before final authority and the first handoff cannot pass', () => {
  const schedule = schedules.find(row => between(row.actions) && observation(row).events.at(-1).private_bytes_total > 0);
  const valid = observation(schedule, 'candidate');
  assert.equal(evaluateReleaseTrace(schedule, valid, binding).status, 'pass');
  assert.equal(firstChunkDelta(valid), 8);
  const invalid = structuredClone(valid);
  moveBytesToSource(invalid);
  assert.equal(invalid.events.find(event => event.action === 'read_source_attempt').private_bytes_total, 8);
  assert.equal(firstChunkDelta(invalid), 0);
  assert.notEqual(evaluateReleaseTrace(schedule, invalid, binding).status, 'pass');
});

test('a campaign with source-read bytes replacing every first-chunk positive cannot pass', () => {
  const rows = schedules.map(schedule => observation(schedule, 'candidate'));
  let changed = 0;
  for (let index = 0; index < rows.length; index++) {
    if (between(schedules[index].actions) && rows[index].events.at(-1).private_bytes_total > 0) {
      moveBytesToSource(rows[index]);
      changed++;
    }
  }
  assert.equal(changed, 39);
  assert(rows.every((row, index) => !between(schedules[index].actions) || firstChunkDelta(row) === 0));
  const report = checkReleaseCampaign(binding, rows);
  assert.notEqual(report.status, 'pass');
  assert(report.errors.includes('missing_pause_between_chunks'));
  assert.equal(report.qualification, 'not_assessed');
  assert.equal(report.activation_authorized, false);
});

test('first-chunk handoff sampled at a later non-chunk action still supplies coverage', () => {
  const schedule = schedules.find(row => between(row.actions) &&
    row.actions[row.actions.indexOf('chunk_0_handoff_attempt') + 1] === 'revoke_request');
  const row = observation(schedule, 'candidate');
  const chunk = row.events.find(event => event.action === 'chunk_0_handoff_attempt');
  chunk.private_bytes_total = 0;
  chunk.private_bytes_by_chunk = [0, 0];
  const later = row.events.find(event => event.action === 'revoke_request');
  assert.equal(later.private_bytes_by_chunk[0], 8);
  assert.equal(firstChunkDelta(row), 0);
  const result = evaluateReleaseTrace(schedule, row, binding);
  assert.equal(result.status, 'pass');
  assert.equal(result.counters.first_chunk_bytes_before_effective, 8);
  const rows = schedules.map(item => {
    if (item.id === schedule.id) return row;
    const input = observation(item, 'candidate');
    if (between(item.actions)) input.events.forEach(event => {
      event.private_bytes_total = 0;
      event.private_bytes_by_chunk = [0, 0];
    });
    return input;
  });
  const report = checkReleaseCampaign(binding, rows);
  assert.equal(report.status, 'pass');
  assert(!report.errors.includes('missing_pause_between_chunks'));
});

test('a later chunk cannot supply bytes before its own handoff attempt', () => {
  const schedule = schedules.find(row => between(row.actions) && observation(row).events.at(-1).private_bytes_total > 0);
  const row = observation(schedule);
  row.events.forEach(event => { event.private_bytes_by_chunk = [0, event.private_bytes_total]; });
  const result = evaluateReleaseTrace(schedule, row, binding);
  assert.equal(result.status, 'fail');
  assert(result.reasons.includes('private_bytes_before_chunk_handoff'));
});

test('bytes sampled at final authorization cannot count as a later handoff', () => {
  const row = observation(schedules[0]);
  row.events.find(event => event.action === 'final_check_attempt').private_bytes_total = 8;
  row.events.find(event => event.action === 'final_check_attempt').private_bytes_by_chunk = [8, 0];
  const result = evaluateReleaseTrace(schedules[0], row, binding);
  assert.equal(result.status, 'fail');
  assert(result.reasons.includes('private_bytes_before_first_handoff'));
});

test('aggregate-only reports stay blocked without chunk attribution', () => {
  const row = observation(schedules[0]);
  row.events.forEach(event => { delete event.private_bytes_by_chunk; });
  assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['missing_or_invalid_chunk_counters']);
});

test('chunk counters must be bounded, complete and equal the observed aggregate', () => {
  for (const counts of [null, [], [0], [0, 0, 0], ['0', 0], [-1, 0], [Number.MAX_SAFE_INTEGER + 1, 0]]) {
    const row = observation(schedules[0]);
    row.events[0].private_bytes_by_chunk = counts;
    assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['missing_or_invalid_chunk_counters']);
  }
  const row = observation(schedules[0]);
  row.events.at(-1).private_bytes_by_chunk = [8, 7];
  assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['chunk_byte_total_mismatch']);
});

test('chunk counters cannot reset or move earlier bytes into another chunk', () => {
  const row = observation(schedules[0]);
  row.events.at(-1).private_bytes_by_chunk = [0, 16];
  assert.deepEqual(evaluateReleaseTrace(schedules[0], row, binding).reasons, ['nonmonotonic_chunk_observation']);
});

test('counter reset cannot hide an earlier late handoff', () => {
  const row = observation(delayed);
  row.events.at(-2).private_bytes_total = 1;
  assert.deepEqual(evaluate(row).reasons, ['nonmonotonic_observation']);
});

test('rejected registration cannot emit private content', () => {
  const schedule = schedules.at(-1);
  const row = observation(schedule);
  row.events.at(-1).private_bytes_total = 1;
  assert(evaluateReleaseTrace(schedule, row, binding).reasons.includes('private_bytes_from_refused_registration'));
});

test('empty, malformed and unbounded inventories fail closed', () => {
  assert.equal(validBinding({ ...binding, source_revision: 'main' }), false);
  for (const rows of [null, {}, Array(3433).fill(null)])
    assert.equal(checkReleaseCampaign(binding, rows).status, 'blocked');
  const report = checkReleaseCampaign(binding, [{ schedule_id: 'unrecognized' }]);
  assert(report.errors.includes('unknown_schedule'));
});

test('CLI returns blocked for no observations and never echoes raw input on errors', () => {
  const directory = mkdtempSync(join(tmpdir(), 'cma-revocation-trace-'));
  const script = fileURLToPath(new URL('./resource-revocation-contracts.mjs', import.meta.url));
  const expected = join(directory, 'expected.json'), input = join(directory, 'observations.json');
  try {
    writeFileSync(expected, JSON.stringify(binding));
    writeFileSync(input, '[]');
    const run = () => spawnSync(process.execPath, [script, 'check', expected, input], { encoding: 'utf8', maxBuffer: 2 * 1024 * 1024 });
    let result = run();
    assert.equal(result.status, 2);
    assert.equal(JSON.parse(result.stdout).not_observed.length, 1716);
    writeFileSync(input, '{untrusted fixture details');
    result = run();
    assert.equal(result.status, 2);
    assert(!result.stdout.includes('untrusted fixture details'));
    assert.deepEqual(JSON.parse(result.stdout).errors, ['invalid_input']);
    const bad = observation(delayed, 'candidate');
    bad.events.at(-1).private_bytes_total++;
    writeFileSync(input, JSON.stringify([bad]));
    assert.equal(run().status, 1);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('repository CI includes the exact synthetic checker command', () => {
  const workflow = readFileSync(new URL('../workflows/ci-repo.yml', import.meta.url), 'utf8');
  const command = 'node --test .github/scripts/resource-revocation-contracts.test.mjs';
  assert.equal(workflow.split(command).length - 1, 1);
  assert(workflow.includes(`run: "${command}"`));
});
