// Fixed, bounded document reader for synthetic Launch admission only.
'use strict';

const COLLECTIONS = ['trigger_launches', 'trigger_launch_input_protocols', 'visitor_launch_days'];

function collect(database) {
  const collections = {};
  for (const name of COLLECTIONS) {
    const result = database.runCommand({find: name, filter: {}, sort: {_id: 1},
      limit: 17, batchSize: 17, singleBatch: true, maxTimeMS: 2000,
      readConcern: {level: 'majority'}});
    const cursor = result && result.cursor;
    // mongosh rewrites optional method calls and can lose BSON Long's receiver.
    // Keep the method invocation explicit so the cursor ID retains its type.
    if (!result || Number(result.ok) !== 1 || !cursor || cursor.id == null
        || cursor.id.toString() !== '0' || cursor.ns !== 'matrix.' + name
        || !Array.isArray(result.cursor.firstBatch) || result.cursor.firstBatch.length > 16) {
      throw new Error('incomplete or excessive Launch document snapshot');
    }
    collections[name] = result.cursor.firstBatch;
  }
  return {schema: 'cma-migration-launch-documents-v1', database: 'matrix', collections};
}

if (typeof module !== 'undefined') module.exports = {collect, COLLECTIONS};
if (typeof process !== 'undefined' && process.env.MATRIX_LAUNCH_READ_URI) {
  let connection;
  try {
    connection = new Mongo(process.env.MATRIX_LAUNCH_READ_URI);
    const value = EJSON.stringify(collect(connection.getDB('matrix')), {relaxed: false});
    if (value.length > 1048576) throw new Error('Launch snapshot exceeds byte bound');
    print(value);
  } catch (_error) {
    print('migration Launch document collection failed');
    quit(1);
  } finally {
    if (connection) connection.close();
  }
}
