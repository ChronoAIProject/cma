'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const {collect, MEMBERS, OWNERS} = require('./migration_membership_documents.cjs');

function reply(command) {
  if (command.hello) return {ok: 1, localTime: new Date(1000)};
  return {ok: 1, cursor: {id: '0', ns: 'matrix.' + command.find,
    firstBatch: command.find === MEMBERS ? [{_id: 'incarnation'}] : []}};
}

test('only bounded majority reads of members and empty owners, then server clock', () => {
  const commands = [];
  const value = collect({runCommand(command) { commands.push(command); return reply(command); }});
  assert.deepEqual(commands, [
    {find: MEMBERS, filter: {}, sort: {_id: 1}, limit: 17, batchSize: 17,
      singleBatch: true, maxTimeMS: 2000, readConcern: {level: 'majority'}},
    {find: OWNERS, filter: {}, sort: {_id: 1}, limit: 1, batchSize: 1,
      singleBatch: true, maxTimeMS: 2000, readConcern: {level: 'majority'}},
    {hello: 1, maxTimeMS: 2000}]);
  assert.equal(value.database, 'matrix');
  assert.equal(value.observed_at.getTime(), 1000);
  assert.deepEqual(value.collections[OWNERS], []);
});

test('unfinished, oversized, malformed and foreign cursors fail', () => {
  for (const value of [null, {ok: 0}, {ok: 1}, {ok: 1, cursor: {}},
    {ok: 1, cursor: {id: '1', ns: 'matrix.' + MEMBERS, firstBatch: []}},
    {ok: 1, cursor: {id: '0', ns: 'foreign.' + MEMBERS, firstBatch: []}},
    {ok: 1, cursor: {id: '0', ns: 'matrix.' + MEMBERS, firstBatch: {}}},
    {ok: 1, cursor: {id: '0', ns: 'matrix.' + MEMBERS, firstBatch: Array(17).fill({})}}]) {
    assert.throws(() => collect({runCommand() { return value; }}), /snapshot/);
  }
  assert.throws(() => collect({runCommand(command) {
    const value = reply(command);
    if (command.find === OWNERS) value.cursor.firstBatch.push({_id: 'agent'});
    return value;
  }}), /snapshot/);
});

test('missing or invalid server clock cannot classify lease expiry', () => {
  for (const clock of [null, {ok: 0}, {ok: 1}, {ok: 1, localTime: '1000'},
    {ok: 1, localTime: new Date(NaN)}]) {
    assert.throws(() => collect({runCommand(command) {
      return command.hello ? clock : reply(command);
    }}), /server clock/);
  }
});

function main(options = {}) {
  const printed = [], calls = [];
  let closed = 0;
  const filename = path.join(__dirname, 'migration_membership_documents.cjs');
  const context = {
    Date,
    process: {env: {MATRIX_MEMBERSHIP_READ_URI: 'ephemeral-local-fixture'}},
    Mongo: function () {
      if (options.connectError) throw new Error('credential must not appear');
      return {getDB(name) { calls.push(name); return {runCommand: reply}; }, close() { closed++; }};
    },
    EJSON: {stringify(value, settings) {
      assert.equal(settings.relaxed, false);
      return options.oversize ? 'x'.repeat(65537) : JSON.stringify(value);
    }},
    print(value) { printed.push(value); },
    quit(code) { assert.equal(code, 1); },
  };
  vm.runInNewContext(fs.readFileSync(filename, 'utf8'), context, {filename});
  return {printed, calls, closed};
}

test('mongosh entry point uses canonical EJSON and closes its connection', () => {
  const result = main();
  assert.deepEqual(result.calls, ['matrix']);
  assert.equal(result.closed, 1);
  assert.equal(JSON.parse(result.printed[0]).schema, 'cma-migration-membership-v1');
});

test('entry failures stay redacted and close connections when present', () => {
  const oversized = main({oversize: true});
  assert.equal(oversized.closed, 1);
  assert.deepEqual(oversized.printed, ['migration membership collection failed']);
  const disconnected = main({connectError: true});
  assert.equal(disconnected.closed, 0);
  assert.deepEqual(disconnected.printed, ['migration membership collection failed']);
});
