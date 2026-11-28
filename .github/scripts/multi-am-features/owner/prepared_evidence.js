(async () => {
  const {agentId, sandboxId, operationId} = lineage;
  const session = await d.getMongo().startSession();
  try {
    // Use one committed snapshot while both Managers can still update the hierarchy.
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const agent = await snapshot.agents.findOne({
      _id: agentId, sandbox_id: sandboxId, 'record.origin.operation_id': operationId,
    }, {
      workspace_id: 1, worktree_id: 1, owner: 1, status: 1, 'record.work.receipts': 1,
      'record.conversation.conversation_id': 1,
    });
    const typedSchema = {$in: [{$type: '$document_schema_version'}, ['int', 'long']]};
    const fields = {document_schema_version: 1, agent_prepared_open_ready_fence: 1};
    const sandbox = agent && await snapshot.sandboxes.findOne({
      _id: sandboxId, owner: agent.owner, status: 'active', 'record.origin.operation_id': operationId,
      $expr: typedSchema,
    }, {...fields, 'record.runtime': 1});
    const reservation = sandbox && await snapshot.native_prepared_sandboxes.findOne({
      _id: sandboxId, kind: 'reservation', state: 'claimed',
      'record.ownership.owner': agent.owner, 'record.ownership.operation_id': operationId,
    }, {...fields, 'record.payload.installed_request_sha256': 1, 'record.payload.runtime': 1,
      'record.payload.workspaces': 1});
    const workspace = sandbox && await snapshot.workspaces.findOne({
      _id: agent.workspace_id, sandbox_id: sandboxId, owner: agent.owner, status: 'ready', $expr: typedSchema,
    }, fields);
    const worktree = workspace && await snapshot.worktrees.findOne({
      _id: agent.worktree_id, workspace_id: workspace._id, sandbox_id: sandboxId,
      owner: agent.owner, status: 'ready', $expr: typedSchema,
    }, fields);
    const parents = [sandbox, workspace, worktree];
    // The installation digest belongs to the reservation. Sandbox work
    // consumes its transient copy before the Agent opens.
    const installed = reservation?.record?.payload?.installed_request_sha256;
    const reserved = reservation?.record?.payload?.runtime;
    const active = sandbox?.record?.runtime;
    const preparedWorkspaces = reservation?.record?.payload?.workspaces;
    const [preparedWorkspace] = Array.isArray(preparedWorkspaces) ? preparedWorkspaces : [];
    const preparedAgents = preparedWorkspace?.agents;
    const [preparedAgent] = Array.isArray(preparedAgents) ? preparedAgents : [];
    const conversation = agent?.record?.conversation?.conversation_id;
    const reservationBound = Boolean(worktree) && preparedWorkspaces?.length === 1 && preparedAgents?.length === 1
      && preparedWorkspace?.id === workspace._id && preparedAgent?.id === agentId
      && preparedAgent?.worktree_id === worktree._id && typeof conversation === 'string'
      && conversation.length > 0 && preparedAgent?.conversation_id === conversation;
    const runtimeBound = ['runtime_id', 'runtime_version', 'boot_id'].every(key =>
      typeof reserved?.[key] === 'string' && reserved[key].length > 0 && reserved[key] === active?.[key])
      && Number.isSafeInteger(Number(reserved?.provision_generation))
      && Number(reserved.provision_generation) > 0
      && Number(reserved.provision_generation) === Number(active.provision_generation);
    const receipts = agent?.record?.work?.receipts;
    const operations = [
      ['add_worktree', 'worktree:workspace:wt-1:add:v1'],
      ['materialize_home', `agent:${agentId}:home:create:v1`],
      ['open_conversation', `agent:${agentId}:open:v1`],
    ];
    const startupReceipts = operations.map(([step, operationId]) => {
      const matches = Array.isArray(receipts) ? receipts.filter(receipt => receipt?.step === step) : [];
      return matches.every(receipt => receipt.operation_id === operationId
        && /^[a-f0-9]{64}$/.test(receipt.request_sha256 ?? '')) ? matches.length : 0;
    });
    print(JSON.stringify({
      consumed: Boolean(reservation),
      agent_ready: agent?.status === 'ready',
      reservation_bound: reservationBound,
      installation_bound: runtimeBound && typeof installed === 'string' && /^[a-f0-9]{64}$/.test(installed),
      schemas: parents.map(parent => Number(parent?.document_schema_version ?? 0)),
      ready_fences: [...parents, reservation].map(row => Number(row?.agent_prepared_open_ready_fence ?? 0)),
      startup_receipts: startupReceipts,
    }));
  } finally {
    await session.endSession();
  }
})();
