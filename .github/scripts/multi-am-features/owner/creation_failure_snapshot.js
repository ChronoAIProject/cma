// Retain a coherent startup snapshot before public retirement, without caller
// data or resource identities. Aggregate work can advance independently.
(async () => {
  const session = await d.getMongo().startSession();
  try {
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const database = session.getDatabase(d.getName());
    const operation = await database.agent_lifecycle_operations.findOne({_id: operationId},
      {owner: 1, sandbox_id: 1, agent_id: 1});
    if (!operation) return print('null');
    const owner = operation.owner;
    const work = {status: 1, 'record.work.step': 1, 'record.work.attempts': 1};
    const agent = operation.agent_id && await database.agents.findOne({_id: operation.agent_id, owner,
      'record.origin.operation_id': operationId}, {...work, sandbox_id: 1, workspace_id: 1, worktree_id: 1});
    const sandboxId = operation.sandbox_id ?? agent?.sandbox_id;
    const sandbox = sandboxId && await database.sandboxes.findOne({_id: sandboxId, owner}, work);
    const workspace = sandbox && agent && await database.workspaces.findOne({
      _id: agent.workspace_id, sandbox_id: sandboxId, owner,
    }, {...work, 'record.resolved_commit': 1});
    const tree = workspace && await database.worktrees.findOne({
      _id: agent.worktree_id, workspace_id: workspace._id, sandbox_id: sandboxId, owner,
    }, {status: 1, 'record.base_commit': 1});
    function progress(row) {
      if (!row) return null;
      const work = row.record?.work;
      return {status: row.status, step: work?.step?.kind ?? work?.step, attempts: Number(work?.attempts)};
    }
    const commitSet = commit => typeof commit === 'string' && /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i.test(commit);
    print(JSON.stringify({
      agent: progress(agent),
      sandbox: progress(sandbox),
      workspace: workspace ? {...progress(workspace), commit_resolved: commitSet(workspace.record?.resolved_commit)} : null,
      worktree: tree ? {status: tree.status, base_bound: commitSet(tree.record?.base_commit)} : null,
    }));
  } finally {
    await session.endSession();
  }
})();
