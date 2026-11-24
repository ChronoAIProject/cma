// Fixed majority reads. No writes, token material, dynamic collections or getMore.
'use strict';

const MEMBERS = 'cma_agent_manager_members';
const OWNERS = 'cma_agent_manager_owners';

function read(database, name, limit) {
  const reply = database.runCommand({find: name, filter: {}, sort: {_id: 1},
    limit, batchSize: limit, singleBatch: true, maxTimeMS: 2000,
    readConcern: {level: 'majority'}});
  const cursor = reply && reply.cursor;
  if (!reply || Number(reply.ok) !== 1 || !cursor || cursor.id == null
      || cursor.id.toString() !== '0' || cursor.ns !== 'matrix.' + name
      || !Array.isArray(cursor.firstBatch) || cursor.firstBatch.length >= limit) {
    throw new Error('incomplete or excessive membership snapshot');
  }
  return cursor.firstBatch;
}

function collect(database) {
  const members = read(database, MEMBERS, 17);
  // A nonempty owner collection means this is no longer the empty-Agent
  // membership fixture. Refuse it rather than claiming ownership coverage.
  const owners = read(database, OWNERS, 1);
  const clock = database.runCommand({hello: 1, maxTimeMS: 2000});
  if (!clock || Number(clock.ok) !== 1 || !(clock.localTime instanceof Date)
      || !Number.isFinite(clock.localTime.getTime())) throw new Error('missing server clock');
  return {schema: 'cma-migration-membership-v1', database: 'matrix',
    observed_at: clock.localTime, collections: {[MEMBERS]: members, [OWNERS]: owners}};
}

if (typeof module !== 'undefined') module.exports = {collect, MEMBERS, OWNERS};
if (typeof process !== 'undefined' && process.env.MATRIX_MEMBERSHIP_READ_URI) {
  let connection;
  try {
    connection = new Mongo(process.env.MATRIX_MEMBERSHIP_READ_URI);
    const raw = EJSON.stringify(collect(connection.getDB('matrix')), {relaxed: false});
    if (raw.length > 65536) throw new Error('membership snapshot exceeds byte bound');
    print(raw);
  } catch (_error) {
    print('migration membership collection failed');
    quit(1);
  } finally {
    if (connection) connection.close();
  }
}
