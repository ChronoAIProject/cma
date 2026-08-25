import type { Frame, Transcript } from './types.js';

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

const FULL_CONVERSATION_MARKDOWN = [
  '## Conversation surface review',
  '',
  'The shell now keeps **agent prose neutral** while its frame uses the new design aliases.',
  'Read the [review log](https://example.test/review) for the measured checks.',
  '',
  '> Review note',
  '> - Preserve the live and replayed shape.',
  '> - Keep links in the prose ink.',
  '',
  '```ts',
  "const surface = { theme: 'Inkwell', ready: true };",
  '```',
  '',
  '| Check | Result |',
  '| --- | --- |',
  '| Owner bubble | Fixed |',
  '| Shim sweep | Clean |',
  '',
  '![Conversation surface diagram](https://example.test/conversation-surface.png)',
  '',
  '- [x] Replayed from history',
  '- [ ] Approved for release',
].join('\n');

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
      seq: 0,
      messageId: 'm_1',
      role: 'assistant',
    }),
    frame(400, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_1',
      seq: 0,
      messageId: 'm_1',
      delta: 'The auth module now uses the new token type. Two call sites changed.',
    }),
    frame(40, { type: 'TEXT_MESSAGE_END', runId: 'run_1', seq: 2, messageId: 'm_1' }),
    frame(30, { type: 'RUN_FINISHED', runId: 'run_1', seq: 3, outcome: { type: 'end_turn' } }),
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
      'cma:toolKind': 'file_read',
      target: 'api/crates/layer2-core/src/session.rs',
    }),
    frame(260, {
      type: 'TOOL_CALL_END',
      runId: 'run_2',
      seq: 3,
      toolCallId: 't_1',
      'cma:status': 'ok',
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
      'cma:toolKind': 'workspace_search',
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
      'cma:toolKind': 'shell',
      target: 'cargo nextest run',
    }),
    frame(300, {
      type: 'TOOL_CALL_END',
      runId: 'run_3',
      seq: 3,
      toolCallId: 't_1',
      'cma:status': 'failed',
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
            reason: 'cma:command_execution',
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
      seq: 0,
      messageId: 'm_2',
      role: 'assistant',
    }),
    frame(220, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_5',
      seq: 0,
      messageId: 'm_2',
      delta: 'Removed build/ and rebuilt.',
    }),
    frame(30, { type: 'TEXT_MESSAGE_END', runId: 'run_5', seq: 4, messageId: 'm_2' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_5', seq: 5, outcome: { type: 'end_turn' } }),
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
            reason: 'cma:network_egress',
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
      seq: 0,
      messageId: 'm_3',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_7',
      seq: 0,
      messageId: 'm_3',
      delta: 'Understood — working from the copy already in the repository instead.',
    }),
    frame(30, { type: 'TEXT_MESSAGE_END', runId: 'run_7', seq: 4, messageId: 'm_3' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_7', seq: 5, outcome: { type: 'end_turn' } }),
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
      'cma:toolKind': 'shell',
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
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 1840 },
        { op: 'replace', path: '/usage/cost', value: '0.014' },
      ],
    }),
    frame(300, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_9',
      seq: 0,
      messageId: 'm_4',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_9',
      seq: 0,
      messageId: 'm_4',
      delta: 'Done.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_9', seq: 3, messageId: 'm_4' }),
    frame(60, {
      type: 'STATE_DELTA',
      runId: 'run_9',
      seq: 4,
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 2310 },
        { op: 'replace', path: '/usage/cost', value: '0.019' },
      ],
    }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_9', seq: 5, outcome: { type: 'end_turn' } }),
  ],
};

