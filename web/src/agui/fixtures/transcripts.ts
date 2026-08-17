import type { Frame, Transcript } from './types';

/**
 * The recorded conversations the chat surface is built and reviewed against.
 *
 * **Every one of these is `authored`**, written from `docs/API_AGUI.md` rather
 * than captured from a session. That is stated on each fixture rather than
 * here, so it travels with the transcript: the provenance of a fixture is a
 * fact about that fixture, and one kept only in a README is one that is wrong
 * the first time somebody replaces a transcript without updating it.
 *
 * They prove a component handles the shape the document describes. They prove
 * nothing about what a kernel actually emits, and they are not evidence that
 * anything works end to end — #130 is what does that.
 */

const THREAD = 'sesn_01JD8Z3K7';

/** One frame, with the gap before it. */
function frame(afterMs: number, event: Record<string, unknown>): Frame {
  return { afterMs, event: { threadId: THREAD, ...event } };
}

/** A plain exchange: a prompt, a turn, a finished message. */
export const PLAIN_TURN: Transcript = {
  name: 'plain-turn',
  about: 'One prompt and one answer, with nothing else in the way.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_1', seq: 1 }),
    frame(120, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_1',
      seq: 2,
      messageId: 'm_1',
      role: 'assistant',
    }),
    frame(400, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_1',
      seq: 3,
      messageId: 'm_1',
      delta: 'The auth module now uses the new token type. Two call sites changed.',
    }),
    frame(40, { type: 'TEXT_MESSAGE_END', runId: 'run_1', seq: 4, messageId: 'm_1' }),
    frame(30, { type: 'RUN_FINISHED', runId: 'run_1', seq: 5, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * Two tool calls, one still running when the turn is read.
 *
 * The case §18.2 is written for: a call that has not finished has to be
 * distinguishable from one that has without opening either.
 */
export const TWO_TOOL_CALLS: Transcript = {
  name: 'two-tool-calls',
  about: 'Two tool calls; the second is still running when the transcript ends.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, "Tool kinds are the runtime\'s own word"',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_2', seq: 1 }),
    frame(90, {
      type: 'TOOL_CALL_START',
      runId: 'run_2',
      seq: 2,
      toolCallId: 't_1',
      'chrono:toolKind': 'file_read',
      target: 'api/crates/layer2-core/src/session.rs',
    }),
    frame(260, {
      type: 'TOOL_CALL_END',
      runId: 'run_2',
      seq: 3,
      toolCallId: 't_1',
      'chrono:status': 'ok',
      output: 'pub struct Session { … }',
    }),
    frame(80, {
      type: 'TOOL_CALL_START',
      runId: 'run_2',
      seq: 4,
      toolCallId: 't_2',
      // A kind no client has a mapping for, on purpose: §3 says the
      // vocabularies grow and a session must not fail because one invented a
      // category.
      'chrono:toolKind': 'workspace_search',
      target: 'granted_services',
    }),
  ],
};

/** A failed tool call, which must not read like a finished one. */
export const FAILED_TOOL_CALL: Transcript = {
  name: 'failed-tool-call',
  about: 'A tool call that failed. §18.2 requires it to be distinguishable.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_3', seq: 1 }),
    frame(70, {
      type: 'TOOL_CALL_START',
      runId: 'run_3',
      seq: 2,
      toolCallId: 't_1',
      'chrono:toolKind': 'shell',
      target: 'cargo nextest run',
    }),
    frame(300, {
      type: 'TOOL_CALL_END',
      runId: 'run_3',
      seq: 3,
      toolCallId: 't_1',
      'chrono:status': 'failed',
      output: 'error: no such command: `nextest`',
    }),
    frame(40, { type: 'RUN_FINISHED', runId: 'run_3', seq: 4, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * An approval requested and granted.
 *
 * §4's shape: the run **ends** to ask, and a new run resumes. The kernel
 * offers three answers, so three arrive.
 */
export const APPROVAL_GRANTED: Transcript = {
  name: 'approval-granted',
  about: 'The run ends on an interrupt, the answer is given, the run resumes.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §4',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_4', seq: 1 }),
    frame(140, {
      type: 'RUN_FINISHED',
      runId: 'run_4',
      seq: 2,
      outcome: {
        type: 'interrupt',
        interrupts: [
          {
            id: 'apr_7Q2',
            reason: 'chrono:command_execution',
            message: 'Run `rm -rf build/`?',
            options: [
              { decision: 'allow', label: 'Allow', affirmative: true },
              { decision: 'deny', label: 'Deny' },
              { decision: 'abort', label: 'Stop the turn' },
            ],
          },
        ],
      },
    }),
    frame(900, { type: 'RUN_STARTED', runId: 'run_5', seq: 3 }),
    frame(120, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_5',
      seq: 4,
      messageId: 'm_2',
      role: 'assistant',
    }),
    frame(220, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_5',
      seq: 5,
      messageId: 'm_2',
      delta: 'Removed build/ and rebuilt.',
    }),
    frame(30, { type: 'TEXT_MESSAGE_END', runId: 'run_5', seq: 6, messageId: 'm_2' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_5', seq: 7, outcome: { type: 'end_turn' } }),
  ],
};

/** An approval requested and denied — a different intention from aborting. */
export const APPROVAL_DENIED: Transcript = {
  name: 'approval-denied',
  about: 'The answer is a refusal, and the agent tries something else.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §4',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_6', seq: 1 }),
    frame(140, {
      type: 'RUN_FINISHED',
      runId: 'run_6',
      seq: 2,
      outcome: {
        type: 'interrupt',
        interrupts: [
          {
            id: 'apr_8R3',
            reason: 'chrono:network_egress',
            message: 'Fetch https://example.invalid/spec.json?',
            options: [
              { decision: 'allow', label: 'Allow', affirmative: true },
              { decision: 'deny', label: 'Deny' },
            ],
          },
        ],
      },
    }),
    frame(700, { type: 'RUN_STARTED', runId: 'run_7', seq: 3 }),
    frame(180, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_7',
      seq: 4,
      messageId: 'm_3',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_7',
      seq: 5,
      messageId: 'm_3',
      delta: 'Understood — working from the copy already in the repository instead.',
    }),
    frame(30, { type: 'TEXT_MESSAGE_END', runId: 'run_7', seq: 6, messageId: 'm_3' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_7', seq: 7, outcome: { type: 'end_turn' } }),
  ],
};

/** A turn stopped part-way through. */
export const INTERRUPTED_TURN: Transcript = {
  name: 'interrupted-turn',
  about: 'A turn interrupted mid-stream. The surface reflects it without a reload.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §5',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_8', seq: 1 }),
    frame(90, {
      type: 'TOOL_CALL_START',
      runId: 'run_8',
      seq: 2,
      toolCallId: 't_9',
      'chrono:toolKind': 'shell',
      target: 'cargo build --release',
    }),
    frame(600, {
      type: 'RUN_FINISHED',
      runId: 'run_8',
      seq: 3,
      outcome: { type: 'user_interrupted' },
    }),
  ],
};

