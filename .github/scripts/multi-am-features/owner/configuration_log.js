(async () => {
  const session = await d.getMongo().startSession();
  try {
    await session.startTransaction({readConcern: {level: 'snapshot'}});
    const snapshot = session.getDatabase(d.getName());
    const agent = await snapshot.agents.findOne({_id: scope.agent}, {'record.conversation.last_seq': 1});
    const rawSequence = agent?.record?.conversation?.last_seq;
    const through = Number(rawSequence);
    if (rawSequence == null || !Number.isSafeInteger(through) || through < 0
        || (scope.after !== null && through < scope.after)) {
      throw new Error('configuration_log_cursor_missing');
    }
    const rows = scope.after === null ? [] : await snapshot.agent_log.find({
      agent_id: scope.agent, seq: {$gt: scope.after, $lte: through},
      'record.kind.type': 'operation_delivery_changed', 'record.kind.operation': 'update_settings',
    }, {_id: 0, seq: 1, 'record.kind': 1}).sort({seq: 1}).limit(3).toArray();
    print(JSON.stringify({through, rows: rows.map(({seq, record: {kind}}) => ({
      seq: Number(seq), sequence: Number(kind.operation_sequence), state: kind.state,
    }))}));
  } finally {
    await session.endSession();
  }
})();
