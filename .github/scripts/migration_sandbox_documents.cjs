// Fixed, bounded reader for Sandbox lifecycle control ledgers only.
'use strict';

const COLLECTIONS = ['sandbox_controls'];

function collect(database) {
  const collections = {};
  for (const name of COLLECTIONS) {
    const result = database.runCommand({find: name, filter: {}, sort: {_id: 1},
      limit: 32, batchSize: 32, singleBatch: true, maxTimeMS: 2000,
      readConcern: {level: 'majority'}});
    const cursor = result && result.cursor;
    if (!result || Number(result.ok) !== 1 || !cursor || cursor.id == null
        || cursor.id.toString() !== '0' || cursor.ns !== 'matrix.' + name
        || !Array.isArray(cursor.firstBatch) || cursor.firstBatch.length > 31) {
      throw new Error('incomplete or excessive Sandbox command snapshot');
    }
    collections[name] = cursor.firstBatch;
  }
  return {schema: 'cma-migration-sandbox-command-documents-v1', database: 'matrix', collections};
}

if (typeof module !== 'undefined') module.exports = {collect, COLLECTIONS};
if (typeof process !== 'undefined' && process.env.MATRIX_SANDBOX_COMMAND_READ_URI) {
  let connection;
  try {
    connection = new Mongo(process.env.MATRIX_SANDBOX_COMMAND_READ_URI);
    const value = EJSON.stringify(collect(connection.getDB('matrix')), {relaxed: false});
    if (value.length > 1048576) throw new Error('Sandbox command snapshot exceeds byte bound');
    print(value);
  } catch (_error) {
    print('migration Sandbox command collection failed');
    quit(1);
  } finally {
    if (connection) connection.close();
  }
}
