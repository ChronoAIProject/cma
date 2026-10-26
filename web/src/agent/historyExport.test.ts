import { afterEach, expect, it, vi } from 'vitest';
import type { AgentTranscript } from '../api/types';
import { agentHistoryJsonl, downloadAgentHistory } from './historyExport';

const transcript: AgentTranscript = {
  agentId: 'agt_123',
  throughSeq: 8,
  turns: [
    {
      turnSequence: 1,
      startedAt: '2026-10-12T00:00:00Z',
      ownerMessage: 'Hello',
      agentMessages: ['Hi'],
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
          content: 'Hi',
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
  expect(lines).toHaveLength(2);
  expect(lines.map((line) => JSON.parse(line))).toEqual([
    expect.objectContaining({ agent_id: 'agt_123', sequence: 7, role: 'user' }),
    expect.objectContaining({ agent_id: 'agt_123', sequence: 8, role: 'assistant' }),
  ]);
});

it('keeps an empty transcript empty', () => {
  expect(agentHistoryJsonl({ ...transcript, turns: [] })).toBe('');
});

it('downloads a named JSONL file and releases its object URL', () => {
  const create = vi.fn(() => 'blob:history');
  const revoke = vi.fn();
  Object.defineProperty(URL, 'createObjectURL', {
    configurable: true,
    value: create,
  });
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true,
    value: revoke,
  });
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
  vi.useFakeTimers();

  downloadAgentHistory('agt_123', transcript);
  vi.runAllTimers();

  expect(create).toHaveBeenCalledOnce();
  expect(click).toHaveBeenCalledOnce();
  expect(revoke).toHaveBeenCalledWith('blob:history');
  expect(document.querySelector('a')).not.toBeInTheDocument();
});
