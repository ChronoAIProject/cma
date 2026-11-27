// This runs only in the disposable prepared fixture. The marker gives cleanup
// exact custody even if the client loses its reply after the synthetic write.
(async (scope, admin, d) => {
  const marker = 'ci_prepared_schema_race';
  const schema = 'document_schema_version';
  const cleanup = 'owned_workspace_cleanup_admission_v1';
  const schemaChange = scope.barrier === 'worktree_schema';
  const leaseChange = scope.barrier === 'work_lease';
  const ownerFence = scope.barrier === 'owner_fence';
  const publicationConflict = ownerFence || scope.barrier === 'publication_conflict';
  const parentEnvelope = scope.barrier === 'parent_envelope';
  const parentPause = scope.barrier === 'parent_pause';

  const field = schemaChange ? schema : cleanup;
  const successor = schemaChange ? NumberLong('999') : true;
  const operationId = scope.operationId;
  const claim = {'record.ownership.operation_id': operationId, kind: 'reservation', state: 'claimed'};

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
      ...claim, 'record.ownership.owner': agent.owner});
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
      work_lease_changed: agent[marker]?.operation === operationId && agent[marker]?.barrier === scope.barrier
        && agent.work_lease_expires_at === agent[marker].replacement
        && agent.record.work.lease?.expires_at === agent[marker].replacement,
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
    const collection = ownerFence ? 'cma_agent_manager_owners' : 'worktrees';
    const active = await admin.aggregate([{$currentOp: {allUsers: true}}, {$match: {
      appName: scope.app, active: true, 'command.update': collection, 'command.autocommit': false,
    }}]).toArray();
    let observed = null;
    for (const {command, opid} of active) {
      // Mongo omits the OP_MSG update sequence from currentOp. The
      // earlier reads capture the live transaction's exact target.
      if (!command.lsid || command.txnNumber == null) continue;
      const expected = ownerFence ? [['agents', rows.agent._id]] : [
        ['worktrees', rows.tree._id], ['sandboxes', rows.sandbox._id],
        ['workspaces', rows.workspace._id], ['native_prepared_sandboxes', rows.reservation._id],
      ];
      const reads = await d.getCollection('system.profile').find({appName: scope.app, 'command.lsid': command.lsid,
        'command.txnNumber': command.txnNumber, 'command.autocommit': false,
        $or: expected.map(([collection, id]) => ({'command.find': collection, 'command.filter._id': id})),
      }, {command: 1}).limit(16).toArray();
      if (new Set(reads.map(row => row.command.find)).size === expected.length) {
        if (observed !== null) throw Error('prepared_schema_race_multiple_bound_transactions');
        observed = {opid, session: command.lsid.id.toString(), transaction: command.txnNumber.toString()};
      }
    }
    return observed;
  }

  async function samplePausePublication() {
    const rows = await lineage(d);
    if (!rows) return null;
    // The prepared transaction reads the exact Agent and claimed reservation
    // before reading its Worktree and taking any parent write locks.
    const active = await admin.aggregate([{$currentOp: {allUsers: true}}, {$match: {
      appName: scope.app, active: true, 'command.find': 'worktrees', 'command.filter._id': rows.tree._id,
      'command.autocommit': false,
    }}]).toArray();
    const matches = [];
    for (const {command, opid} of active) {
      if (!command.lsid || command.txnNumber == null) continue;
      const reads = await d.getCollection('system.profile').find({
        appName: scope.app, 'command.autocommit': false,
        'command.lsid': command.lsid, 'command.txnNumber': command.txnNumber,
        $or: [{'command.find': 'agents', 'command.filter._id': rows.agent._id},
          {'command.find': 'native_prepared_sandboxes', 'command.filter._id': rows.reservation._id}],
      }, {command: 1}).limit(8).toArray();
      if (new Set(reads.map(row => row.command.find)).size === 2) {
        matches.push(require('crypto').createHash('sha256')
          .update(JSON.stringify([opid, command.lsid, command.txnNumber.toString()])).digest('hex'));
      }
    }
    if (matches.length > 1) throw Error('prepared_parent_pause_multiple_transactions');
    return matches.length === 1 ? {...proof(rows), sandboxStatus: rows.sandbox.status,
      transactionSha256: matches[0]} : null;
  }

  // A held read provides time to install the envelopes. It might be
  // another reader of this Sandbox, so qualification uses the worker's commit.
  async function hasHeldSandboxRead(rows) {
    const active = await admin.aggregate([{$currentOp: {allUsers: true}}, {$match: {
      appName: scope.app, active: true, 'command.find': 'sandboxes', 'command.filter._id': rows.sandbox._id,
      'command.autocommit': {$exists: false},
    }}, {$limit: 1}]).toArray();
    return active.length === 1;
  }

  async function replaceParentEnvelopes(rows) {
    const {sandbox, workspace, reservation} = rows;
    const [sandboxVersion, workspaceVersion] = scope.parentVersions;
    const installed = reservation.record.payload.installed_request_sha256;
    const parentRevisions = [sandbox, workspace].map(row => Number(row.revision) + 1);
    if (!parentRevisions.every(revision => Number.isSafeInteger(revision) && revision > 1)) {
      throw Error('prepared_parent_envelope_revision_invalid');
    }
    const session = await d.getMongo().startSession();
    try {
      await session.startTransaction({readConcern: {level: 'snapshot'}});
      const database = session.getDatabase(d.getName());
      for (const [collection, row, version, status, versions] of [
        ['sandboxes', sandbox, sandboxVersion, 'active', [1, 3, 4]],
        ['workspaces', workspace, workspaceVersion, 'ready', [1, 3]],
      ]) {
        const work = row.record.work;
        if (!versions.includes(Number(row[schema])) || row.status !== status || work.lease != null
            || Number(row.revision) !== Number(row.record.revision)) {
          throw Error('prepared_parent_envelope_not_idle');
        }
        const set = {[schema]: NumberLong(String(version))};
        const unset = {};
        if (version === 1) unset['record.work.authority_protocol'] = '';
        else set['record.work.authority_protocol'] = 'renewed_v1';
        if (collection === 'sandboxes') {
          if (version === 4) {
            if (!/^[a-f0-9]{64}$/.test(installed ?? '')) throw Error('prepared_parent_installation_unbound');
            set['record.work.prepared_installation_digest'] = installed;
          } else unset['record.work.prepared_installation_digest'] = '';
        }
        const filter = {_id: row._id, owner: rows.agent.owner, status, [schema]: row[schema], revision: row.revision,
          'record.work.lease': null, agent_prepared_open_ready_fence: {$exists: false},
          [cleanup]: {$exists: false}, channel_reuse_cleanup_execution: {$exists: false}};
        const update = {$set: set, $inc: {revision: NumberLong('1'), 'record.revision': NumberLong('1')}};
        if (Object.keys(unset).length) update.$unset = unset;
        const result = await database[collection].updateOne(filter, update, {maxTimeMS: 700});
        if (result.modifiedCount !== 1) throw Error('prepared_parent_envelope_write_missed');
      }
      await session.commitTransaction();
      return parentRevisions;
    } finally {
      await session.endSession();
    }
  }

  async function rawAgent(agentId) {
    const raw = await d.agents.findOne({_id: agentId}, undefined, {raw: true});
    if (!Buffer.isBuffer(raw)) throw Error('prepared_owner_fence_agent_bson_unavailable');
    return raw;
  }

  async function inject() {
    const deadline = Date.now() + 4500;
    while (Date.now() < deadline) {
      const rows = await lineage(d);
      const openDebt = rows?.agent.record.work.cleanup_debt;
      const readyForOwnerFence = !ownerFence || (Array.isArray(openDebt) && openDebt.length === 1
        && openDebt[0].step === 'open_conversation'
        && openDebt[0].operation_id === `agent:${rows.agent._id}:open:v1`);
      const bound = rows && readyForOwnerFence
        && (parentEnvelope ? await hasHeldSandboxRead(rows) : await currentUpdate(rows));
      if (bound) {
        const before = proof(rows);
        if (!pending(before) || before.schema !== 1 || before.cleanup_marked || rows.tree.status !== 'creating') {
          throw Error('prepared_schema_race_startup_precondition');
        }
        if (parentEnvelope) {
          const parentRevisions = await replaceParentEnvelopes(rows);
          return {...before, parentVersions: scope.parentVersions, parentRevisions};
        }
        if (publicationConflict) {
          const reservation = rows.reservation;
          const lease = rows.agent.record.work.lease;
          const leaseDeadlineMs = Date.parse(lease?.expires_at);
          const workFence = Number(lease?.fence);
          const owner = await d.cma_agent_manager_owners.findOne({_id: rows.agent._id,
            instance_id: {$type: 'string'}, $expr: {$gt: ['$expires_at', '$$NOW']}});
          const ownerEpoch = Number(owner?.epoch);
          if (!owner?.instance_id || !Number.isSafeInteger(ownerEpoch) || ownerEpoch < 1
              || !Number.isFinite(leaseDeadlineMs) || leaseDeadlineMs <= Date.now()
              || rows.agent.work_lease_expires_at !== lease?.expires_at
              || typeof lease?.holder !== 'string' || !lease.holder
              || rows.agent.work_lease_holder !== lease.holder
              || !Number.isSafeInteger(workFence) || workFence < 1
              || Number(rows.agent.work_lease_fence) !== workFence) {
            throw Error('prepared_publication_original_authority_unbound');
          }
          const originalAgent = ownerFence ? await rawAgent(rows.agent._id) : null;
          const mutation = {$set: {[marker]: {operation: operationId, barrier: scope.barrier}}};
          const collection = ownerFence ? d.cma_agent_manager_owners : d.native_prepared_sandboxes;
          // Both writes change the transaction snapshot while keeping authority intact.
          const filter = ownerFence ? {_id: owner._id, instance_id: owner.instance_id,
            epoch: owner.epoch, effect_revision: owner.effect_revision, [marker]: {$exists: false}} : {
            ...claim, _id: reservation._id, revision: reservation.revision,
            'record.ownership.owner': rows.agent.owner, [marker]: {$exists: false},
            agent_prepared_open_ready_fence: {$exists: false},
          };
          const changed = await collection.updateOne(filter, mutation, {maxTimeMS: 700});
          if (changed.modifiedCount !== 1) throw Error('prepared_publication_conflict_write_missed');
          const agentUnchanged = !ownerFence || originalAgent.equals(await rawAgent(rows.agent._id));
          const after = await currentUpdate(rows);
          if (after?.opid !== bound.opid || after?.session !== bound.session
              || after?.transaction !== bound.transaction) {
            throw Error('prepared_publication_conflict_transaction_changed');
          }
          const workLeaseSha256 = require('crypto').createHash('sha256')
            .update(`${workFence}:${lease.holder}`).digest('hex');
          return {...before, ownerInstance: owner.instance_id, ownerEpoch, leaseDeadlineMs, workLeaseSha256,
            agentUnchanged};
        }
        if (leaseChange) {
          await replaceLease(rows.agent);
          return before;
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

  async function replaceLease(agent) {
    const lease = agent.record.work.lease;
    const replacement = new Date(Date.now() + 45000).toISOString().replace(/\.\d{3}Z$/, 'Z');
    if (!lease || typeof lease.expires_at !== 'string' || Date.parse(lease.expires_at) <= Date.parse(replacement)
        || agent.work_lease_expires_at !== lease.expires_at) {
      throw Error('prepared_schema_race_work_lease_unbound');
    }
    const changed = await d.agents.updateOne({
      _id: agent._id, revision: agent.revision, [marker]: {$exists: false},
      work_lease_holder: lease.holder, work_lease_fence: lease.fence, work_lease_expires_at: lease.expires_at,
      'record.work.lease.expires_at': lease.expires_at,
    }, {$set: {work_lease_expires_at: replacement, 'record.work.lease.expires_at': replacement,
      [marker]: {operation: operationId, barrier: scope.barrier, previous: lease.expires_at, replacement,
        holder: lease.holder, fence: lease.fence}}}, {maxTimeMS: 700});
    if (changed.modifiedCount !== 1) throw Error('prepared_schema_race_concurrent_write_missed');
  }

  async function read() {
    const session = await d.getMongo().startSession();
    try {
      await session.startTransaction({readConcern: {level: 'snapshot'}});
      const rows = await lineage(session.getDatabase(d.getName()));
      if (parentPause) return rows && {...proof(rows), sandboxStatus: rows.sandbox.status};
      const observed = rows && (leaseChange ? rows.agent[marker] : rows.tree[marker]);
      if (observed?.operation !== operationId || observed?.barrier !== scope.barrier) {
        throw Error('prepared_schema_race_marker_missing');
      }
      const value = proof(rows);
      if (value.schema !== (schemaChange ? Number(successor) : 1)
          || value.cleanup_marked !== (!schemaChange && !leaseChange)
          || value.work_lease_changed !== leaseChange || !pending(value)) {
        throw Error('prepared_schema_race_partial_publication');
      }
      return value;
    } finally {
      await session.endSession();
    }
  }

  async function restore() {
    if (leaseChange) return restoreLease();
    if (ownerFence) {
      const rows = await lineage(d);
      if (!rows) return {restored: 0};
      const restored = await d.cma_agent_manager_owners.updateOne({_id: rows.agent._id,
        [`${marker}.operation`]: operationId, [`${marker}.barrier`]: scope.barrier,
      }, {$unset: {[marker]: ''}}, {maxTimeMS: 700});
      return {restored: restored.modifiedCount};
    }
    if (publicationConflict) {
      const restored = await d.native_prepared_sandboxes.updateOne({
        ...claim, _id: {$in: scope.reservations},
        [`${marker}.operation`]: operationId, [`${marker}.barrier`]: scope.barrier,
      }, {$unset: {[marker]: ''}}, {maxTimeMS: 700});
      return {restored: restored.modifiedCount};
    }
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

  async function restoreLease() {
    const rows = await d.agents.find({[`${marker}.operation`]: operationId, sandbox_id: {$in: scope.reservations}})
      .limit(2).toArray();
    if (!rows.length) return {restored: 0};
    const [agent] = rows;
    const found = agent[marker];
    if (rows.length !== 1 || found.barrier !== scope.barrier || typeof found.previous !== 'string'
        || !(Date.parse(found.previous) > Date.parse(found.replacement))) {
      throw Error('prepared_schema_race_restore_custody');
    }
    const restored = await d.agents.updateOne({
      _id: agent._id, revision: agent.revision, [`${marker}.operation`]: operationId,
      [`${marker}.barrier`]: scope.barrier, work_lease_holder: found.holder, work_lease_fence: found.fence,
      work_lease_expires_at: found.replacement, 'record.work.lease.expires_at': found.replacement,
    }, {$set: {work_lease_expires_at: found.previous, 'record.work.lease.expires_at': found.previous},
      $unset: {[marker]: ''}});
    if (restored.modifiedCount !== 1) throw Error('prepared_schema_race_restore_conflict');
    return {restored: 1};
  }

  if (scope.action === 'sample') print(JSON.stringify(await samplePausePublication()));
  if (scope.action === 'inject') print(JSON.stringify(await inject()));
  if (scope.action === 'read') print(JSON.stringify(await read()));
  if (scope.action === 'restore') print(JSON.stringify(await restore()));
})(scope, admin, d);
