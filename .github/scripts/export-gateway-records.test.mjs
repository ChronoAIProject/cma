import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const script = fs.readFileSync(new URL('./export-gateway-records.js', import.meta.url), 'utf8');
const collections = ['gateway_channel_records', 'gateway_connection_records', 'gateway_interaction_records'];
const id = '00000000-0000-4000-8000-000000000001';
const long = value => ({ _bsontype: 'Long', toString: () => String(value) });
const binary = bytes => ({
  _bsontype: 'Binary', sub_type: 0,
  // BSON Binary may hold more backing capacity than its logical bytes.
  buffer: Buffer.concat([Buffer.from(bytes), Buffer.from([91, 92, 93])]),
  length() { return bytes.length; },
  value(raw) { assert.equal(raw, true); return this.buffer.subarray(0, bytes.length); },
});
const row = (changes = {}) => ({
  _id: id, owner: 'migration-fixture-owner', document_schema_version: long(1),
  version: long(1), blob: binary([0, 128, 255, 0]), ...changes,
});

function fixture(t) {
  const directory = fs.mkdtempSync(join(tmpdir(), 'cma-gateway-export-test-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const output = join(directory, 'export.ndjson');
  const printed = [], closed = [], queried = [];
  const data = {
    gateway_channel_records: [row()],
    gateway_connection_records: [row()],
    gateway_interaction_records: [row({ expires_at: new Date('2030-01-02T03:04:05Z') })],
  };
  return {
    directory, output, printed, closed, queried, data,
    records: () => fs.readFileSync(output, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse),
    run({ names = collections, fileSystem = {}, readFailure, closeFailure, environment = { CMA_GATEWAY_EXPORT_FILE: output } } = {}) {
      return runInNewContext(script, {
        Buffer, process: { env: environment }, print: message => printed.push(message),
        require: name => {
          if (name === 'node:fs') return { ...fs, ...fileSystem };
          if (name === 'node:path') return { join, dirname: value => value.slice(0, value.lastIndexOf('/')) };
          throw new Error('Unexpected module');
        },
        db: {
          getCollectionNames: () => names,
          getCollection(name) {
            queried.push(name);
            return {
              find(query) {
                assert.equal(JSON.stringify(query), '{}');
                return {
                  sort(order) { assert.equal(JSON.stringify(order), '{"_id":1}'); return this; },
                  batchSize(size) { assert.equal(size, 100); return this; },
                  *[Symbol.iterator]() {
                    for (const record of data[name]) yield record;
                    if (readFailure === name) throw new Error('private-driver-detail');
                  },
                  close() { closed.push(name); if (closeFailure === name) throw new Error('private-close-detail'); },
                };
              },
            };
          },
        },
      }, { filename: 'export-gateway-records.js', timeout: 2000 });
    },
    rejects(options) {
      assert.throws(() => this.run(options), error => {
        assert.equal(error.message, 'Gateway export failed; check source collections, record format and output availability');
        assert.doesNotMatch(error.stack, /private-driver-detail|private-close-detail|private-filesystem-detail/);
        return true;
      });
      assert.deepEqual(printed, []);
      assert.deepEqual(fs.readdirSync(directory), []);
    },
  };
}

test('exports all three domains with exact IDs, owners, i64 versions, ciphertext bytes and expiry', t => {
  const f = fixture(t);
  f.data.gateway_channel_records[0].version = long('9223372036854775807');
  f.data.gateway_channel_records[0].owner = 'owner-频道';
  f.data.gateway_connection_records[0].document_schema_version = { _bsontype: 'Int32', toString: () => '1' };
  f.data.gateway_connection_records[0].version = 42;
  f.run({ fileSystem: {
    writeFileSync(fd, bytes) {
      assert.equal(fs.existsSync(f.output), false, 'final file must not expose a partial export');
      assert.equal(fs.fstatSync(fd).mode & 0o777, 0o600);
      return fs.writeFileSync(fd, bytes);
    },
  } });
  assert.deepEqual(f.records(), [
    { kind: 'channel', record_id: id, owner: 'owner-频道', version: '9223372036854775807', blob: 'AID/AA==' },
    { kind: 'connection', record_id: id, owner: 'migration-fixture-owner', version: '42', blob: 'AID/AA==' },
    { kind: 'interaction', record_id: id, owner: 'migration-fixture-owner', version: '1', blob: 'AID/AA==', expires_at: 1893553445 },
  ]);
  for (const record of f.records()) assert.deepEqual(Buffer.from(record.blob, 'base64'), Buffer.from([0, 128, 255, 0]));
  assert.deepEqual(f.queried, collections);
  assert.deepEqual(f.closed, collections);
  assert.equal(fs.statSync(f.output).mode & 0o777, 0o600);
  assert.deepEqual(fs.readdirSync(f.directory), ['export.ndjson']);
  assert.deepEqual(f.printed, ['Exported 1 channel, 1 connection, 1 interaction sealed records']);
  assert.ok(fs.readFileSync(f.output, 'utf8').endsWith('\n'));
});

test('retains expired interactions and exact representable expiry boundaries', t => {
  const f = fixture(t);
  f.data.gateway_interaction_records = [
    row({ expires_at: new Date(1000) }),
    row({ _id: id.replace(/1$/, '2'), expires_at: new Date(8640000000000000) }),
  ];
  f.run();
  assert.deepEqual(f.records().filter(record => record.kind === 'interaction').map(record => record.expires_at), [1, 8640000000000]);
});

test('accepts bounded maximum ciphertext and UTF-8 owner without changing either', t => {
  const f = fixture(t);
  const bytes = Buffer.from(Array.from({ length: 65536 }, (_, index) => index % 256));
  f.data.gateway_channel_records[0].blob = binary(bytes);
  f.data.gateway_channel_records[0].owner = '界'.repeat(170) + 'ab';
  f.run();
  assert.deepEqual(Buffer.from(f.records()[0].blob, 'base64'), bytes);
  assert.equal(f.records()[0].owner, f.data.gateway_channel_records[0].owner);
  for (const line of fs.readFileSync(f.output, 'utf8').split('\n')) assert.ok(Buffer.byteLength(line) <= 131072);
});

test('empty but present collections produce a complete empty export', t => {
  const f = fixture(t);
  for (const name of collections) f.data[name] = [];
  f.run();
  assert.equal(fs.readFileSync(f.output, 'utf8'), '');
  assert.deepEqual(f.closed, collections);
  assert.deepEqual(f.printed, ['Exported 0 channel, 0 connection, 0 interaction sealed records']);
});

for (const missing of collections) {
  test(`rejects missing ${missing} instead of silently exporting an incomplete database`, t => {
    const f = fixture(t);
    f.rejects({ names: collections.filter(name => name !== missing) });
    assert.deepEqual(f.queried, []);
  });
}

const malformed = {
  'missing ID': { _id: undefined },
  'noncanonical UUID': { _id: '00000000000040008000000000000001' },
  'uppercase UUID': { _id: 'AAAAAAAA-0000-4000-8000-000000000001' },
  'empty owner': { owner: '' },
  'nonstring owner': { owner: 17 },
  'oversized UTF-8 owner': { owner: '界'.repeat(171) },
  'invalid Unicode owner': { owner: '\ud800' },
  'missing schema': { document_schema_version: undefined },
  'unsupported schema': { document_schema_version: long(2) },
  'string schema': { document_schema_version: '1' },
  'missing version': { version: undefined },
  'zero version': { version: 0 },
  'negative version': { version: long(-1) },
  'fractional version': { version: 1.5 },
  'nonfinite version': { version: Infinity },
  'rounded numeric version': { version: Number('9007199254740993') },
  'out-of-range i64 version': { version: long('9223372036854775808') },
  'oversized decimal version': { version: long('1'.repeat(128)) },
  'string version': { version: '1' },
  'decimal BSON version': { version: { _bsontype: 'Decimal128', toString: () => '1' } },
  'missing blob': { blob: undefined },
  'raw buffer blob': { blob: Buffer.from([1]) },
  'empty blob': { blob: binary([]) },
  'oversized blob': { blob: binary(Buffer.alloc(65537)) },
  'non-generic blob subtype': { blob: { ...binary([1]), sub_type: 4 } },
  'inconsistent binary length': { blob: { ...binary([1]), length: () => 2 } },
  'nonbyte binary value': { blob: { ...binary([1]), value: () => 'a' } },
  'unexpected connection expiry': { expires_at: new Date(1000) },
};
for (const [name, changes] of Object.entries(malformed)) {
  test(`rejects ${name} and removes earlier valid rows`, t => {
    const f = fixture(t);
    f.data.gateway_connection_records = [row(changes)];
    f.rejects();
    assert.deepEqual(f.closed, collections.slice(0, 2));
  });
}

for (const [name, expires_at] of Object.entries({
  missing: undefined, string: '2030-01-02T03:04:05Z', numeric: 1893553445000,
  invalid: new Date(NaN), zero: new Date(0), negative: new Date(-1000), fractional: new Date(1001),
})) {
  test(`rejects ${name} interaction expiry without publishing partial output`, t => {
    const f = fixture(t);
    f.data.gateway_interaction_records[0].expires_at = expires_at;
    f.rejects();
    assert.deepEqual(f.closed, collections);
  });
}

test('rejects repeated IDs within a domain', t => {
  const f = fixture(t);
  f.data.gateway_connection_records.push(row());
  f.rejects();
});

for (const failure of ['readFailure', 'closeFailure']) {
  test(`${failure} on final collection removes output and does not print driver details`, t => {
    const f = fixture(t);
    f.rejects({ [failure]: collections[2] });
    assert.deepEqual(f.closed, collections);
  });
}

for (const operation of ['writeFileSync', 'fsyncSync', 'closeSync', 'linkSync']) {
  test(`${operation} failure removes staging data and does not publish a success file`, t => {
    const f = fixture(t);
    f.rejects({ fileSystem: { [operation]: (...args) => {
      if (operation === 'closeSync') fs.closeSync(...args);
      throw new Error('private-filesystem-detail');
    } } });
  });
}

for (const symlink of [false, true]) {
  test(`refuses to overwrite an existing ${symlink ? 'symlink' : 'file'}`, t => {
    const f = fixture(t);
    const target = symlink ? join(f.directory, 'existing.ndjson') : f.output;
    fs.writeFileSync(target, 'preserve existing export');
    if (symlink) fs.symlinkSync(target, f.output);
    assert.throws(() => f.run(), /Gateway export failed/);
    assert.equal(fs.readFileSync(target, 'utf8'), 'preserve existing export');
    assert.equal(fs.lstatSync(f.output).isSymbolicLink(), symlink);
    assert.deepEqual(fs.readdirSync(f.directory).sort(), symlink ? ['existing.ndjson', 'export.ndjson'] : ['export.ndjson']);
    assert.deepEqual(f.printed, []);
  });
}

test('requires an explicit export destination before reading data', t => {
  const f = fixture(t);
  assert.throws(() => f.run({ environment: {} }), /CMA_GATEWAY_EXPORT_FILE is required/);
  assert.deepEqual(f.queried, []);
  assert.deepEqual(fs.readdirSync(f.directory), []);
});
