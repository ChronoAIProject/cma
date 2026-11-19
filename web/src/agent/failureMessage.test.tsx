import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentTranscript } from '../api/types';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { failureMessage } from './failureMessage';
import { MessageStream } from './MessageStream';
import { StreamEntry } from './StreamEntry';
import { mergeTranscriptSnapshot } from './chronology';

const fallback =
  'The Agent run failed. Its delivery or task effects may be incomplete. Review the conversation before sending another message.';
const sentinel = 'PRIVATE_UPSTREAM_SENTINEL';
const diagnostics = `${sentinel} http://127.0.0.1:9876/private Ray ID: synthetic-ray X-Proxy: synthetic-header`;
const html = (status: number) =>
  `unexpected status ${status}: <!doctype html><html><style>.proxy{color:red}</style><body>${diagnostics}</body></html>`;
const bodies: readonly [string, unknown][] = [
  ['503 HTML', html(503)],
  ['524 HTML', html(524)],
  ['JSON', JSON.stringify({ error: diagnostics, status: 503 })],
  ['plain text', diagnostics],
  ['empty', ''],
  ['missing', undefined],
  ['malformed text', `{ "error": "${diagnostics}`],
  ['malformed value', { message: diagnostics, status: 524 }],
  ['overlong', diagnostics.repeat(4096)],
];
const categories = [
  'provider_failure',
  `unknown-${sentinel}`,
  undefined,
  { class: 'upstream_unavailable', status: 503 },
  'upstream_unavailable',
  'upstream_timeout',
  'insufficient_credits',
  'permission_denied',
  'agent_log_capacity_exceeded',
];
const at = '2026-11-18T00:00:00Z';
const transcript: AgentTranscript = {
  agentId: 'local-agent',
  throughSeq: 4,
  turns: [
    {
      turnSequence: 1,
      startedAt: at,
      ownerMessage: 'Earlier question',
      agentMessages: ['Earlier answer'],
      messages: [
        { sequence: 2, role: 'user', content: 'Earlier question', appendedAt: at },
        { sequence: 3, role: 'assistant', content: 'Earlier answer', appendedAt: at },
      ],
      outcome: { type: 'completed', at },
    },
  ],
};
const currentTurn = {
  turnSequence: 5,
  startedAt: at,
  ownerMessage: 'Current question',
  agentMessages: ['Partial answer'],
  messages: [
    { sequence: 6, role: 'user' as const, content: 'Current question', appendedAt: at },
    { sequence: 7, role: 'assistant' as const, content: 'Partial answer', appendedAt: at },
  ],
  retainedOutput: { available: true },
};

function failed(path: 'live' | 'durable' | 'transcript', category: unknown, detail: unknown) {
  const failure = { class: category, detail };
  if (path === 'transcript')
    return conversationFromTranscript({
      ...transcript,
      throughSeq: 8,
      turns: [...transcript.turns, { ...currentTurn, outcome: { type: 'failed', failure, at } }],
    });
  const active = conversationFromTranscript({
    ...transcript,
    throughSeq: 7,
    turns: [...transcript.turns, { ...currentTurn, outcome: { type: 'active' } }],
  });
  return path === 'live'
    ? foldLiveEvent(
        active,
        {
          type: 'RUN_ERROR',
          threadId: 'local-agent',
          runId: '5',
          timestamp: at,
          code: category,
          message: detail,
          'cma:retainedOutput': { available: true },
        },
        'local-agent',
      )
    : foldAgentEvent(active, {
        seq: 8,
        appendedAt: at,
        retainedOutput: { available: true },
        kind: { type: 'turn_failed', turn_sequence: 5, failure },
      });
}

afterEach(() => vi.unstubAllGlobals());

