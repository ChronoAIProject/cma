(async () => {
  const session = await d.getMongo().startSession();
  try {
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const {agentId, skillId} = scope;
    const agent = await snapshot.agents.findOne({_id: agentId}, {'record.work.live_skills': 1});
    const skills = agent?.record?.work?.live_skills;
    const entries = Array.isArray(skills) ? skills.filter(row => row.reference?.id === skillId)
      .map(row => ({status: row.status, version: row.material?.version})) : [];
    const log = await snapshot.agent_log.find({agent_id: agentId,
      'record.kind.type': 'skill_installation_changed', 'record.kind.skill_id': skillId,
    }, {_id: 0, seq: 1, 'record.kind.status': 1}).sort({seq: 1}).limit(3).toArray();
    print(JSON.stringify({entries, changes: log.map(entry => ({seq: Number(entry.seq),
      status: entry.record.kind.status})), agentFound: agent != null}));
  } finally {
    await session.endSession();
  }
})();
