// Run with mongosh using a temporary, read-only legacy migration credential.
// All legacy Gateway writers must be quiesced until the import is verified.
// The export contains sealed records only; the gateway keeps its existing keys.
const fs = require('node:fs');
const path = require('node:path');
const output = process.env.CMA_GATEWAY_EXPORT_FILE;
if (!output) throw new Error('CMA_GATEWAY_EXPORT_FILE is required');
const domains = [
  ['channel', 'gateway_channel_records'],
  ['connection', 'gateway_connection_records'],
  ['interaction', 'gateway_interaction_records'],
];
const invalid = () => { throw new Error('Unsupported legacy record; export is incomplete'); };

function positiveInteger(value) {
  // mongosh can promote small BSON integers to numbers. Never round a Long
  // through Number: record revisions must survive beyond MAX_SAFE_INTEGER.
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) invalid();
  } else if (!value || !['Long', 'Int32'].includes(value._bsontype)) invalid();
  const decimal = value.toString();
  if (!/^[1-9][0-9]{0,18}$/.test(decimal) || BigInt(decimal) > 9223372036854775807n) invalid();
  return decimal;
}

function exportRecord(kind, row) {
  if (!row || typeof row._id !== 'string'
      || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(row._id)
      || typeof row.owner !== 'string' || !row.owner.length
      || Buffer.byteLength(row.owner, 'utf8') > 512
      || Buffer.from(row.owner, 'utf8').toString('utf8') !== row.owner
      || positiveInteger(row.document_schema_version) !== '1') invalid();
  const version = positiveInteger(row.version);
  const binary = row.blob;
  if (!binary || binary._bsontype !== 'Binary' || binary.sub_type !== 0) invalid();
  const length = binary.length();
  if (!Number.isSafeInteger(length) || length < 1 || length > 65536) invalid();
  // Binary.buffer can contain unused capacity; value(true) returns only the
  // logical ciphertext bytes, including leading/trailing zero bytes.
  const bytes = binary.value(true);
  if (!ArrayBuffer.isView(bytes) || bytes.BYTES_PER_ELEMENT !== 1 || bytes.byteLength !== length) invalid();
  const record = { kind, record_id: row._id, owner: row.owner, version, blob: Buffer.from(bytes).toString('base64') };
  if (kind === 'interaction') {
    // The legacy writer stores seconds * 1000 as BSON Date; the importer takes
    // seconds. Refuse invalid or fractional dates instead of losing precision.
    const millis = Date.prototype.getTime.call(row.expires_at);
    if (!Number.isSafeInteger(millis) || millis <= 0 || millis % 1000 !== 0) invalid();
    record.expires_at = millis / 1000;
  } else if (Object.hasOwn(row, 'expires_at')) invalid();
  return record;
}

let temporaryDirectory;
let fd;
const counts = { channel: 0, connection: 0, interaction: 0 };
try {
  const collections = db.getCollectionNames();
  if (domains.some(([, collection]) => !collections.includes(collection))) {
    throw new Error('Missing legacy collection; export is incomplete');
  }
  // A private staging directory in the destination filesystem keeps partial
  // exports separate. Hard-link publication atomically refuses any existing
  // destination, including a symlink; rename would silently overwrite it.
  temporaryDirectory = fs.mkdtempSync(path.join(path.dirname(output), '.cma-gateway-export-'));
  const temporaryFile = path.join(temporaryDirectory, 'records.ndjson');
  fd = fs.openSync(temporaryFile, 'wx', 0o600);
  for (const [kind, collection] of domains) {
    const cursor = db.getCollection(collection).find({}).sort({ _id: 1 }).batchSize(100);
    let previousId;
    try {
      // mongosh cursors expose hasNext/next, not JavaScript iteration.
      while (cursor.hasNext()) {
        const row = cursor.next();
        const record = exportRecord(kind, row);
        if (previousId !== undefined && record.record_id <= previousId) invalid();
        previousId = record.record_id;
        fs.writeFileSync(fd, JSON.stringify(record) + '\n');
        counts[kind]++;
      }
    } finally {
      cursor.close();
    }
  }
  fs.fsyncSync(fd);
  fs.closeSync(fd);
  fd = undefined;
  fs.linkSync(temporaryFile, output);
} catch {
  // Driver/filesystem errors can contain credentials, URIs or record values.
  throw new Error('Gateway export failed; check source collections, record format and output availability');
} finally {
  // Cleanup errors must not expose driver data or mask the original failure.
  // A killed process can leave a private staging directory, never a partial
  // file at the requested destination.
  if (fd !== undefined) { try { fs.closeSync(fd); } catch {} }
  if (temporaryDirectory) { try { fs.rmSync(temporaryDirectory, { recursive: true }); } catch {} }
}
print(`Exported ${counts.channel} channel, ${counts.connection} connection, ${counts.interaction} interaction sealed records`);
