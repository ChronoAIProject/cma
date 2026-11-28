(async () => {
  const {agentId, key} = attachmentScope;
  const owner = 'native-am-fixture-owner';
  const session = d.getMongo().startSession();
  try {
    session.startTransaction({readConcern: {level: 'snapshot'}});
    const database = session.getDatabase(d.getName());
    const agent = await database.agents.findOne({_id: agentId, owner, status: 'ready'});
    if (!agent) throw Error('attachment_fixture_agent_unbound');
    const sandbox = await database.sandboxes.findOne({_id: agent.sandbox_id, owner, status: 'active'});
    const workspace = await database.workspaces.findOne({_id: agent.workspace_id, sandbox_id: agent.sandbox_id,
      owner, status: 'ready'});
    const worktree = await database.worktrees.findOne({_id: agent.worktree_id, sandbox_id: agent.sandbox_id,
      workspace_id: agent.workspace_id, owner, status: 'ready', 'record.agent_id': agentId});
    const policy = await database.resource_policies.findOne({_id: agent.sandbox_id});
    const scopeOwner = policy ? policy.record?.owner : {kind: 'person', subject: owner};
    if (!sandbox || !workspace || !worktree || scopeOwner?.kind !== 'person' || scopeOwner.subject !== owner
        || (policy && (policy.record.sandbox_id !== sandbox._id
          || policy.record.sandbox_created_at !== sandbox.record.created_at))) {
      throw Error('attachment_fixture_lineage_unbound');
    }
    // AgentAttachmentService hashes the JSON tuple containing the UTF-8
    // identity bytes. Serde serializes the byte vector as a JSON array.
    const lineage = [sandbox._id, sandbox.record?.created_at, workspace._id, workspace.record?.created_at,
      worktree._id, worktree.record?.path, worktree.record?.branch, worktree.record?.base_commit,
      agentId, agent.record?.created_at, agent.record?.creation_fingerprint];
    if (lineage.some(value => typeof value !== 'string' || !value)) {
      throw Error('attachment_fixture_identity_unbound');
    }
    const identity = Buffer.from(JSON.stringify([{kind: 'person', subject: owner}, ...lineage]));
    const digest = require('crypto').createHash('sha256')
      .update(JSON.stringify(['agent-attachment-v1', Array.from(identity), key])).digest('hex');
    print(JSON.stringify({attachment_id: 'att_' + digest}));
  } finally {
    await session.endSession();
  }
})();
