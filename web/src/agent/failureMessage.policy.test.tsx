import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { failureMessage } from './failureMessage';
import { MessageStream } from './MessageStream';

const at = '2026-01-01T00:00:00Z';
const categories = [
  ['agent_turn_timeout', 'This turn reached its time limit and has ended.'],
  ['approval_timeout', 'The approval wait timed out. This turn has ended.'],
  ['approval_denied', 'Approval was denied. This turn has ended.'],
  ['approval_expired', 'The approval request expired. This turn has ended.'],
  [
    'approval_wait_disconnected',
    'The approval request disconnected and could not continue. This turn has ended.',
  ],
] as const;

describe.each(['live', 'durable', 'transcript'] as const)('%s policy terminal', (path) => {
  it.each(categories)(
    'renders the authoritative %s outcome and retains partial output',
    (code, copy) => {
      const failure = { class: code, detail: 'UNTRUSTED_PROVIDER_DETAIL' };
      const initial = conversationFromTranscript({
        agentId: 'agent-fixture',
        throughSeq: path === 'transcript' ? 4 : 3,
        turns: [
          {
            turnSequence: 1,
            startedAt: at,
            ownerMessage: 'Check the draft',
            agentMessages: ['Partial answer'],
            messages: [
              { sequence: 2, role: 'user', content: 'Check the draft', appendedAt: at },
              { sequence: 3, role: 'assistant', content: 'Partial answer', appendedAt: at },
            ],
            outcome: path === 'transcript' ? { type: 'failed', failure, at } : { type: 'active' },
          },
        ],
      });
      const state =
        path === 'transcript'
          ? initial
          : path === 'durable'
            ? foldAgentEvent(initial, {
                seq: 4,
                appendedAt: at,
                kind: { type: 'turn_failed', turn_sequence: 1, failure },
              })
            : foldLiveEvent(
                initial,
                {
                  type: 'RUN_ERROR',
                  threadId: 'agent-fixture',
                  runId: '1',
                  timestamp: at,
                  code,
                  message: failure.detail,
                },
                'agent-fixture',
              );
      const approve = vi.fn();
      render(<MessageStream conversation={state} onApproval={approve} />);
      expect(screen.getByText(copy)).toBeVisible();
      expect(screen.getByText('Partial answer')).toBeVisible();
      expect(state.activeRunId).toBeUndefined();
      expect(state.turnOutcomes?.['1']).toBe('failed');
      expect(document.body).not.toHaveTextContent('UNTRUSTED_PROVIDER_DETAIL');
      expect(document.body).not.toHaveTextContent('not sent');
      expect(screen.queryByRole('button', { name: /^Thinking/ })).toBeNull();
      expect(approve).not.toHaveBeenCalled();
      expect(failureMessage(code).nextStep).toContain('does not automatically replay');
      expect(failureMessage(code).nextStep).toContain('may repeat effects');
    },
  );
});

it.each(['upstream_timeout', 'upstream_unavailable', '503', '524', 'waiting', undefined])(
  'does not infer approval from %s',
  (code) => {
    expect(failureMessage(code).detail).not.toMatch(/approval/i);
  },
);
