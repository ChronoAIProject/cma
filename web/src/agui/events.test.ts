import { describe, expect, it } from 'vitest';
import { KNOWN_EVENTS, toEvent } from './events';

/**
 * The AG-UI event vocabulary — `docs/API_AGUI.md` §3.
 *
 * @vitest-environment node
 */

const base = { threadId: 'sesn_1', runId: 'run_1', seq: 4 };

describe('reading a frame', () => {
  it.each(KNOWN_EVENTS)('classifies %s', (type) => {
    expect(toEvent({ ...base, type }).type).toBe(type);
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

  it('never throws on a frame that is not an object', () => {
    // A trust boundary. The honest failure is "we do not know what this is".
    for (const junk of [null, undefined, 'text', 42, []]) {
      expect(() => toEvent(junk)).not.toThrow();
      expect(toEvent(junk).type).toBe('UNKNOWN');
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
      'chrono:toolKind': 'something_new',
      target: 'src/main.rs',
    });
    expect(start.type === 'TOOL_CALL_START' && start.toolKind).toBe('something_new');

    const end = toEvent({
      ...base,
      type: 'TOOL_CALL_END',
      toolCallId: 't1',
      'chrono:status': 'partially_applied',
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
  };

  it('carries the kernel wording verbatim', () => {
    const event = toEvent(interrupt);
    if (event.type !== 'RUN_FINISHED' || event.outcome?.type !== 'interrupt') {
      throw new Error('expected an interrupt outcome');
    }
    expect(event.outcome.interrupts[0]?.message).toBe('Run `rm -rf build/`?');
    expect(event.outcome.interrupts[0]?.reason).toBe('chrono:command_execution');
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

  it('drops an interrupt with no identifier rather than rendering one nobody can answer', () => {
    const event = toEvent({
      ...base,
      type: 'RUN_FINISHED',
      outcome: { type: 'interrupt', interrupts: [{ message: 'no id' }] },
    });
    if (event.type !== 'RUN_FINISHED' || event.outcome?.type !== 'interrupt') {
      throw new Error('expected an interrupt outcome');
    }
    expect(event.outcome.interrupts).toEqual([]);
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
      delta: [{ op: 'replace', path: '/usage/inputTokens', value: 4820 }, { nonsense: true }],
    });
    if (event.type !== 'STATE_DELTA') {
      throw new Error('expected a state delta');
    }
    expect(event.patch).toEqual([{ op: 'replace', path: '/usage/inputTokens', value: 4820 }]);
  });
});
