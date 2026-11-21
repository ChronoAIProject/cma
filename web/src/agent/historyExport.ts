import type { AgentTranscript } from '../api/types';

/**
 * Serialize the public Agent transcript projection as one JSON object per line.
 * Keeping one record per message makes the file useful to tools that stream
 * JSONL, while the turn fields preserve the lifecycle and outcome metadata.
 */
export function agentHistoryJsonl(transcript: AgentTranscript): string {
  const lines: string[] = [
    JSON.stringify({
      format: 'cma.retained-conversation',
      version: 1,
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
        includes: ['recorded_user_messages', 'recorded_assistant_messages', 'turn_outcomes'],
        excludes: ['raw_runtime_records', 'tool_records', 'unrecorded_output'],
      },
    }),
  ];
  for (const turn of transcript.turns) {
    if (turn.messages.length === 0) {
      lines.push(
        JSON.stringify({
          agent_id: transcript.agentId,
          through_seq: transcript.throughSeq,
          turn_sequence: turn.turnSequence,
          started_at: turn.startedAt,
          outcome: turn.outcome,
          retained_output: turn.retainedOutput,
          owner_message: turn.ownerMessage,
          messages: [],
        }),
      );
      continue;
    }
    for (const message of turn.messages) {
      lines.push(
        JSON.stringify({
          agent_id: transcript.agentId,
          through_seq: transcript.throughSeq,
          turn_sequence: turn.turnSequence,
          started_at: turn.startedAt,
          outcome: turn.outcome,
          retained_output: turn.retainedOutput,
          sequence: message.sequence,
          role: message.role,
          content: message.content,
          appended_at: message.appendedAt,
        }),
      );
    }
  }
  return `${lines.join('\n')}${lines.length === 0 ? '' : '\n'}`;
}

export function downloadAgentHistory(agentId: string, source: AgentTranscript | Blob): void {
  const blob =
    source instanceof Blob
      ? source
      : new Blob([agentHistoryJsonl(source)], { type: 'application/x-ndjson' });
  if (!(source instanceof Blob) && blob.size > 16 * 1024 * 1024)
    throw new Error('Retained conversation exceeds the export size limit. No file was downloaded.');
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download =
    source instanceof Blob
      ? `cma-agent-${agentId}-history.jsonl`
      : `cma-agent-${agentId}-retained-conversation-v1.jsonl`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
