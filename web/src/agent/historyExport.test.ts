import { afterEach, expect, it, vi } from 'vitest';
import type { AgentTranscript } from '../api/types';
import { agentHistoryJsonl, downloadAgentHistory } from './historyExport';

const answer = '**检查日期：**2026 年\n\n**检查范围：**仅对当前本地工作区';
const transcript: AgentTranscript = {
  agentId: 'agt_123',
  throughSeq: 8,
  turns: [
    {
      turnSequence: 1,
      startedAt: '2026-10-12T00:00:00Z',
      ownerMessage: 'Hello',
      agentMessages: [answer],
      messages: [
        {
          sequence: 7,
          role: 'user',
          content: 'Hello',
          appendedAt: '2026-10-12T00:00:01Z',
        },
        {
          sequence: 8,
          role: 'assistant',
          content: answer,
          appendedAt: '2026-10-12T00:00:02Z',
        },
      ],
      outcome: { status: 'completed' },
    },
  ],
};

afterEach(() => vi.restoreAllMocks());

it('serializes one valid JSON object per transcript message', () => {
  const lines = agentHistoryJsonl(transcript).trimEnd().split('\n');
  expect(lines).toHaveLength(3);
  expect(lines.map((line) => JSON.parse(line))).toEqual([
    expect.objectContaining({
      format: 'cma.retained-conversation',
      version: 1,
      through_seq: 8,
      coverage: expect.objectContaining({ raw_history: false, completeness: 'not_certified' }),
    }),
    expect.objectContaining({ agent_id: 'agt_123', sequence: 7, role: 'user' }),
    expect.objectContaining({
      agent_id: 'agt_123',
      sequence: 8,
      role: 'assistant',
      content: answer,
    }),
  ]);
});

it('exports coverage even when the retained transcript is empty', () => {
  expect(JSON.parse(agentHistoryJsonl({ ...transcript, turns: [] }))).toMatchObject({
    format: 'cma.retained-conversation',
    through_seq: 8,
    coverage: {
      first_turn_sequence: null,
      last_turn_sequence: null,
      completeness: 'not_certified',
    },
  });
});

it('downloads a named JSONL file and releases its object URL', () => {
  const create = vi.fn<(source: Blob) => string>(() => 'blob:history');
  const revoke = vi.fn();
  Object.defineProperty(URL, 'createObjectURL', {
    configurable: true,
    value: create,
  });
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true,
    value: revoke,
  });
  let filename: string | undefined;
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
    this: HTMLAnchorElement,
  ) {
    filename = this.download;
  });
  vi.useFakeTimers();

  downloadAgentHistory('agt_123', transcript);
  vi.runAllTimers();

  expect(create).toHaveBeenCalledOnce();
  expect(create.mock.calls[0]?.[0]).toMatchObject({ type: 'application/x-ndjson' });
  expect(filename).toBe('cma-agent-agt_123-retained-conversation-v1.jsonl');
  expect(click).toHaveBeenCalledOnce();
  expect(revoke).toHaveBeenCalledWith('blob:history');
  expect(document.querySelector('a')).not.toBeInTheDocument();
});

it('preserves incomplete turn outcomes and retained-output notices without certifying completeness', () => {
  const outcome = {
    type: 'failed',
    failure: { class: 'conversation_output_gap', detail: 'Output is incomplete.' },
  };
  const lines = agentHistoryJsonl({
    ...transcript,
    turns: [{ ...transcript.turns[0]!, outcome, retainedOutput: { available: true } }],
  })
    .trimEnd()
    .split('\n')
    .map((line) => JSON.parse(line));
  expect(lines[0].coverage).toMatchObject({
    completeness: 'not_certified',
    retention_window: 'not_reported_by_source',
  });
  expect(lines[1]).toMatchObject({ outcome, retained_output: { available: true } });
});
