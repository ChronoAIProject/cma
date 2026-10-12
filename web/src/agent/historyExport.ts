import type { AgentTranscript } from '../api/types';

/**
 * Serialize the public Agent transcript projection as one JSON object per line.
 * Keeping one record per message makes the file useful to tools that stream
 * JSONL, while the turn fields preserve the lifecycle and outcome metadata.
 */
export function agentHistoryJsonl(transcript: AgentTranscript): string {
  const lines: string[] = [];
  for (const turn of transcript.turns) {
    if (turn.messages.length === 0) {
      lines.push(
        JSON.stringify({
          agent_id: transcript.agentId,
          through_seq: transcript.throughSeq,
          turn_sequence: turn.turnSequence,
          started_at: turn.startedAt,
          outcome: turn.outcome,
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

export function downloadAgentHistory(agentId: string, transcript: AgentTranscript): void {
  const url = URL.createObjectURL(
    new Blob([agentHistoryJsonl(transcript)], { type: 'application/x-ndjson' }),
  );
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = `cma-agent-${agentId}-history.jsonl`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
