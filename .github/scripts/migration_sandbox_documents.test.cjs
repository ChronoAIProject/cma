'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {collect, COLLECTIONS} = require('./migration_sandbox_documents.cjs');

test('only bounded majority reads of the exact control collection', () => {
  const requests = [];
  const value = collect({runCommand(command) {
    requests.push(command);
    return {ok: 1, cursor: {id: {toString() { return '0'; }}, ns: 'matrix.' + command.find,
      firstBatch: [{_id: 'fixture'}]}};
  }});
  assert.deepEqual(COLLECTIONS, ['sandbox_controls']);
  assert.deepEqual(requests, [{find: 'sandbox_controls', filter: {}, sort: {_id: 1},
    limit: 32, batchSize: 32, singleBatch: true, maxTimeMS: 2000, readConcern: {level: 'majority'}}]);
  assert.equal(value.collections.sandbox_controls.length, 1);
});

test('missing cursor, unfinished cursor, wrong namespace and excessive rows fail', () => {
  for (const reply of [null, {ok: 0}, {ok: 1, cursor: {id: '1', ns: 'matrix.sandbox_controls', firstBatch: []}},
    {ok: 1, cursor: {id: '0', ns: 'foreign.sandbox_controls', firstBatch: []}},
    {ok: 1, cursor: {id: '0', ns: 'matrix.sandbox_controls', firstBatch: Array(32).fill({})}},
    {ok: 1, cursor: {id: '0', ns: 'matrix.sandbox_controls', firstBatch: {}}}]) {
    assert.throws(() => collect({runCommand() { return reply; }}), /snapshot/);
  }
});
