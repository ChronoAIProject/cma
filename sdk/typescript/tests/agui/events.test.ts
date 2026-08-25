import { describe, expect, it } from 'vitest';
import { toEvent } from '../../src/agui/events.js';
import { initialConversation, reduce } from '../../src/conversation/fold.js';
import { ProtocolError } from '../../src/errors.js';
import { KNOWN_EVENTS } from '../../src/wire.js';

/**
 * The AG-UI event vocabulary — `docs/API_AGUI.md` §3.
 *
 * @vitest-environment node
 */

const base = { threadId: 'sesn_1', runId: 'run_1', seq: 4 };
const durableProjectionPrefixes = [
  'TEXT_MESSAGE_START',
  'TEXT_MESSAGE_CONTENT',
  'TOOL_CALL_START',
  'TOOL_CALL_END',
] as const;
const durableProjectionPrefixSet = new Set<string>(durableProjectionPrefixes);
const durableFinalOrSingletonKinds = KNOWN_EVENTS.filter(
  (kind) => !durableProjectionPrefixSet.has(kind),
);

function validFrame(type: (typeof KNOWN_EVENTS)[number]): Record<string, unknown> {
  const fields: Record<(typeof KNOWN_EVENTS)[number], Record<string, unknown>> = {
    RUN_STARTED: {},
    RUN_FINISHED: {},
    RUN_ERROR: { message: 'the run failed' },
    TEXT_MESSAGE_START: { messageId: 'msg_1', role: 'assistant' },
    TEXT_MESSAGE_CONTENT: { messageId: 'msg_1', delta: 'hello' },
    TEXT_MESSAGE_END: { messageId: 'msg_1' },
    TOOL_CALL_START: {
      toolCallId: 'call_1',
      toolCallName: 'Read file',
      'cma:toolKind': 'read',
    },
    TOOL_CALL_ARGS: { toolCallId: 'call_1', delta: '{}', 'cma:truncated': false },
    TOOL_CALL_END: { toolCallId: 'call_1', 'cma:status': 'ok' },
    TOOL_CALL_RESULT: { toolCallId: 'call_1', content: 'done', 'cma:truncated': false },
    REASONING_MESSAGE_CONTENT: { messageId: 'reasoning_1', delta: 'considering' },
    STATE_DELTA: { delta: [{ op: 'replace', path: '/usage/inputTokens', value: 1 }] },
  };
  return { ...base, type, ...fields[type] };
}

describe('reading a frame', () => {
  it.each(KNOWN_EVENTS)('classifies %s', (type) => {
    expect(toEvent(validFrame(type)).type).toBe(type);
  });

  it.each(durableProjectionPrefixes)('accepts unnumbered durable prefix %s', (type) => {
    const event = toEvent({ ...validFrame(type), seq: 0 });

    expect(event).toMatchObject({
      type,
      seq: 0,
    });
    expect(event.ephemeral).not.toBe(true);
  });

  it.each(durableFinalOrSingletonKinds)(
    'rejects unnumbered durable final or singleton %s',
    (type) => {
      expect(() => toEvent({ ...validFrame(type), seq: 0 })).toThrow(ProtocolError);
    },
  );

  it('preserves an unnumbered unknown durable prefix', () => {
    const raw = { ...base, type: 'FUTURE_THING', seq: 0, futureValue: 7 };
    const event = toEvent(raw);

    expect(event).toMatchObject({ type: 'UNKNOWN', kind: 'FUTURE_THING', seq: 0 });
    expect(event.type === 'UNKNOWN' && event.raw).toEqual(raw);
  });

  it('retains an explicitly live zero sequence', () => {
    expect(toEvent({ ...validFrame('RUN_STARTED'), seq: 0, 'cma:live': true })).toMatchObject({
      type: 'RUN_STARTED',
      seq: 0,
      ephemeral: true,
    });
  });

  it('keeps an unknown kind whole rather than dropping it', () => {
    // §3: "the set grows additively" and "a client must tolerate unknown event
    // types rather than failing on them". A client that discarded one would
    // lose exactly what a newer control plane was trying to report — the same
    // property #66 established server-side for the durable log.
    const event = toEvent({ ...base, type: 'THINKING_START', detail: 'considering' });

    expect(event.type).toBe('UNKNOWN');
    if (event.type !== 'UNKNOWN') {
      throw new Error('unreachable');
    }
    expect(event.kind).toBe('THINKING_START');
    expect(event.raw.detail).toBe('considering');
    expect(event.seq).toBe(4);
  });

  it('round-trips an unknown kind without losing a field', () => {
    const frame = { ...base, type: 'FUTURE_THING', a: 1, b: { c: [2, 3] } };
    const event = toEvent(frame);
    if (event.type !== 'UNKNOWN') {
      throw new Error('expected an unknown event');
    }
    expect(event.raw).toEqual(frame);
  });

  it('reports a frame that is not an object as a protocol failure', () => {
    for (const junk of [null, undefined, 'text', 42, []]) {
      expect(() => toEvent(junk)).toThrow(ProtocolError);
    }
  });

  it('reads the tool kind and status as the kernel wrote them', () => {
    // §3: "not mapped onto a fixed set: the vocabularies differ between
    // kernels and grow without warning, and a session must not fail because
    // one invented a category."
    const start = toEvent({
      ...base,
      type: 'TOOL_CALL_START',
      toolCallId: 't1',
      'cma:toolKind': 'something_new',
      target: 'src/main.rs',
    });
    expect(start.type === 'TOOL_CALL_START' && start.toolKind).toBe('something_new');

    const end = toEvent({
      ...base,
      type: 'TOOL_CALL_END',
      toolCallId: 't1',
      'cma:status': 'partially_applied',
    });
    expect(end.type === 'TOOL_CALL_END' && end.status).toBe('partially_applied');
  });
});

