(async () => {
  const hold = 'hangDuringBatchInsert';
  const sweepHold = 'hangInsertBeforeWrite';
  const namespace = d.getName() + '.agent_creation_provenance';
  const sweepNamespace = d.getName() + '.agent_provenance_progress';
  const budget = 10;
  const {action, app} = scope;
  async function configure(failpoint, mode, data) {
    const result = await admin.runCommand({configureFailPoint: failpoint, mode, ...(data ? {data} : {})});
    if (result.ok !== 1) throw Error('provenance_progress_commit_failpoint_failed');
    return Number(result.count);
  }
  async function profile(enabled) {
    const result = await admin.runCommand(enabled
      ? {profile: 2, filter: {appName: app, 'command.commitTransaction': 1}}
      : {profile: 0, filter: 'unset'});
    if (result.ok !== 1) throw Error('provenance_progress_commit_profile_failed');
  }
  if (action === 'arm') {
    await configure(hold, 'alwaysOn', {nss: namespace, shouldCheckForInterrupt: true});
    await configure('failCommand', {times: 1}, {appName: app, namespace: d.getName() + '.agents',
      failCommands: ['aggregate'], blockConnection: true, blockTimeMS: 4500});
    print(JSON.stringify({configured: true}));
    return;
  }
  if (action === 'restore') {
    try { await configure('failCommand', 'off'); }
    finally {
      try { await configure(hold, 'off'); }
      finally {
        try { await configure(sweepHold, 'off'); }
        finally { await profile(false); }
      }
    }
    print(JSON.stringify({configured: false}));
    return;
  }
  const transaction = (session, number) => require('crypto').createHash('sha256')
    .update(session.id.toString() + ':' + number.toString()).digest('hex');
  const pulse = 'fixture provenance progress acknowledgement loss ' + scope.transaction;
  if (action === 'commit') {
    async function heldInsert() {
      const ops = await admin.aggregate([
        {$currentOp: {allUsers: true}},
        {$match: {active: true, appName: app, ns: namespace, failpointMsg: hold,
          'command.insert': 'agent_creation_provenance', 'transaction.parameters.autocommit': false}},
        {$limit: 2},
      ]).toArray();
      if (ops.length > 1) throw Error('provenance_progress_commit_multiple_held_inserts');
      const [op] = ops;
      const number = op?.transaction?.parameters?.txnNumber;
      if (!op?.lsid?.id || number == null) return null;
      if (transaction(op.lsid, number) !== scope.transaction) {
        throw Error('provenance_progress_commit_insert_transaction_changed');
      }
      return {opid: op.opid, session_sha256: require('crypto').createHash('sha256')
        .update(op.lsid.id.buffer).digest('hex')};
    }

    const deadline = Date.now() + 6500;
    let held;
    do {
      held = await heldInsert();
      if (held) break;
      await sleep(30);
    } while (Date.now() < deadline);
    if (!held) throw Error('provenance_progress_commit_insert_not_held');

    // The worker can retry before its log reaches the fixture. Keep the
    // following sweep receipt transaction from consuming the commit pulse.
    await configure(sweepHold, 'alwaysOn', {ns: sweepNamespace});
    await profile(true);
    const baseline = await configure('failCommand', {times: budget}, {appName: app,
      failCommands: ['commitTransaction'], writeConcernError: {
        code: 50, codeName: 'MaxTimeMSExpired', errmsg: pulse,
      }});
    if (!Number.isSafeInteger(baseline) || baseline < 0 || (await heldInsert())?.opid !== held.opid) {
      throw Error('provenance_progress_commit_insert_lost');
    }
    await configure(hold, 'off');
    print(JSON.stringify({baseline, session_sha256: held.session_sha256}));
    return;
  }

  async function affectedCommits() {
    const result = await admin.runCommand({getLog: 'global'});
    if (result.ok !== 1) throw Error('provenance_progress_commit_log_unavailable');
    const activated = new Set();
    let target = 0, other = 0, confirmed = 0;
    // A connection processes its commands serially. Mongo may log more
    // than one failpoint check for the same command, so consume the
    // activation at that connection's next completed commit command.
    for (const line of result.log) {
      const {id, ctx, msg, attr} = EJSON.parse(line);
      if (!ctx || attr?.appName !== app) continue;
      if (id === 4898500 && attr.command === 'commitTransaction'
          && attr.data?.writeConcernError?.errmsg === pulse) {
        activated.add(ctx);
        continue;
      }
      const command = attr.command;
      if (msg !== 'Slow query' || command?.commitTransaction !== 1) continue;
      const affected = activated.delete(ctx);
      const matching = command.lsid?.id && command.txnNumber != null
        && transaction(command.lsid, command.txnNumber) === scope.transaction;
      if (matching && affected) target++;
      if (matching && !affected) confirmed++;
      if (!matching && affected) other++;
    }
    return {target, other, confirmed};
  }

  if (action !== 'settle' && action !== 'observe_commit') throw Error('provenance_progress_commit_action_required');
  const affected = action === 'settle'
    ? await configure('failCommand', 'off') - scope.baseline : scope.affected;
  const {target, other, confirmed} = await affectedCommits();
  if (!Number.isSafeInteger(affected) || affected !== 5 || target !== affected || other !== 0 || confirmed !== 0) {
    throw Error('provenance_progress_commit_attribution_failed');
  }
  if (action === 'settle') await configure(sweepHold, 'off');
  print(JSON.stringify({commit_acknowledgements_lost: affected}));
})();
