(async () => {
  const {agentId, sandboxId, operationId} = lineage;
  const session = await d.getMongo().startSession();
  try {
    // Use one committed snapshot while both Managers can still update the hierarchy.
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const agent = await snapshot.agents.findOne({_id: agentId, sandbox_id: sandboxId}, {
      workspace_id: 1, worktree_id: 1, owner: 1, status: 1, 'record.work.prepared_installation_digest': 1,
    });
    const typedSchema = {$in: [{$type: '$document_schema_version'}, ['int', 'long']]};
    const fields = {document_schema_version: 1, agent_prepared_open_ready_fence: 1};
    const sandbox = agent && await snapshot.sandboxes.findOne({
      _id: sandboxId, owner: agent.owner, status: 'active', $expr: typedSchema,
    }, fields);
    const reservation = sandbox && await snapshot.native_prepared_sandboxes.findOne({
      _id: sandboxId, kind: 'reservation', state: 'claimed',
      'record.ownership.owner': agent.owner, 'record.ownership.operation_id': operationId,
    }, fields);
    const workspace = sandbox && await snapshot.workspaces.findOne({
      _id: agent.workspace_id, sandbox_id: sandboxId, owner: agent.owner, status: 'ready', $expr: typedSchema,
    }, fields);
    const worktree = workspace && await snapshot.worktrees.findOne({
      _id: agent.worktree_id, workspace_id: workspace._id, sandbox_id: sandboxId,
      owner: agent.owner, $expr: typedSchema,
    }, fields);
    const parents = [sandbox, workspace, worktree];
    print(JSON.stringify({
      consumed: Boolean(reservation),
      agent_ready: agent?.status === 'ready',
      installation_bound: /^[a-f0-9]{64}$/.test(agent?.record?.work?.prepared_installation_digest ?? ''),
      schemas: parents.map(parent => Number(parent?.document_schema_version ?? 0)),
      ready_fences: [...parents, reservation].map(row => Number(row?.agent_prepared_open_ready_fence ?? 0)),
    }));
  } finally {
    await session.endSession();
  }
})();
