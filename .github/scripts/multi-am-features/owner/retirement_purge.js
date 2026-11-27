(async () => {
  const {sandboxId, agentId, operationId, arm} = scope;
  const owner = 'native-am-fixture-owner';
  const session = await d.getMongo().startSession();
  try {
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const target = {_id: sandboxId, owner, origin_operation_id: operationId};
    const sandbox = await snapshot.sandboxes.findOne(target, {
      status: 1, revision: 1, purge_after: 1, work_ready_at: 1, work_lease_expires_at: 1,
      last_seq: 1, 'record.runtime': 1, 'record.work.step': 1,
    });
    const children = await snapshot.agents.find({sandbox_id: sandboxId}, {
      status: 1, owner: 1, 'record.origin.operation_id': 1,
    }).limit(3).toArray();
    const tail = collection => snapshot.getCollection(collection).findOne({sandbox_id: sandboxId}, {_id: 1});
    const event = await tail('sandbox_events');
    const workspace = await tail('workspaces');
    const worktree = await tail('worktrees');
    const agentEvent = await snapshot.agent_log.findOne({agent_id: agentId}, {_id: 1});
    const creation = await snapshot.agent_creation_provenance.findOne({_id: agentId,
      'record.agent_id': agentId, 'record.sandbox_id': sandboxId, 'record.owner.kind': 'person',
      'record.owner.subject': owner, 'record.provisioning_origin.type': 'lifecycle',
      'record.provisioning_origin.operation_id': operationId});
    if (!creation) throw new Error('retirement_creation_provenance_missing');
    const creationDigest = require('crypto').createHash('sha256').update(EJSON.stringify(creation)).digest('hex');
    if (!arm) {
      const foreign = !sandbox && await snapshot.sandboxes.findOne({_id: sandboxId}, {_id: 1});
      print(JSON.stringify({present: Boolean(sandbox || foreign), agents: children.length,
        sandbox_events: Boolean(event), agent_events: Boolean(agentEvent),
        workspaces: Boolean(workspace), worktrees: Boolean(worktree), creation_sha256: creationDigest}));
      return;
    }

    const initial = sandbox?.purge_after;
    const now = Date.now();
    if (sandbox?.status !== 'retired' || sandbox.record.runtime != null
        || sandbox.record.work?.step !== 'wait_for_purge' || sandbox.work_lease_expires_at != null
        || typeof initial !== 'string' || Date.parse(initial) <= now || !Number.isFinite(Date.parse(initial))
        || sandbox.work_ready_at !== initial || !event || !agentEvent || !workspace || !worktree
        || children.length < 1 || children.length > 2
        || children.some(row => row.owner !== owner || row.status !== 'deleted')
        || !children.some(row => row._id === agentId && row.record?.origin?.operation_id === operationId)) {
      throw new Error('retirement_purge_terminal_lineage_required');
    }
    // Move only the elapsed-time inputs. The actual production worker still
    // proves custody, retention, title finality and the exact revision.
    const elapsed = new Date(now + 1000).toISOString().replace(/\.\d{3}Z$/, 'Z');
    const update = await snapshot.sandboxes.updateOne({...target, status: 'retired', revision: sandbox.revision,
      last_seq: sandbox.last_seq, purge_after: initial, work_ready_at: initial, work_lease_expires_at: null},
    {$set: {purge_after: elapsed, work_ready_at: elapsed, 'record.purge_after': elapsed, 'record.work.ready_at': elapsed}});
    if (update.modifiedCount !== 1) throw new Error('retirement_purge_schedule_not_advanced');
    await session.commitTransaction();
    print(JSON.stringify({armed: true, agents: children.length, creation_sha256: creationDigest}));
  } finally {
    await session.endSession();
  }
})();
