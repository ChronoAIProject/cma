// This runs only in the disposable prepared fixture. The marker gives cleanup
// exact custody even if the client loses its reply after the synthetic write.
(async (scope, admin, d) => {
  const marker = 'ci_prepared_schema_race';
  const schema = 'document_schema_version';
  const cleanup = 'owned_workspace_cleanup_admission_v1';
  const schemaChange = scope.barrier === 'worktree_schema';
  const field = schemaChange ? schema : cleanup;
  const successor = schemaChange ? NumberLong('999') : true;
  const operationId = scope.operationId;

  async function lineage(database) {
    const agent = await database.agents.findOne({'record.origin.operation_id': operationId,
      sandbox_id: {$in: scope.reservations}});
    if (!agent) return null;
    const sandbox = await database.sandboxes.findOne({_id: agent.sandbox_id, owner: agent.owner,
      'record.origin.operation_id': operationId});
    const workspace = await database.workspaces.findOne({_id: agent.workspace_id, sandbox_id: agent.sandbox_id, owner: agent.owner});
    const tree = await database.worktrees.findOne({_id: agent.worktree_id, sandbox_id: agent.sandbox_id,
      workspace_id: agent.workspace_id, owner: agent.owner});
    const reservation = await database.native_prepared_sandboxes.findOne({_id: agent.sandbox_id,
      kind: 'reservation', state: 'claimed', 'record.ownership.owner': agent.owner,
      'record.ownership.operation_id': operationId});
    if (!sandbox || !workspace || !tree || !reservation) return null;
    const workspaces = reservation.record.payload.workspaces;
    if (!Array.isArray(workspaces) || workspaces.length !== 1 || workspaces[0].id !== workspace._id) return null;
    const agents = workspaces[0].agents;
    if (!Array.isArray(agents) || agents.length !== 1 || agents[0].id !== agent._id
        || agents[0].worktree_id !== tree._id) return null;
    return {agent, sandbox, workspace, tree, reservation};
  }

  function proof(rows) {
    const {agent, sandbox, workspace, tree, reservation} = rows;
    return {
      agentId: agent._id, sandboxId: sandbox._id,
      schema: Number(tree[schema]), state: agent.status, step: agent.record.work.step,
      cleanup_marked: Object.hasOwn(tree, cleanup),
      receipt_count: agent.record.work.receipts.length,
      conversation_unpublished: agent.record.conversation.conversation_id == null,
      ready_fences_absent: [sandbox, workspace, tree, reservation]
        .every(row => !Object.hasOwn(row, 'agent_prepared_open_ready_fence')),
    };
  }

  function pending(value) {
    return value.state === 'starting' && value.step === 'open_conversation'
      && value.receipt_count === 2 && value.conversation_unpublished && value.ready_fences_absent;
  }

  async function currentUpdate(rows) {
    const active = await admin.aggregate([{$currentOp: {allUsers: true}}, {$match: {
      appName: scope.app, 'command.update': 'worktrees', 'command.autocommit': false,
    }}]).toArray();
    for (const {command} of active) {
      // Mongo omits the OP_MSG update sequence from currentOp. Bind the
      // live session to the exact prepared hierarchy it read before writing.
      if (!command.lsid || command.txnNumber == null) continue;
      const reads = await d.getCollection('system.profile').find({appName: scope.app, 'command.lsid': command.lsid,
        'command.txnNumber': command.txnNumber, 'command.autocommit': false, $or: [
          {'command.find': 'worktrees', 'command.filter._id': rows.tree._id},
          {'command.find': 'sandboxes', 'command.filter._id': rows.sandbox._id},
          {'command.find': 'workspaces', 'command.filter._id': rows.workspace._id},
          {'command.find': 'native_prepared_sandboxes', 'command.filter._id': rows.reservation._id},
        ]}, {command: 1}).limit(16).toArray();
      if (new Set(reads.map(row => row.command.find)).size === 4) return true;
    }
    return false;
  }

  async function inject() {
    const deadline = Date.now() + 4500;
    while (Date.now() < deadline) {
      const rows = await lineage(d);
      if (rows && await currentUpdate(rows)) {
        const before = proof(rows);
        if (!pending(before) || before.schema !== 1 || before.cleanup_marked || rows.tree.status !== 'creating') {
          throw Error('prepared_schema_race_startup_precondition');
        }
        const changed = await d.worktrees.updateOne({_id: rows.tree._id, revision: rows.tree.revision,
          [schema]: rows.tree[schema], [marker]: {$exists: false},
          [cleanup]: {$exists: false},
          agent_prepared_open_ready_fence: {$exists: false}},
        {$set: {[field]: successor, [marker]: {operation: operationId, barrier: scope.barrier,
          previous: rows.tree[schema]}}}, {maxTimeMS: 700});
        if (changed.modifiedCount !== 1) throw Error('prepared_schema_race_concurrent_write_missed');
        return before;
      }
      await new Promise(resolve => setTimeout(resolve, 60));
    }
    return null;
  }

  async function read() {
    const session = await d.getMongo().startSession();
    try {
      await session.startTransaction({readConcern: {level: 'snapshot'}});
      const rows = await lineage(session.getDatabase(d.getName()));
      if (!rows || rows.tree[marker]?.operation !== operationId || rows.tree[marker]?.barrier !== scope.barrier) {
        throw Error('prepared_schema_race_marker_missing');
      }
      const value = proof(rows);
      if (value.schema !== (schemaChange ? Number(successor) : 1)
          || value.cleanup_marked !== !schemaChange || !pending(value)) {
        throw Error('prepared_schema_race_partial_publication');
      }
      return value;
    } finally {
      await session.endSession();
    }
  }

  async function restore() {
    const rows = await d.worktrees.find({[`${marker}.operation`]: operationId,
      sandbox_id: {$in: scope.reservations}}).limit(2).toArray();
    if (!rows.length) return {restored: 0};
    const [tree] = rows;
    if (rows.length !== 1 || Number(tree[marker]?.previous) !== 1 || tree[marker]?.barrier !== scope.barrier) {
      throw Error('prepared_schema_race_restore_custody');
    }
    const update = schemaChange
      ? {$set: {[schema]: tree[marker].previous}, $unset: {[marker]: ''}}
      : {$unset: {[cleanup]: '', [marker]: ''}};
    const restored = await d.worktrees.updateOne({_id: tree._id, [field]: successor,
      [`${marker}.operation`]: operationId, [`${marker}.barrier`]: scope.barrier}, update);
    if (restored.modifiedCount !== 1) throw Error('prepared_schema_race_restore_conflict');
    return {restored: 1};
  }

  if (scope.action === 'inject') print(JSON.stringify(await inject()));
  if (scope.action === 'read') print(JSON.stringify(await read()));
  if (scope.action === 'restore') print(JSON.stringify(await restore()));
})(scope, admin, d);
