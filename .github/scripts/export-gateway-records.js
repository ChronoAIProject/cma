// Run with mongosh using a temporary, read-only legacy migration credential.
// Management writes must be quiesced until the destination import is verified.
// The export contains sealed records only; the gateway keeps its existing keys.
const fs = require('fs');
const output = process.env.CMA_GATEWAY_EXPORT_FILE;
if (!output) throw new Error('CMA_GATEWAY_EXPORT_FILE is required');
const fd = fs.openSync(output, 'wx', 0o600);
let count = 0;
try {
  for (const row of db.getCollection('gateway_channel_records').find({}).sort({ _id: 1 })) {
    if (!row.blob || !row.owner || !row.version || row.document_schema_version.toString() !== '1') {
      throw new Error('Unsupported legacy record; export is incomplete');
    }
    fs.writeSync(fd, JSON.stringify({
      record_id: row._id, owner: row.owner, version: row.version.toString(),
      blob: Buffer.from(row.blob.buffer).toString('base64'),
    }) + '\n');
    count++;
  }
  fs.fsyncSync(fd);
} finally { fs.closeSync(fd); }
print(`Exported ${count} sealed records`);