/**
 * A session that reports usage.
 *
 * §3: usage lands as RFC 6902 patches against `/usage`, so a chat surface can
 * show spend without polling.
 */
export const REPORTS_USAGE: Transcript = {
  name: 'reports-usage',
  about: 'Usage arrives as state patches during the turn.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, "STATE_DELTA is where get_usage lands"',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_9', seq: 1 }),
    frame(150, {
      type: 'STATE_DELTA',
      runId: 'run_9',
      seq: 2,
      patch: [
        { op: 'replace', path: '/usage/tokens', value: 1840 },
        { op: 'replace', path: '/usage/cost', value: '0.014' },
      ],
    }),
    frame(300, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_9',
      seq: 3,
      messageId: 'm_4',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_9',
      seq: 4,
      messageId: 'm_4',
      delta: 'Done.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_9', seq: 5, messageId: 'm_4' }),
    frame(60, {
      type: 'STATE_DELTA',
      runId: 'run_9',
      seq: 6,
      patch: [
        { op: 'replace', path: '/usage/tokens', value: 2310 },
        { op: 'replace', path: '/usage/cost', value: '0.019' },
      ],
    }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_9', seq: 7, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * A session that reports none.
 *
 * The absence is what matters (§18.5): a session with no `usage` grant must
 * not read as one that has spent nothing.
 */
export const NO_USAGE: Transcript = {
  name: 'no-usage',
  about: 'A session with no usage capability. Nothing about it says zero.',
  provenance: 'authored',
  source: 'Web_Style.md §18.5',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_10', seq: 1 }),
    frame(200, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_10',
      seq: 2,
      messageId: 'm_5',
      role: 'assistant',
    }),
    frame(250, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_10',
      seq: 3,
      messageId: 'm_5',
      delta: 'Finished, though I cannot tell you what it cost.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_10', seq: 4, messageId: 'm_5' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_10', seq: 5, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * A session carried onto a newer runtime mid-conversation.
 *
 * §3: a stretch of conversation the runtime had to drop arrives at `/gaps`,
 * which §18.6 draws as a boundary in the message stream at `rotated_at`.
 */
export const ROTATES: Transcript = {
  name: 'rotates',
  about: 'A rotation mid-conversation, and the boundary it leaves behind.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, CLAUDE.md §5',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_11', seq: 1 }),
    frame(120, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_11',
      seq: 2,
      messageId: 'm_6',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_11',
      seq: 3,
      messageId: 'm_6',
      delta: 'Starting on the migration.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_11', seq: 4, messageId: 'm_6' }),
    frame(1200, {
      type: 'STATE_DELTA',
      runId: 'run_11',
      seq: 5,
      patch: [
        {
          op: 'add',
          path: '/gaps/-',
          value: { rotatedAt: '2026-08-18T09:12:44Z', reason: 'runtime_version' },
        },
      ],
    }),
    frame(400, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_11',
      seq: 6,
      messageId: 'm_7',
      role: 'assistant',
    }),
    frame(220, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_11',
      seq: 7,
      messageId: 'm_7',
      delta: 'Continuing from where the snapshot was taken.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_11', seq: 8, messageId: 'm_7' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_11', seq: 9, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * A kind no build has heard of.
 *
 * §3: the set grows additively and a client must tolerate one. This is the
 * fixture that proves a component does rather than that the parser does.
 */
export const UNKNOWN_KIND: Transcript = {
  name: 'unknown-kind',
  about: 'An event kind this build does not know, in the middle of a turn.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, §7',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_12', seq: 1 }),
    frame(120, {
      type: 'THINKING_START',
      runId: 'run_12',
      seq: 2,
      detail: 'weighing two approaches',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_12',
      seq: 3,
      messageId: 'm_8',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_12',
      seq: 4,
      messageId: 'm_8',
      delta: 'The second one is simpler.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_12', seq: 5, messageId: 'm_8' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_12', seq: 6, outcome: { type: 'end_turn' } }),
  ],
};

/** Every fixture, so a test can walk them without listing them again. */
export const TRANSCRIPTS: readonly Transcript[] = [
  PLAIN_TURN,
  TWO_TOOL_CALLS,
  FAILED_TOOL_CALL,
  APPROVAL_GRANTED,
  APPROVAL_DENIED,
  INTERRUPTED_TURN,
  REPORTS_USAGE,
  NO_USAGE,
  ROTATES,
  UNKNOWN_KIND,
];

export function transcriptNamed(name: string): Transcript {
  const found = TRANSCRIPTS.find((transcript) => transcript.name === name);
  if (found === undefined) {
    throw new Error(`no transcript named ${name}`);
  }
  return found;
}
