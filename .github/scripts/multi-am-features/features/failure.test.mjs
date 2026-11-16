import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, readdir, readFile, rm, stat} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {retainFailure, withRestoration} from '../failure.mjs';

test('primary typed conflict and restoration failure remain separate private evidence', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'multi-am-failure-'));
  const primary = Object.assign(Error('api_status_409'), {response: {status: 409,
    body: {code: 'agent_operation_in_flight', detail: 'private fixture detail'},
    timing: {request_id: 'private-fixture-request'}}});
  const restoration = Object.assign(Error('api_status_503'), {response: {status: 503,
    body: {code: 'resource_access_unavailable'}}});
  const receipts = [];
  try {
    await assert.rejects(withRestoration(async () => { throw primary; }, async () => { throw restoration; },
      async (stage, error) => { receipts.push(await retainFailure(directory, stage, error, {expected_revision: 7})); }),
    error => error === primary);
    assert.deepEqual(receipts.map(row => row.stage), ['primary', 'restoration']);
    assert.deepEqual(receipts.map(row => row.problem_code), ['agent_operation_in_flight', 'resource_access_unavailable']);
    assert.ok(!JSON.stringify(receipts).includes('private-fixture-request'));
    assert.ok(!JSON.stringify(receipts).includes('private fixture detail'));
    const records = [];
    for (const file of await readdir(directory)) {
      const path = join(directory, file), raw = await readFile(path);
      assert.equal((await stat(path)).mode & 0o077, 0);
      assert.ok(receipts.some(row => row.evidence_sha256 === createHash('sha256').update(raw).digest('hex')));
      records.push(JSON.parse(raw));
    }
    assert.equal(records.find(row => row.stage === 'primary').response.timing.request_id, 'private-fixture-request');
    assert.equal(records.find(row => row.stage === 'restoration').response.status, 503);
  } finally { await rm(directory, {recursive: true, force: true}); }
});

test('evidence-write failure cannot prevent restoration or replace the primary failure', async () => {
  const primary = Error('original_conflict');
  let restored = false;
  await assert.rejects(withRestoration(async () => { throw primary; }, async () => { restored = true; },
    async () => { throw Error('evidence_io_failure'); }), error => error === primary);
  assert.equal(restored, true);
});