describe('an interrupt', () => {
  const interrupt = {
    ...base,
    type: 'RUN_FINISHED',
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
  };

  it('carries the kernel wording verbatim', () => {
    const event = toEvent(interrupt);
    if (event.type !== 'RUN_FINISHED' || event.outcome?.type !== 'interrupt') {
      throw new Error('expected an interrupt outcome');
    }
    expect(event.outcome.interrupts[0]?.message).toBe('Run `rm -rf build/`?');
    expect(event.outcome.interrupts[0]?.reason).toBe('cma:command_execution');
  });

  it('carries every answer the kernel offered, not a hardcoded pair', () => {
    // §4: the sets differ per kernel and grow. A kernel offering three answers
    // must reach the surface with three.
    const event = toEvent(interrupt);
    if (event.type !== 'RUN_FINISHED' || event.outcome?.type !== 'interrupt') {
      throw new Error('expected an interrupt outcome');
    }
    expect(event.outcome.interrupts[0]?.options?.map((option) => option.decision)).toEqual([
      'allow',
      'deny',
      'abort',
    ]);
  });

  it('reports an interrupt with no identifier rather than silently dropping it', () => {
    expect(() =>
      toEvent({
        ...base,
        type: 'RUN_FINISHED',
        outcome: { type: 'interrupt', interrupts: [{ message: 'no id' }] },
      }),
    ).toThrow(ProtocolError);
  });
});

describe('a run error', () => {
  it('keeps the capability name intact', () => {
    // `API_Overview.md` rule 4: errors name what is missing. The name is the
    // only part a person can act on.
    const event = toEvent({ ...base, type: 'RUN_ERROR', message: 'no', capability: 'steer' });
    expect(event.type === 'RUN_ERROR' && event.capability).toBe('steer');
  });
});

describe('a state delta', () => {
  it('reads the patch rather than replacing state', () => {
    // §3: "STATE_DELTA patches, it does not replace."
    const event = toEvent({
      ...base,
      type: 'STATE_DELTA',
      delta: [{ op: 'replace', path: '/usage/inputTokens', value: 4820 }],
    });
    if (event.type !== 'STATE_DELTA') {
      throw new Error('expected a state delta');
    }
    expect(event.patch).toEqual([{ op: 'replace', path: '/usage/inputTokens', value: 4820 }]);
  });

  it('reports a malformed patch rather than silently dropping it', () => {
    expect(() => toEvent({ ...base, type: 'STATE_DELTA', delta: [{ nonsense: true }] })).toThrow(
      ProtocolError,
    );
  });

  it('preserves from and accepts the empty pointer to the document root', () => {
    const event = toEvent({
      ...base,
      type: 'STATE_DELTA',
      delta: [{ op: 'copy', from: '/mode', path: '' }],
    });
    if (event.type !== 'STATE_DELTA') {
      throw new Error('expected a state delta');
    }
    expect(event.patch[0]).toMatchObject({ op: 'copy', from: '/mode', path: '' });
  });

  it('preserves each RFC 6902 operation shape through the production fold', () => {
    const event = toEvent({
      ...base,
      type: 'STATE_DELTA',
      delta: [
        { op: 'add', path: '/extension', value: { source: 'copy me' } },
        { op: 'replace', path: '/mode', value: null },
        { op: 'copy', from: '/extension/source', path: '/extension/copied' },
        { op: 'move', from: '/extension/copied', path: '/extension/moved' },
        { op: 'test', path: '/mode', value: null },
        { op: 'remove', path: '/extension/source' },
      ],
    });
    if (event.type !== 'STATE_DELTA') {
      throw new Error('expected a state delta');
    }

    expect(event.patch).toStrictEqual([
      { op: 'add', path: '/extension', value: { source: 'copy me' } },
      { op: 'replace', path: '/mode', value: null },
      { op: 'copy', from: '/extension/source', path: '/extension/copied' },
      { op: 'move', from: '/extension/copied', path: '/extension/moved' },
      { op: 'test', path: '/mode', value: null },
      { op: 'remove', path: '/extension/source' },
    ]);
    const conversation = reduce(initialConversation(), event);
    expect(conversation.aguiState.extension).toEqual({ moved: 'copy me' });
    expect(conversation.aguiState.mode).toBeNull();
  });

  it.each(['add', 'replace', 'test'])('requires an explicit value for %s', (op) => {
    expect(() => toEvent({ ...base, type: 'STATE_DELTA', delta: [{ op, path: '/mode' }] })).toThrow(
      ProtocolError,
    );
  });

  it.each(['copy', 'move'])('requires from for %s', (op) => {
    expect(() =>
      toEvent({ ...base, type: 'STATE_DELTA', delta: [{ op, path: '/extension' }] }),
    ).toThrow(ProtocolError);
  });

  it('reports a non-string from pointer', () => {
    expect(() =>
      toEvent({
        ...base,
        type: 'STATE_DELTA',
        delta: [{ op: 'copy', from: 7, path: '/copied' }],
      }),
    ).toThrow(ProtocolError);
  });
});
