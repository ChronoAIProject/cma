'use strict';
const assert = require('node:assert/strict');
const test = require('node:test');
const fs = require('node:fs');
const vm = require('node:vm');
const {collectCatalog} = require('./migration_catalog.cjs');

const row = (name = 'sandbox_usage_events') => ({name, type: 'collection',
  options: {validator: {document_schema_version: {$eq: 1}}}, info: {uuid: 'uuid-1'}});
const index = (name = '_id_', key = {_id: 1}) => ({name, key, v: 2});
const batch = (ns, rows, id = 0, key = 'firstBatch') =>
  ({ok: 1, cursor: {ns: 'matrix.' + ns, id, [key]: rows}});

function database(responses) {
  return {calls: [], runCommand(command) {
    this.calls.push(command);
    assert.ok(responses.length, 'unexpected catalog request');
    const response = responses.shift();
    if (response instanceof Error) throw response;
    return response;
  }};
}

test('drains both collection and index cursors and preserves compound key order', () => {
  const db = database([
    batch('$cmd.listCollections', [row()], 31),
    batch('$cmd.listCollections', [{name: 'summary', type: 'view',
      options: {viewOn: 'sandbox_usage_events', pipeline: []}}], 0, 'nextBatch'),
    batch('sandbox_usage_events', [index()], 72),
    batch('sandbox_usage_events', [index('interval',
      {sandbox_id: 1, ended_at: 1, started_at: 1, revision: 1})], 0, 'nextBatch'),
  ]);
  const result = collectCatalog(db);
  assert.equal(result.collections.length, 2);
  assert.deepEqual(result.collections[0].indexes[1].key,
    [['sandbox_id', 1], ['ended_at', 1], ['started_at', 1], ['revision', 1]]);
  assert.deepEqual(result.collections[1].indexes, []);
  assert.deepEqual(db.calls.map(call => Object.keys(call)[0]),
    ['listCollections', 'getMore', 'listIndexes', 'getMore']);
});

test('fails on server error, malformed/empty inventory, or missing UUID', () => {
  for (const responses of [[{ok: 0, errmsg: 'withheld'}], [{ok: 1}],
    [batch('$cmd.listCollections', [])],
    [batch('$cmd.listCollections', [{...row(), info: {}}])],
    [batch('$cmd.listCollections', [row(), row()])]]) {
    assert.throws(() => collectCatalog(database(responses)));
  }
});

test('missing, duplicate, or unfinished index prevents a partial catalog receipt', () => {
  for (const indexes of [[], [index(), index()], [{...index(), buildUUID: 'building'}],
    [{spec: index()}], [index('bad', {})]]) {
    assert.throws(() => collectCatalog(database([
      batch('$cmd.listCollections', [row()]), batch('sandbox_usage_events', indexes),
    ])));
  }
});

test('kills an open cursor when inventory exceeds the explicit bound', () => {
  const db = database([batch('$cmd.listCollections',
    Array.from({length: 513}, (_, i) => row('row-' + i)), 19), {ok: 1}]);
  assert.throws(() => collectCatalog(db), /exceeds bound/);
  assert.deepEqual(db.calls.at(-1), {killCursors: '$cmd.listCollections', cursors: [19]});
});

test('getMore failure and cursor namespace changes fail closed and join the cursor', () => {
  for (const response of [new Error('transport'), {ok: 0},
    {ok: 1, cursor: {ns: 'other.rows', id: 41, nextBatch: []}}]) {
    const db = database([batch('$cmd.listCollections', [row()], 41), response, {ok: 1}]);
    assert.throws(() => collectCatalog(db));
    assert.deepEqual(db.calls.at(-1), {killCursors: '$cmd.listCollections', cursors: [41]});
  }
});

test('a cursor that never advances is bounded even with empty batches', () => {
  const db = database([batch('$cmd.listCollections', [], 7),
    ...Array.from({length: 513}, () => batch('$cmd.listCollections', [], 7, 'nextBatch')),
    {ok: 1}]);
  assert.throws(() => collectCatalog(db), /did not finish/);
  assert.equal(db.calls.at(-1).killCursors, '$cmd.listCollections');
});

test('collector touches metadata only and preserves validator and TTL definitions', () => {
  const db = database([batch('$cmd.listCollections', [row()]),
    batch('sandbox_usage_events', [index(), {...index('ttl', {expires_at: 1}),
      expireAfterSeconds: 0, partialFilterExpression: {state: 'finished'}}])]);
  const result = collectCatalog(db);
  assert.deepEqual(result.collections[0].options, row().options);
  assert.equal(result.collections[0].indexes[1].expireAfterSeconds, 0);
  assert.ok(db.calls.every(call => ['listCollections', 'listIndexes', 'getMore', 'killCursors']
    .includes(Object.keys(call)[0])));
});

test('mongosh entrypoint emits one catalog, closes its connection, and redacts failures', () => {
  const script = fs.readFileSync(require.resolve('./migration_catalog.cjs'), 'utf8');
  for (const fails of [false, true]) {
    let closed = false;
    const output = [];
    const db = database(fails ? [new Error('synthetic-private-uri')] : [
      batch('$cmd.listCollections', [row()]), batch('sandbox_usage_events', [index()]),
    ]);
    const context = {process: {env: {MATRIX_CATALOG_URI: 'synthetic-private-uri'}},
      Mongo: class {
        constructor(uri) { assert.equal(uri, 'synthetic-private-uri'); }
        getDB(name) { assert.equal(name, 'matrix'); return db; }
        close() { closed = true; }
      }, EJSON: {stringify: JSON.stringify}, print: text => output.push(text),
      quit: code => { throw new Error('exit:' + code); }};
    if (fails) assert.throws(() => vm.runInNewContext(script, context), /exit:1/);
    else vm.runInNewContext(script, context);
    assert.equal(closed, true);
    assert.equal(output.length, 1);
    assert.ok(!output[0].includes('synthetic-private-uri'));
    if (!fails) assert.equal(JSON.parse(output[0]).collections.length, 1);
  }
});
