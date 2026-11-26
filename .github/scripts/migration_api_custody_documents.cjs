// Bounded, find-only observation of encrypted API custody. No decrypt authority.
'use strict';
const COLLECTION = 'auth_api_bearers';
function collect(database) {
  const result = database.runCommand({find: COLLECTION, filter: {}, sort: {_id: 1},
    limit: 5, batchSize: 5, singleBatch: true, maxTimeMS: 2000,
    readConcern: {level: 'majority'}});
  const cursor = result && result.cursor;
  if (!result || Number(result.ok) !== 1 || !cursor || cursor.id == null
      || cursor.id.toString() !== '0' || cursor.ns !== 'matrix.' + COLLECTION
      || !Array.isArray(cursor.firstBatch) || cursor.firstBatch.length > 4) {
    throw new Error('incomplete or excessive API custody snapshot');
  }
  return {schema: 'cma-migration-api-custody-documents-v1', database: 'matrix',
    collections: {[COLLECTION]: cursor.firstBatch}};
}
if (typeof module !== 'undefined') module.exports = {collect};
if (typeof process !== 'undefined' && process.env.MATRIX_API_CUSTODY_READ_URI) {
  let connection;
  try {
    connection = new Mongo(process.env.MATRIX_API_CUSTODY_READ_URI);
    const raw = EJSON.stringify(collect(connection.getDB('matrix')), {relaxed: false});
    if (raw.length > 1048576) throw new Error('API custody snapshot exceeds byte bound');
    print(raw);
  } catch (_error) {
    print('migration API custody collection failed');
    quit(1);
  } finally {
    if (connection) connection.close();
  }
}