/** A context reading accompanied by the ceiling the runtime stated. */
export const CONTEXT_WITH_WINDOW: Transcript = {
  name: 'context-with-window',
  about: 'Context fullness arrives with the window it is drawn from.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, "STATE_DELTA is where get_usage lands"',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_context_window', seq: 1 }),
    frame(80, {
      type: 'STATE_DELTA',
      runId: 'run_context_window',
      seq: 2,
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 1840 },
        { op: 'replace', path: '/usage/outputTokens', value: 260 },
        { op: 'replace', path: '/usage/contextUsed', value: 42_100 },
        { op: 'replace', path: '/usage/contextWindow', value: 200_000 },
      ],
    }),
    frame(20, {
      type: 'RUN_FINISHED',
      runId: 'run_context_window',
      seq: 3,
      outcome: { type: 'end_turn' },
    }),
  ],
};

/** A context reading from a runtime that stated no ceiling. */
export const CONTEXT_WITHOUT_WINDOW: Transcript = {
  name: 'context-without-window',
  about: 'Context fullness arrives without an invented ceiling.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, "STATE_DELTA is where get_usage lands"',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_context_open', seq: 1 }),
    frame(80, {
      type: 'STATE_DELTA',
      runId: 'run_context_open',
      seq: 2,
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 1840 },
        { op: 'replace', path: '/usage/outputTokens', value: 260 },
        { op: 'replace', path: '/usage/contextUsed', value: 42_100 },
      ],
    }),
    frame(20, {
      type: 'RUN_FINISHED',
      runId: 'run_context_open',
      seq: 3,
      outcome: { type: 'end_turn' },
    }),
  ],
};

/** A context fall, projected before the reading that revealed it. */
export const CONTEXT_COMPACTS: Transcript = {
  name: 'context-compacts',
  about: 'The agent summarises a nearly full conversation to make room.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, "STATE_DELTA is where get_usage lands"',
  frames: [
    frame(0, {
      type: 'STATE_DELTA',
      runId: 'run_context_compacts',
      seq: 1,
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 14_200 },
        { op: 'replace', path: '/usage/outputTokens', value: 3100 },
        { op: 'replace', path: '/usage/contextUsed', value: 198_000 },
        { op: 'replace', path: '/usage/contextWindow', value: 200_000 },
      ],
    }),
    frame(80, {
      type: 'STATE_DELTA',
      runId: 'run_context_compacts',
      seq: 2,
      delta: [
        {
          op: 'add',
          path: '/contextCompactions/-',
          value: { was: 198_000, now: 42_100, window: 200_000 },
        },
      ],
    }),
    frame(20, {
      type: 'STATE_DELTA',
      runId: 'run_context_compacts',
      seq: 3,
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 14_200 },
        { op: 'replace', path: '/usage/outputTokens', value: 3100 },
        { op: 'replace', path: '/usage/contextUsed', value: 42_100 },
        { op: 'replace', path: '/usage/contextWindow', value: 200_000 },
      ],
    }),
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
      seq: 0,
      messageId: 'm_5',
      role: 'assistant',
    }),
    frame(250, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_10',
      seq: 0,
      messageId: 'm_5',
      delta: 'Finished, though I cannot tell you what it cost.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_10', seq: 2, messageId: 'm_5' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_10', seq: 3, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * A conversation with a counted stretch the runtime did not retain.
 *
 * §3: the dropped stretch arrives at `/gaps` with its frame count, which
 * §18.6 draws as a boundary in the message stream.
 */
export const ROTATES: Transcript = {
  name: 'rotates',
  about: 'A counted conversation gap, and the boundary it leaves behind.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md §3, CLAUDE.md §5',
  frames: [
    frame(0, { type: 'RUN_STARTED', runId: 'run_11', seq: 1 }),
    frame(120, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_11',
      seq: 0,
      messageId: 'm_6',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_11',
      seq: 0,
      messageId: 'm_6',
      delta: 'Starting on the migration.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_11', seq: 2, messageId: 'm_6' }),
    frame(1200, {
      type: 'STATE_DELTA',
      runId: 'run_11',
      seq: 3,
      delta: [
        {
          op: 'add',
          path: '/gaps/-',
          value: { frames: 12 },
        },
      ],
    }),
    frame(400, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_11',
      seq: 0,
      messageId: 'm_7',
      role: 'assistant',
    }),
    frame(220, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_11',
      seq: 0,
      messageId: 'm_7',
      delta: 'Continuing after the unrecorded stretch.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_11', seq: 4, messageId: 'm_7' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_11', seq: 5, outcome: { type: 'end_turn' } }),
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
      seq: 0,
      messageId: 'm_8',
      role: 'assistant',
    }),
    frame(200, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_12',
      seq: 0,
      messageId: 'm_8',
      delta: 'The second one is simpler.',
    }),
    frame(20, { type: 'TEXT_MESSAGE_END', runId: 'run_12', seq: 3, messageId: 'm_8' }),
    frame(20, { type: 'RUN_FINISHED', runId: 'run_12', seq: 4, outcome: { type: 'end_turn' } }),
  ],
};

