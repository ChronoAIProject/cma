(async () => {
  const {sandboxId, agentId, operationId, admissions, arm, titleDigest, advanceTitle, refusedLeaseSha256} = scope;
  const owner = 'native-am-fixture-owner';
  const session = await d.getMongo().startSession();
  try {
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const target = {_id: sandboxId, owner, origin_operation_id: operationId};
    const sandbox = await snapshot.sandboxes.findOne(target, {
      status: 1, revision: 1, purge_after: 1, work_ready_at: 1, work_lease_expires_at: 1,
      last_seq: 1, 'record.runtime': 1, 'record.work': 1,
    });
    const children = await snapshot.agents.find({sandbox_id: sandboxId}, {
      status: 1, owner: 1, 'record.origin.operation_id': 1,
    }).limit(admissions.length + 1).toArray();
    const expected = new Map(admissions.map(row => [row.agentId, row.operationId]));
    const exactChildren = children.length === expected.size && children.every(row => expected.has(row._id)
      && row.owner === owner && row.status === 'deleted'
      && expected.get(row._id) === row.record?.origin?.operation_id);
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
    const title = titleDigest && sandbox && await snapshot.agents.findOne({_id: agentId, sandbox_id: sandboxId,
      owner, status: 'deleted', 'record.origin.operation_id': operationId, title_effect_floor: 2,
      'title_effect_custody.version': 1, 'title_effect_custody.intent.agent_id': agentId,
      'title_effect_custody.intent.request_digest': titleDigest, 'title_effect_custody.result': null},
      {title_effect_custody: 1});
    const intent = title?.title_effect_custody?.intent;
    const acquired = Number(intent?.acquired_ms), expires = Number(intent?.expires_ms);
    const duration = expires - acquired;
    const now = Date.now();
    if (titleDigest && sandbox && (!Number.isSafeInteger(acquired) || !Number.isSafeInteger(expires)
        || duration <= 0 || duration > 60000)) {
      throw new Error('retirement_purge_unresolved_title_unbound');
    }
    async function moveTitleWindow(newStart) {
      const filter = {_id: agentId, sandbox_id: sandboxId, owner, status: 'deleted',
        'record.origin.operation_id': operationId,
        'title_effect_custody.intent.request_digest': titleDigest, 'title_effect_custody.result': null,
        'title_effect_custody.intent.acquired_ms': intent.acquired_ms,
        'title_effect_custody.intent.expires_ms': intent.expires_ms};
      const update = await snapshot.agents.updateOne(filter, {$set: {
        'title_effect_custody.intent.acquired_ms': NumberLong(String(newStart)),
        'title_effect_custody.intent.expires_ms': NumberLong(String(newStart + duration)),
      }});
      if (update.modifiedCount !== 1) throw new Error('retirement_purge_title_window_changed');
    }
    if (advanceTitle) {
      const lease = sandbox?.record.work?.lease;
      if (!sandbox || !exactChildren || !event || !agentEvent || !workspace || !worktree
          || sandbox.status !== 'retired' || sandbox.record.runtime != null
          || sandbox.record.work?.step?.kind !== 'purge' || Date.parse(sandbox.purge_after) > now
          || !Number.isFinite(Date.parse(sandbox.purge_after))
          || typeof lease?.holder !== 'string' || !lease.holder || !Number.isSafeInteger(Number(lease.fence))
          || Number(lease.fence) < 1 || lease.expires_at !== sandbox.work_lease_expires_at
          || !Number.isFinite(Date.parse(lease.expires_at))) {
        throw new Error('retirement_purge_title_deferral_unbound');
      }
      const leaseSha256 = require('crypto').createHash('sha256')
        .update(lease.fence.toString() + ':' + lease.holder).digest('hex');
      if (leaseSha256 !== refusedLeaseSha256) throw new Error('retirement_purge_refused_lease_replaced');
      await moveTitleWindow(now - duration - 1000);
      // The refused attempt can retain its work lease. Advance only its
      // exact expiry so the regular scheduler can claim the next attempt.
      const elapsedLease = new Date(now - 1000).toISOString().replace(/\.\d{3}Z$/, 'Z');
      const update = await snapshot.sandboxes.updateOne({...target, status: 'retired',
        revision: sandbox.revision, 'record.work.step.kind': 'purge', work_lease_expires_at: lease.expires_at,
        'record.work.lease.expires_at': lease.expires_at, 'record.work.lease.holder': lease.holder,
        'record.work.lease.fence': lease.fence}, {$set: {work_lease_expires_at: elapsedLease,
        'record.work.lease.expires_at': elapsedLease}});
      if (update.modifiedCount !== 1) throw new Error('retirement_purge_deferred_lease_changed');
      await session.commitTransaction();
      print(JSON.stringify({title_advanced: true, creation_sha256: creationDigest}));
      return;
    }
    if (!arm) {
      if ((!sandbox && children.length !== 0) || (sandbox && !exactChildren)) {
        throw new Error('retirement_purge_changed_admission_set');
      }
      const foreign = !sandbox && await snapshot.sandboxes.findOne({_id: sandboxId}, {_id: 1});
      if (foreign) throw new Error('retirement_purge_changed_sandbox');
      print(JSON.stringify({present: Boolean(sandbox), agents: children.length,
        sandbox_events: Boolean(event), agent_events: Boolean(agentEvent),
        workspaces: Boolean(workspace), worktrees: Boolean(worktree), creation_sha256: creationDigest,
        ...(titleDigest && sandbox ? {unresolved_title_active: now < expires} : {})}));
      return;
    }

    const initial = sandbox?.purge_after;
    if (sandbox?.status !== 'retired' || sandbox.record.runtime != null
        || sandbox.record.work?.step?.kind !== 'wait_for_purge' || sandbox.work_lease_expires_at != null
        || typeof initial !== 'string' || Date.parse(initial) <= now || !Number.isFinite(Date.parse(initial))
        || sandbox.work_ready_at !== initial || !event || !agentEvent || !workspace || !worktree
        || !exactChildren) {
      throw new Error('retirement_purge_terminal_lineage_required');
    }
    // Move only the elapsed-time inputs. The actual production worker still
    // proves custody, retention, title finality and the exact revision.
    if (titleDigest) await moveTitleWindow(now);
    const elapsed = new Date(now + 1000).toISOString().replace(/\.\d{3}Z$/, 'Z');
    const update = await snapshot.sandboxes.updateOne({...target, status: 'retired', revision: sandbox.revision,
      last_seq: sandbox.last_seq, 'record.work.step.kind': 'wait_for_purge',
      purge_after: initial, work_ready_at: initial, work_lease_expires_at: null},
    {$set: {purge_after: elapsed, work_ready_at: elapsed, 'record.purge_after': elapsed, 'record.work.ready_at': elapsed}});
    if (update.modifiedCount !== 1) throw new Error('retirement_purge_schedule_not_advanced');
    await session.commitTransaction();
    print(JSON.stringify({armed: true, agents: children.length, creation_sha256: creationDigest}));
  } finally {
    await session.endSession();
  }
})();
