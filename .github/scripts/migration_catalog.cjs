// Read catalog metadata only. Document contents are never queried or printed.
'use strict';

function collectCatalog(database) {
  function command(value) {
    const result = database.runCommand(value);
    if (!result || Number(result.ok) !== 1) throw new Error('catalog command failed');
    return result;
  }
  function drain(request, limit) {
    let cursor;
    const rows = [];
    try {
      cursor = command(request).cursor;
      let batchKey = 'firstBatch';
      let pages = 0;
      while (true) {
        if (++pages > limit + 1) throw new Error('catalog cursor did not finish');
        if (!cursor || cursor.id === undefined || cursor.id === null
            || typeof cursor.ns !== 'string' || !cursor.ns.startsWith('matrix.')
            || !Array.isArray(cursor[batchKey])) throw new Error('invalid catalog cursor');
        rows.push(...cursor[batchKey]);
        if (rows.length > limit) throw new Error('catalog inventory exceeds bound');
        if (cursor.id.toString() === '0') return rows;
        const next = command({getMore: cursor.id, collection: cursor.ns.slice(7), batchSize: 64});
        if (next.cursor?.ns !== cursor.ns) throw new Error('catalog cursor namespace changed');
        cursor = next.cursor;
        batchKey = 'nextBatch';
      }
    } finally {
      if (cursor?.id !== undefined && cursor.id !== null
          && cursor.id.toString() !== '0' && cursor.ns?.startsWith('matrix.')) {
        command({killCursors: cursor.ns.slice(7), cursors: [cursor.id]});
      }
    }
  }
  const entries = drain({listCollections: 1, nameOnly: false,
    cursor: {batchSize: 64}, maxTimeMS: 2000}, 512);
  if (entries.length === 0) throw new Error('empty catalog');
  const names = new Set();
  const collections = entries.map(entry => {
    if (!entry || typeof entry.name !== 'string' || !entry.name || names.has(entry.name)
        || !['collection', 'view', 'timeseries'].includes(entry.type)
        || !entry.options || typeof entry.options !== 'object' || Array.isArray(entry.options)) {
      throw new Error('invalid catalog collection');
    }
    names.add(entry.name);
    const uuid = entry.info?.uuid ?? null;
    if (entry.type === 'collection' && uuid === null) throw new Error('collection UUID missing');
    const indexes = entry.type === 'view' ? [] : drain({listIndexes: entry.name,
      includeBuildUUIDs: true, cursor: {batchSize: 64}, maxTimeMS: 2000}, 256).map(index => {
      if (!index || index.spec || index.buildUUID || typeof index.name !== 'string'
          || !index.name || !index.key || typeof index.key !== 'object'
          || Array.isArray(index.key) || Object.keys(index.key).length === 0) {
        throw new Error('invalid or unfinished index');
      }
      const {ns: _namespace, ...definition} = index;
      // BSON compound index field order is semantic; JSON object equality is not.
      return {...definition, key: Object.entries(index.key)};
    });
    if (entry.type !== 'view' && indexes.length === 0) throw new Error('indexes missing');
    if (new Set(indexes.map(index => index.name)).size !== indexes.length) {
      throw new Error('duplicate index name');
    }
    return {name: entry.name, type: entry.type, uuid, options: entry.options,
      indexes: indexes.sort((a, b) => a.name.localeCompare(b.name))};
  });
  return {schema: 'cma-migration-catalog-v1', database: 'matrix',
    collections: collections.sort((a, b) => a.name.localeCompare(b.name))};
}

if (typeof module !== 'undefined') module.exports = {collectCatalog};
if (typeof Mongo !== 'undefined') {
  let connection;
  try {
    connection = new Mongo(process.env.MATRIX_CATALOG_URI);
    print(EJSON.stringify(collectCatalog(connection.getDB('matrix')), {relaxed: false}));
  } catch {
    // Do not expose a connection string or any server payload on failure.
    print('migration catalog collection failed');
    quit(1);
  } finally {
    if (connection) connection.close();
  }
}
