'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {collect, COLLECTIONS} = require('./migration_launch_documents.cjs');
const fs = require('node:fs');
const vm = require('node:vm');
const crypto = require('node:crypto');

test('collector uses only bounded majority reads of the exact synthetic collections', () => {
  const calls = [];
  const result = collect({runCommand(command) {
    calls.push(command);
    return {ok: 1, cursor: {id: 0, ns: 'matrix.' + command.find, firstBatch: [{_id: command.find}]}};
  }});
  assert.deepEqual(calls.map(c => c.find), COLLECTIONS);
  for (const c of calls) {
    assert.deepEqual(c, {find: c.find, filter: {}, sort: {_id: 1}, limit: 17, batchSize: 17,
      singleBatch: true, maxTimeMS: 2000, readConcern: {level: 'majority'}});
  }
  assert.equal(result.database, 'matrix');
  assert.equal(Object.keys(result.collections).length, 3);
});

test('failed command, partial cursor, namespace change and excessive rows fail closed', () => {
  for (const fault of ['failed', 'cursor', 'namespace', 'rows', 'batch', 'null']) {
    assert.throws(() => collect({runCommand(command) {
      const value = {ok: 1, cursor: {id: 0, ns: 'matrix.' + command.find, firstBatch: []}};
      if (fault === 'failed') value.ok = 0;
      if (fault === 'cursor') value.cursor.id = 1;
      if (fault === 'namespace') value.cursor.ns = 'admin.foreign';
      if (fault === 'rows') value.cursor.firstBatch = Array(17).fill({});
      if (fault === 'batch') value.cursor.firstBatch = null;
      return fault === 'null' ? null : value;
    }}), /incomplete or excessive/);
  }
});

test('shell entry closes connection and never logs credentials on failure', () => {
  const source = fs.readFileSync(__dirname + '/migration_launch_documents.cjs', 'utf8');
  for (const fault of [null, 'query', 'oversize', 'connect']) {
    const credential = crypto.randomBytes(24).toString('base64url');
    const printed = [];
    let closed = 0;
    class Connection {
      constructor(uri) {
        assert.equal(uri, credential);
        if (fault === 'connect') throw new Error(credential);
      }
      getDB(name) {
        assert.equal(name, 'matrix');
        return {runCommand(command) {
          if (fault === 'query') throw new Error(credential);
          return {ok: 1, cursor: {id: 0, ns: 'matrix.' + command.find, firstBatch: []}};
        }};
      }
      close() { closed += 1; }
    }
    const context = {process: {env: {MATRIX_LAUNCH_READ_URI: credential}}, Mongo: Connection,
      EJSON: {stringify(value, options) {
        assert.equal(options.relaxed, false);
        return fault === 'oversize' ? 'x'.repeat(1048577) : JSON.stringify(value);
      }}, print: value => printed.push(value), quit: code => { throw new Error('exit-' + code); }};
    if (fault) assert.throws(() => vm.runInNewContext(source, context), /exit-1/);
    else vm.runInNewContext(source, context);
    assert.equal(closed, fault === 'connect' ? 0 : 1);
    assert.equal(printed.length, 1);
    assert.ok(!printed[0].includes(credential));
    if (fault) assert.equal(printed[0], 'migration Launch document collection failed');
    else assert.equal(JSON.parse(printed[0]).schema, 'cma-migration-launch-documents-v1');
  }
});
