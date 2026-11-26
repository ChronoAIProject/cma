(async () => {
  const {sandboxId, agentIds} = lifecycleSample;
  const session = await d.getMongo().startSession();
  try {
    // Separate reads of the same rows can falsely report a half-published phase.
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const parent = await snapshot.sandboxes.findOne({_id: sandboxId}, {
      owner: 1, status: 1, revision: 1, purge_after: 1, 'record.runtime': 1,
    });
    const children = parent && await snapshot.agents.find({
      _id: {$in: agentIds}, sandbox_id: sandboxId, owner: parent.owner,
    }, {status: 1, revision: 1, last_seq: 1, 'record.conversation.conversation_id': 1,
      'record.conversation.runtime_generation': 1}).toArray();
    print(JSON.stringify(parent && {
      status: parent.status, revision: Number(parent.revision),
      purge_deadline_future: typeof parent.purge_after === 'string' && Date.parse(parent.purge_after) > Date.now(),
      runtime_detached: parent.record?.runtime == null,
      boot_id: parent.record?.runtime?.boot_id ?? null,
      children: agentIds.map(agentId => {
        const child = children.find(row => row._id === agentId);
        const conversation = child?.record?.conversation;
        return child && {status: child.status, revision: Number(child.revision), last_seq: Number(child.last_seq),
          conversation_id: conversation?.conversation_id ?? null,
          runtime_generation: Number(conversation?.runtime_generation ?? 0)};
      }),
    }));
  } finally {
    await session.endSession();
  }
})();