/**
 * The complete conversation review surface in one canonical transcript.
 *
 * The final frame leaves an approval in the composer's seat. Before it, the
 * second run pauses after a live reasoning frame, so a browser can inspect
 * both liveness and the identical durable result without private test-only
 * component wiring.
 */
export const FULL_CONVERSATION: Transcript = {
  name: 'full-conversation',
  about:
    'Owner and agent turns, rich markdown, tools, reasoning, sign-in, gap, failure, usage, and approval.',
  provenance: 'authored',
  source: 'docs/API_AGUI.md sections 3-4; docs/Web_Style.md section 13',
  frames: [
    frame(0, {
      type: 'RUN_STARTED',
      runId: 'run_full_1',
      seq: 1,
      timestamp: '2026-08-23T10:00:00Z',
    }),
    frame(40, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_full_1',
      seq: 0,
      messageId: 'full_user_1',
      role: 'user',
      timestamp: '2026-08-23T10:00:01Z',
    }),
    frame(40, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_full_1',
      seq: 0,
      messageId: 'full_user_1',
      delta: 'Review the conversation surface in both appearances.',
    }),
    frame(20, {
      type: 'TEXT_MESSAGE_END',
      runId: 'run_full_1',
      seq: 2,
      messageId: 'full_user_1',
    }),
    frame(80, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_full_1',
      seq: 0,
      messageId: 'full_agent_1',
      role: 'assistant',
      timestamp: '2026-08-23T10:00:02Z',
    }),
    frame(140, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_full_1',
      seq: 0,
      messageId: 'full_agent_1',
      delta: FULL_CONVERSATION_MARKDOWN,
    }),
    frame(20, {
      type: 'TEXT_MESSAGE_END',
      runId: 'run_full_1',
      seq: 3,
      messageId: 'full_agent_1',
    }),
    frame(80, {
      type: 'TOOL_CALL_START',
      runId: 'run_full_1',
      seq: 0,
      toolCallId: 'full_tool_ok',
      'cma:toolKind': 'file_read',
      toolCallName: 'web/src/chat/MessageStream.css',
      timestamp: '2026-08-23T10:00:03Z',
    }),
    frame(40, {
      type: 'TOOL_CALL_ARGS',
      runId: 'run_full_1',
      seq: 4,
      toolCallId: 'full_tool_ok',
      delta: '{"line_start":1,"line_end":120}',
      'cma:truncated': false,
    }),
    frame(100, {
      type: 'TOOL_CALL_END',
      runId: 'run_full_1',
      seq: 0,
      toolCallId: 'full_tool_ok',
      'cma:status': 'ok',
      output: '120 lines read',
      timestamp: '2026-08-23T10:00:04Z',
    }),
    frame(30, {
      type: 'TOOL_CALL_RESULT',
      runId: 'run_full_1',
      seq: 5,
      toolCallId: 'full_tool_ok',
      content: '.turn { display: flex; }',
      'cma:truncated': false,
      timestamp: '2026-08-23T10:00:04Z',
    }),
    frame(80, {
      type: 'TOOL_CALL_START',
      runId: 'run_full_1',
      seq: 6,
      toolCallId: 'full_tool_failed',
      'cma:toolKind': 'shell',
      toolCallName: 'npm run visual:proof',
      timestamp: '2026-08-23T10:00:05Z',
    }),
    frame(120, {
      type: 'TOOL_CALL_END',
      runId: 'run_full_1',
      seq: 7,
      toolCallId: 'full_tool_failed',
      'cma:status': 'failed',
      output: 'The first capture used the stale cascade.',
      timestamp: '2026-08-23T10:00:06Z',
    }),
    frame(60, {
      type: 'STATE_DELTA',
      runId: 'run_full_1',
      seq: 8,
      delta: [
        { op: 'replace', path: '/usage/inputTokens', value: 6210 },
        { op: 'replace', path: '/usage/outputTokens', value: 1840 },
        { op: 'replace', path: '/usage/contextUsed', value: 24_800 },
        { op: 'replace', path: '/usage/contextWindow', value: 200_000 },
        { op: 'replace', path: '/usage/cost', value: '0.087' },
        {
          op: 'replace',
          path: '/signIn',
          value: {
            url: 'https://auth.example.test/device',
            code: 'ABCD-EFGH',
            message: 'Connect the account used by this review session.',
          },
        },
        {
          op: 'add',
          path: '/gaps/-',
          value: { frames: 4 },
        },
      ],
    }),
    frame(100, {
      type: 'RUN_ERROR',
      runId: 'run_full_1',
      seq: 9,
      message: 'The first visual proof used an obsolete stylesheet.',
      capability: 'render_current_styles',
      code: 'tool_execution_failed',
      timestamp: '2026-08-23T10:00:07Z',
    }),
    frame(260, {
      type: 'RUN_STARTED',
      runId: 'run_full_2',
      seq: 10,
      timestamp: '2026-08-23T10:00:08Z',
    }),
    frame(40, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_full_2',
      seq: 0,
      messageId: 'full_user_2',
      role: 'user',
      timestamp: '2026-08-23T10:00:09Z',
    }),
    frame(40, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_full_2',
      seq: 0,
      messageId: 'full_user_2',
      delta: 'Retry against the current bundle and keep the failure in the record.',
    }),
    frame(20, {
      type: 'TEXT_MESSAGE_END',
      runId: 'run_full_2',
      seq: 11,
      messageId: 'full_user_2',
    }),
    frame(160, {
      type: 'REASONING_MESSAGE_CONTENT',
      runId: 'run_full_2',
      seq: 0,
      messageId: 'full_reasoning_1',
      delta: 'Comparing the live stream with the replayed transcript.',
      'cma:live': true,
    }),
    frame(1800, {
      type: 'TEXT_MESSAGE_START',
      runId: 'run_full_2',
      seq: 0,
      messageId: 'full_agent_2',
      role: 'assistant',
      timestamp: '2026-08-23T10:00:11Z',
    }),
    frame(120, {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_full_2',
      seq: 0,
      messageId: 'full_agent_2',
      delta: 'The current bundle now produces the same durable markup after reconnect.',
    }),
    frame(20, {
      type: 'TEXT_MESSAGE_END',
      runId: 'run_full_2',
      seq: 12,
      messageId: 'full_agent_2',
    }),
    frame(80, {
      type: 'RUN_FINISHED',
      runId: 'run_full_2',
      seq: 13,
      timestamp: '2026-08-23T10:00:12Z',
      outcome: {
        type: 'interrupt',
        interrupts: [
          {
            id: 'full_approval_1',
            reason: 'cma:release_review',
            message: 'Approve the restyled conversation surface?',
            options: [
              { decision: 'allow', label: 'Approve', affirmative: true },
              { decision: 'deny', label: 'Keep reviewing' },
            ],
          },
        ],
      },
    }),
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
  CONTEXT_WITH_WINDOW,
  CONTEXT_WITHOUT_WINDOW,
  CONTEXT_COMPACTS,
  NO_USAGE,
  ROTATES,
  UNKNOWN_KIND,
  FULL_CONVERSATION,
];

export function transcriptNamed(name: string): Transcript {
  const found = TRANSCRIPTS.find((transcript) => transcript.name === name);
  if (found === undefined) {
    throw new Error(`no transcript named ${name}`);
  }
  return found;
}
