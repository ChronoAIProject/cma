import { HISTORY_TOO_LARGE, MAX_HISTORY_BYTES, type RetainedHistory } from './readRetainedHistory';

/**
 * Serialize the public transcript and tool events in durable sequence order.
 * Turn fields preserve the lifecycle and outcome metadata.
 */
export function agentHistoryJsonl({ transcript, toolEvents }: RetainedHistory): string {
  const manifest = {
    format: 'cma.retained-conversation',
    version: toolEvents ? 2 : 1,
    agent_id: transcript.agentId,
    through_seq: transcript.throughSeq,
    coverage: {
      raw_history: false,
      completeness: 'not_certified',
      retention_window: 'not_reported_by_source',
      gaps: 'see_turn_outcomes; absence_of_gaps_is_not_certified',
      truncation: 'none_by_exporter',
      first_turn_sequence: transcript.turns[0]?.turnSequence ?? null,
      last_turn_sequence: transcript.turns.at(-1)?.turnSequence ?? null,
      includes: [
        'recorded_user_messages',
        'recorded_assistant_messages',
        'turn_outcomes',
        ...(toolEvents ? ['recorded_tool_events'] : []),
      ],
      excludes: [
        'raw_runtime_records',
        ...(toolEvents ? [] : ['tool_records']),
        'unrecorded_output',
      ],
    },
  };
  const records = [];
  for (const turn of transcript.turns) {
    const fields = {
      agent_id: transcript.agentId,
      through_seq: transcript.throughSeq,
      turn_sequence: turn.turnSequence,
      started_at: turn.startedAt,
      outcome: turn.outcome,
      retained_output: turn.retainedOutput,
    };
    if (turn.messages.length === 0) {
      records.push({
        ...fields,
        sequence: turn.turnSequence,
        owner_message: turn.ownerMessage,
        messages: [],
      });
      continue;
    }
    for (const message of turn.messages) {
      records.push({
        ...fields,
        sequence: message.sequence,
        role: message.role,
        content: message.content,
        appended_at: message.appendedAt,
      });
    }
  }
  for (const event of toolEvents ?? []) {
    records.push({
      agent_id: transcript.agentId,
      through_seq: transcript.throughSeq,
      sequence: event.seq,
      appended_at: event.appendedAt,
      tool_event: event.kind,
    });
  }
  records.sort((left, right) => left.sequence - right.sequence);
  return `${[manifest, ...records].map((record) => JSON.stringify(record)).join('\n')}\n`;
}

export function downloadAgentHistory(agentId: string, source: RetainedHistory | Blob): void {
  const blob =
    source instanceof Blob
      ? source
      : new Blob([agentHistoryJsonl(source)], { type: 'application/x-ndjson' });
  if (!(source instanceof Blob) && blob.size > MAX_HISTORY_BYTES)
    throw new Error(HISTORY_TOO_LARGE);
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download =
    source instanceof Blob
      ? `cma-agent-${agentId}-history.jsonl`
      : `cma-agent-${agentId}-retained-conversation-v${source.toolEvents ? 2 : 1}.jsonl`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
