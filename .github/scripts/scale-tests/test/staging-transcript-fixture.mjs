// Public transcript pages contain one turn and at most `limit` source rows;
// the queued owner is repeated only in the first source window of that turn.
export function transcriptPage(path, turns, through) {
  const query = new URL(path, 'https://synthetic.invalid').searchParams;
  const limit = Number(query.get('limit'));
  if (query.has('through') && Number(query.get('through')) !== through) throw new Error('source changed');
  const index = query.has('turn') ? turns.findIndex((t) => t.turn_sequence === Number(query.get('turn'))) : 0;
  const turn = turns[index];
  const after = query.has('after') ? Number(query.get('after')) : turn.turn_sequence - 1;
  const end = turns[index + 1]?.turn_sequence - 1 || through;
  const pageEnd = Math.min(end, after + limit);
  const messages = turn.messages.filter((m) => m.sequence <= pageEnd
    && (m.sequence > after || (after === turn.turn_sequence - 1 && m.sequence < turn.turn_sequence)));
  const next = pageEnd < end ? { turn: turn.turn_sequence, after: pageEnd, through, limit }
    : turns[index + 1] ? { turn: turns[index + 1].turn_sequence, after: end, through, limit } : null;
  return { agent_id: 'agt_synthetic', through_seq: through,
    turns: [{ ...turn, messages, agent_messages: messages.filter((m) => m.role === 'assistant').map((m) => m.content) }], next };
}