describe.each(['live', 'durable', 'transcript'] as const)('%s failed turns', (path) => {
  it.each(bodies)('omits the entire %s body for every failure category', (_name, detail) => {
    const fetcher = vi.fn();
    const onApproval = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    for (const category of categories) {
      const state = failed(path, category, detail);
      const notice = state.recovery[0]!;
      const view = render(
        <StreamEntry
          entry={{ kind: 'recovery', item: notice }}
          approvalBusy={false}
          onApproval={onApproval}
        />,
      );
      expect(screen.getByText('Agent run failed')).toBeVisible();
      expect(screen.getByText(failureMessage(category).detail)).toBeVisible();
      expect(screen.getByRole('status')).toHaveTextContent('does not automatically replay');
      expect(screen.getByRole('status')).toHaveTextContent('may repeat effects');
      expect(notice.code).toBeUndefined();
      expect(JSON.stringify(notice)).not.toContain(sentinel);
      const details = view.container.querySelector('details');
      if (details) {
        expect(details).not.toHaveAttribute('open');
        fireEvent.click(screen.getByText('Technical details'));
        expect(details).toHaveAttribute('open');
        expect(details.textContent).toContain('Category:');
      } else {
        expect(screen.getByText(fallback)).toBeVisible();
      }
      for (const privateText of [
        sentinel,
        '127.0.0.1',
        'synthetic-ray',
        'X-Proxy',
        '.proxy',
        'doctype',
        'provider_failure',
      ])
        expect(document.body.innerHTML).not.toContain(privateText);
      expect(screen.queryByRole('link')).not.toBeInTheDocument();
      expect(screen.queryByRole('button')).not.toBeInTheDocument();
      expect(fetcher).not.toHaveBeenCalled();
      expect(onApproval).not.toHaveBeenCalled();
      view.unmount();
    }
  });

  it('retains terminal state, identity, output and chronology through replay', () => {
    const state = failed(path, 'provider_failure', html(503));
    expect(state.activeRunId).toBeUndefined();
    expect(state.turnOutcomes).toEqual({ '1': 'completed', '5': 'failed' });
    expect(state.cursor).toBe(path === 'live' ? 7 : 8);
    expect(state.turnTiming?.['5']).toEqual({ startedAt: at, finishedAt: at });
    expect(state.recovery[0]).toMatchObject({ runId: '5', retainedOutput: { available: true } });
    expect(state.messages.map(({ id, runId, content }) => ({ id, runId, content }))).toEqual([
      { id: 'event:2', runId: '1', content: 'Earlier question' },
      { id: 'event:3', runId: '1', content: 'Earlier answer' },
      { id: 'event:6', runId: '5', content: 'Current question' },
      { id: 'event:7', runId: '5', content: 'Partial answer' },
    ]);
    const ordered = (snapshot: typeof state) =>
      mergeTranscriptSnapshot(
        conversationFromTranscript({ agentId: transcript.agentId, throughSeq: 0, turns: [] }),
        snapshot,
      );
    const view = render(<MessageStream conversation={ordered(state)} onApproval={vi.fn()} />);
    const text = document.body.textContent!;
    expect(text.indexOf('Earlier answer')).toBeLessThan(text.indexOf('Current question'));
    expect(text.indexOf('Partial answer')).toBeLessThan(text.indexOf('Agent run failed'));
    view.rerender(
      <MessageStream
        conversation={ordered(failed('transcript', 'provider_failure', html(503)))}
        onApproval={vi.fn()}
      />,
    );
    expect(screen.getByText(fallback)).toBeVisible();
    expect(document.body.innerHTML).not.toContain(sentinel);
  });
});

it.each([
  ['upstream_unavailable', 'temporarily unavailable (503)', 503],
  ['upstream_timeout', 'timed out (524)', 524],
  ['insufficient_credits', 'insufficient credits (402)', 402],
  ['permission_denied', 'permission was denied', undefined],
  ['agent_log_capacity_exceeded', 'retained partial output', undefined],
] as const)(
  'maps only normalized %s to safe copy and structured fields',
  (category, summary, status) => {
    expect(failureMessage(category).detail).toContain(summary);
    expect(failureMessage(category).technicalDetails).toEqual({
      category,
      ...(status === undefined ? {} : { httpStatus: status }),
    });
  },
);

it.each([
  null,
  undefined,
  '',
  503,
  524,
  [],
  {},
  '503',
  '524',
  'toString',
  '__proto__',
  'UPSTREAM_TIMEOUT',
  'upstream_timeout ',
  sentinel.repeat(10000),
])('uses a bounded generic message for unrecognized class %#', (category) => {
  const message = failureMessage(category);
  expect(message.detail).toBe(fallback);
  expect(message.technicalDetails).toBeUndefined();
  expect(JSON.stringify(message).length).toBeLessThan(600);
});

it('keeps sign-in recovery separate from failed-turn details', () => {
  const state = foldAgentEvent(conversationFromTranscript(transcript), {
    seq: 5,
    appendedAt: at,
    kind: {
      type: 'sign_in_required',
      message: 'Sign in to continue.',
      url: 'https://example.test/sign-in',
      code: 'SYNTHETIC-CODE',
    },
  });
  render(<MessageStream conversation={state} onApproval={vi.fn()} />);
  expect(screen.getByRole('link', { name: 'Continue sign in' })).toHaveAttribute(
    'href',
    'https://example.test/sign-in',
  );
  expect(screen.getByText('SYNTHETIC-CODE')).toBeVisible();
  expect(screen.queryByText('Technical details')).not.toBeInTheDocument();
  expect(state.turnOutcomes).toEqual({ '1': 'completed' });
});
