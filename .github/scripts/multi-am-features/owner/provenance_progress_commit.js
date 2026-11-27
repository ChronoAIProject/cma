(async () => {
  const hold = 'hangDuringBatchInsert';
  const namespace = d.getName() + '.agent_creation_provenance';
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
      finally { await profile(false); }
    }
    print(JSON.stringify({configured: false}));
    return;
  }
  if (action !== 'commit') throw Error('provenance_progress_commit_action_required');

  const transaction = (session, number) => require('crypto').createHash('sha256')
    .update(session.id.toString() + ':' + number.toString()).digest('hex');
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
    return op.opid;
  }

  const deadline = Date.now() + 6500;
  let opid;
  do {
    opid = await heldInsert();
    if (opid != null) break;
    await sleep(30);
  } while (Date.now() < deadline);
  if (opid == null) throw Error('provenance_progress_commit_insert_not_held');

  const pulse = 'fixture provenance progress acknowledgement loss ' + scope.transaction;
  await profile(true);
  const baseline = await configure('failCommand', {times: budget}, {appName: app,
    failCommands: ['commitTransaction'], writeConcernError: {
      code: 50, codeName: 'MaxTimeMSExpired', errmsg: pulse,
    }});
  if (!Number.isSafeInteger(baseline) || baseline < 0 || await heldInsert() !== opid) {
    throw Error('provenance_progress_commit_insert_lost');
  }
  await configure(hold, 'off');

  async function affectedCommits() {
    const result = await admin.runCommand({getLog: 'global'});
    if (result.ok !== 1) throw Error('provenance_progress_commit_log_unavailable');
    const activated = new Set();
    let target = 0, other = 0;
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
      if (msg !== 'Slow query' || command?.commitTransaction !== 1 || !activated.delete(ctx)) continue;
      if (command.lsid?.id && command.txnNumber != null
          && transaction(command.lsid, command.txnNumber) === scope.transaction) target++;
      else other++;
    }
    return {target, other};
  }

  const logDeadline = Date.now() + 1500;
  while (Date.now() < logDeadline && (await affectedCommits()).target < 5) await sleep(15);
  const affected = await configure('failCommand', 'off') - baseline;
  const {target, other} = await affectedCommits();
  await profile(false);
  if (!Number.isSafeInteger(affected) || affected < 5 || affected >= budget || target !== affected || other !== 0) {
    throw Error('provenance_progress_commit_attribution_failed');
  }
  print(JSON.stringify({commit_acknowledgements_lost: affected}));
})();
