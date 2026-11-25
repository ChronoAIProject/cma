import type { CmaWebClient } from '../api/client';
import type { AgentEvent, AgentTranscript } from '../api/types';

const MAX_PAGES = 256;
export const MAX_HISTORY_BYTES = 16 * 1024 * 1024;
export const HISTORY_TOO_LARGE =
  'Retained conversation exceeds the export size limit. No file was downloaded.';
const INCOMPLETE_HISTORY =
  'Retained history could not be read through its saved sequence. Please retry.';
const TOOL_EVENTS = new Set(['tool_call_started', 'tool_call_updated', 'tool_output_reported']);

export interface RetainedHistory {
  readonly transcript: AgentTranscript;
  readonly toolEvents?: readonly AgentEvent[];
}

/** Read the same durable tool records shown in the conversation through the transcript watermark. */
export async function readRetainedHistory(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  format: 'history' | 'transcript',
): Promise<RetainedHistory> {
  const transcript = await client.readAgentTranscript(agentId, {
    signal,
    cache: 'no-store',
    maxPages: MAX_PAGES,
    maxBytes: MAX_HISTORY_BYTES,
  });
  signal.throwIfAborted();
  if (transcript.agentId !== agentId)
    throw new Error('The retained conversation did not match this Agent. Please retry.');
  if (!Number.isSafeInteger(transcript.throughSeq) || transcript.throughSeq < 0)
    throw new Error(INCOMPLETE_HISTORY);
  if (format === 'transcript') return { transcript };

  const toolEvents: AgentEvent[] = [];
  const encoder = new TextEncoder();
  let bytes = 0;
  let cursor = 0;
  for (let pageNumber = 0; cursor < transcript.throughSeq && pageNumber < MAX_PAGES; pageNumber++) {
    const page = await client.readAgentEvents(agentId, { cursor, signal, cache: 'no-store' });
    signal.throwIfAborted();
    for (const event of page.items) {
      if (event.seq !== cursor + 1) throw new Error(INCOMPLETE_HISTORY);
      cursor = event.seq;
      if (TOOL_EVENTS.has(event.kind.type)) {
        bytes += encoder.encode(JSON.stringify(event)).byteLength;
        if (bytes > MAX_HISTORY_BYTES) throw new Error(HISTORY_TOO_LARGE);
        toolEvents.push(event);
      }
      if (cursor === transcript.throughSeq) break;
    }
    if (cursor < transcript.throughSeq && (page.items.length === 0 || page.next !== cursor))
      throw new Error(INCOMPLETE_HISTORY);
  }
  if (cursor !== transcript.throughSeq) throw new Error(INCOMPLETE_HISTORY);
  return { transcript, toolEvents };
}
