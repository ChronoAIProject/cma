(async () => {
  const controlId = 'agent-provenance-v1';
  const progress = d.agent_provenance_progress;
  const pristine = {_id: controlId, protocol: 1, phase: 'ready', revision: 0, owner: null,
    cursor: null, last_receipt: null, 'counts.scanned': 0};
  const now = new Date();
  if (scope.action === 'defer') {
    const deferred = await progress.updateOne({...pristine, next_sweep_at: new Date(0)},
      {$set: {next_sweep_at: new Date(now.getTime() + 2 * 60 * 60 * 1000)}});
    print(JSON.stringify({deferred: deferred.modifiedCount === 1}));
    return;
  }

  const {action, agentId, sandboxId, operationId, app} = scope;
  const owner = 'native-am-fixture-owner';
  const sha = value => require('crypto').createHash('sha256').update(value).digest('hex');
  if (action === 'sample') {
    const operations = await admin.aggregate([
      {$currentOp: {allUsers: true}},
      {$match: {active: true, appName: app, 'command.$db': d.getName(),
        'command.aggregate': 'agents', 'command.autocommit': false}},
      {$limit: 16},
    ]).toArray();
    const targets = operations.filter(({command}) => command.pipeline?.[0]?.$match?._id === agentId
      && command.pipeline.some(stage => stage.$lookup?.from === 'agent_creation_provenance'
        && stage.$lookup.as === 'creation' && stage.$lookup.pipeline?.[0]?.$match?._id === agentId));
    if (targets.length > 1) throw Error('provenance_progress_multiple_held_reads');
    const held = targets[0];
    print(JSON.stringify({transaction_sha256: held?.command?.lsid?.id && held.command.txnNumber != null
      ? sha([held.opid, held.command.lsid.id, held.command.txnNumber].map(String).join(':')) : null}));
    return;
  }

  const creationKey = {_id: agentId, 'record.agent_id': agentId, 'record.sandbox_id': sandboxId,
    'record.owner.kind': 'person', 'record.owner.subject': owner,
    'record.provisioning_origin.type': 'lifecycle', 'record.provisioning_origin.operation_id': operationId};
  const canonical = value => EJSON.stringify(value, {relaxed: false});
  const session = d.getMongo().startSession();
  try {
    session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const creation = snapshot.agent_creation_provenance;
    const control = snapshot.agent_provenance_progress;
    const row = await creation.findOne(creationKey);
    const agent = await snapshot.agents.findOne({_id: agentId, sandbox_id: sandboxId, owner, status: 'ready',
      'record.origin.type': 'lifecycle', 'record.origin.operation_id': operationId},
    {revision: 1, last_seq: 1});
    const sandbox = await snapshot.sandboxes.findOne({_id: sandboxId, owner, origin_operation_id: operationId,
      status: 'active'}, {_id: 1});
    const candidates = await snapshot.agents.find({}, {_id: 1}).limit(2).toArray();
    if (!agent || !sandbox || candidates.length !== 1 || candidates[0]._id !== agentId) {
      throw Error('provenance_progress_requires_sole_ready_agent');
    }

    if (action === 'original' || action === 'arm') {
      if (row?.record?.creation_origin?.kind !== 'manual' || !row.record.created_by) {
        throw Error('provenance_progress_original_creation_unbound');
      }
      if (action === 'original') {
        print(JSON.stringify(canonical(row)));
        return;
      }
      if (canonical(row) !== scope.original) throw Error('provenance_progress_original_creation_changed');
      const armed = await control.updateOne({...pristine, next_sweep_at: {$gt: now}},
        {$set: {next_sweep_at: now}});
      const removed = await creation.deleteOne(creationKey);
      if (armed.modifiedCount !== 1 || removed.deletedCount !== 1) throw Error('provenance_progress_not_armed');
      await session.commitTransaction();
      print(JSON.stringify({armed: true, agent_revision: Number(agent.revision)}));
      return;
    }

    const recovered = row?.record?.creation_origin?.kind === 'legacy_unknown' && row.record.created_by === null;
    if (action === 'observe') {
      const state = await control.findOne({_id: controlId, cursor: agentId});
      const counts = state?.counts;
      const recorded = Number(counts?.scanned) === 1 && Number(counts?.recorded) === 1
        && ['retained', 'skipped', 'errors'].every(key => Number(counts?.[key]) === 0);
      print(JSON.stringify({recorded: recovered && recorded, agent_revision: Number(agent.revision),
        sweep_finished: state?.phase === 'sweep_finished'}));
      return;
    }
    if (action !== 'restore') throw Error('provenance_progress_action_required');
    const original = EJSON.parse(scope.original, {relaxed: false});
    if (original?._id !== agentId || original.record?.creation_origin?.kind !== 'manual') {
      throw Error('provenance_progress_restore_original_unbound');
    }
    if (!row) {
      if (await creation.findOne({_id: agentId})) throw Error('provenance_progress_restore_foreign_row');
      await creation.insertOne(original);
    } else if (canonical(row) !== scope.original) {
      const derived = {...original, record: {...original.record,
        creation_origin: {kind: 'legacy_unknown'}, created_by: null}};
      if (!recovered || canonical(row) !== canonical(derived)) throw Error('provenance_progress_derived_creation_changed');
      const restored = await creation.replaceOne({...creationKey, 'record.creation_origin.kind': 'legacy_unknown',
        'record.created_by': null}, original);
      if (restored.modifiedCount !== 1) throw Error('provenance_progress_restore_lost');
    }
    // Prevent an unrelated follow-up sweep from interfering with the other
    // serial fixture cases. The next normal use of this database is teardown.
    const state = await control.findOne({_id: controlId});
    if (state?.owner != null) {
      const deferred = await control.updateOne({_id: controlId, revision: state.revision, run: state.run},
        {$set: {next_sweep_at: new Date(now.getTime() + 2 * 60 * 60 * 1000)}});
      if (deferred.modifiedCount !== 1) throw Error('provenance_progress_restore_control_changed');
    }
    await session.commitTransaction();
    print(JSON.stringify({restored: true}));
  } finally {
    await session.endSession();
  }
})();
