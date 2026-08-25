import { describe, expect, it } from 'vitest';
import { ProtocolError, applyStateDelta, initialState } from '../../src/index.js';
import type { JsonPatch } from '../../src/agui/index.js';

describe('the public AG-UI state fold', () => {
  it('applies replacements without mutating its input document', () => {
    const state = initialState();
    const next = applyStateDelta(state, [
      { op: 'replace', path: '/usage/inputTokens', value: 41 },
      { op: 'replace', path: '/usage/outputTokens', value: 9 },
    ]);

    expect(next.usage).toMatchObject({ inputTokens: 41, outputTokens: 9 });
    expect(state.usage).toMatchObject({ inputTokens: null, outputTokens: null });
    expect(next).not.toBe(state);
    expect(next.usage).not.toBe(state.usage);
  });

  it('appends every value in operation order and does not retain patch-owned objects', () => {
    const first = { frames: 2 };
    const second = { frames: 3 };
    const next = applyStateDelta(initialState(), [
      { op: 'add', path: '/gaps/-', value: first },
      { op: 'add', path: '/gaps/-', value: second },
    ]);

    first.frames = 99;
    second.frames = 100;
    expect(next.gaps).toEqual([{ frames: 2 }, { frames: 3 }]);
  });

  it('keeps independent session documents isolated', () => {
    const first = initialState();
    const second = initialState();
    const folded = applyStateDelta(first, [{ op: 'add', path: '/gaps/-', value: { frames: 1 } }]);

    expect(folded.gaps).toEqual([{ frames: 1 }]);
    expect(first.gaps).toEqual([]);
    expect(second.gaps).toEqual([]);
    expect(first.gaps).not.toBe(second.gaps);
  });

  it('retains additive fields a newer control plane introduces', () => {
    const next = applyStateDelta(initialState(), [
      { op: 'add', path: '/somethingElse', value: { status: 'new' } },
      { op: 'add', path: '/usage/cacheTokens', value: 12 },
    ]);

    expect(next.somethingElse).toEqual({ status: 'new' });
    expect(next.usage.cacheTokens).toBe(12);
  });

  it('supports valid copy and move operations, including their from pointer', () => {
    const next = applyStateDelta(initialState(), [
      { op: 'replace', path: '/mode', value: 'safe' },
      { op: 'add', path: '/extension', value: {} },
      { op: 'copy', from: '/mode', path: '/extension/copied' },
      { op: 'move', from: '/extension/copied', path: '/extension/moved' },
    ]);

    expect(next.extension).toEqual({ moved: 'safe' });
    expect(next.mode).toBe('safe');
  });

  it('decodes RFC 6901 tokens exactly', () => {
    const next = applyStateDelta(initialState(), [
      { op: 'add', path: '/extension', value: {} },
      { op: 'add', path: '/extension/slash~1member', value: 1 },
      { op: 'add', path: '/extension/tilde~0member', value: 2 },
      { op: 'add', path: '/extension/', value: 3 },
    ]);

    expect(next.extension).toEqual({ 'slash/member': 1, 'tilde~member': 2, '': 3 });
  });

  it.each([
    { patches: [{ op: 'copy', from: 'not-a-pointer', path: '/extension' }] },
    { patches: [{ op: 'move', from: 'not-a-pointer', path: '/extension' }] },
    { patches: [{ op: 'add', path: '/invalid~2escape', value: true }] },
    { patches: [{ op: 'add', path: '/trailing~', value: true }] },
    { patches: [{ op: 'copy', from: '/invalid~2escape', path: '/extension' }] },
  ] satisfies readonly { readonly patches: readonly JsonPatch[] }[])(
    'rejects invalid RFC 6901 pointer %#',
    ({ patches }) => {
      expect(() => applyStateDelta(initialState(), patches)).toThrow(ProtocolError);
    },
  );

  it('rejects a move from the root or any value into its proper descendant', () => {
    expect(() =>
      applyStateDelta(initialState(), [{ op: 'move', from: '', path: '/extension' }]),
    ).toThrow(ProtocolError);
    expect(() =>
      applyStateDelta(initialState(), [
        { op: 'add', path: '/extension', value: { child: null } },
        { op: 'move', from: '/extension', path: '/extension/child' },
      ]),
    ).toThrow(ProtocolError);
  });

  it('copies the root by value without creating a cycle', () => {
    const next = applyStateDelta(initialState(), [
      { op: 'replace', path: '/mode', value: 'before-copy' },
      { op: 'copy', from: '', path: '/extension' },
    ]);

    expect(next.extension).toMatchObject({ mode: 'before-copy' });
    expect(next.extension).not.toHaveProperty('extension');
  });

  it('uses own JSON members without confusing them with prototype members', () => {
    const next = applyStateDelta(initialState(), [
      {
        op: 'add',
        path: '/constructor',
        value: { prototype: { source: 'own member' } },
      },
      { op: 'copy', from: '/constructor/prototype/source', path: '/toString' },
      { op: 'replace', path: '/toString', value: 'replaced own member' },
      { op: 'add', path: '/hasOwnProperty', value: { nested: true } },
      {
        op: 'test',
        path: '',
        value: {
          ...initialState(),
          constructor: { prototype: { source: 'own member' } },
          toString: 'replaced own member',
          hasOwnProperty: { nested: true },
        },
      },
    ]);

    expect(Object.prototype.hasOwnProperty.call(next, 'constructor')).toBe(true);
    expect(Object.prototype.hasOwnProperty.call(next, 'toString')).toBe(true);
    expect(Object.prototype.hasOwnProperty.call(next, 'hasOwnProperty')).toBe(true);
    expect(next.toString).toBe('replaced own member');
  });

  it('rejects inherited operation metadata after cloning an ordinary patch object', () => {
    const members = ['op', 'path', 'from'] as const;
    const descriptors = members.map(
      (member) => [member, Object.getOwnPropertyDescriptor(Object.prototype, member)] as const,
    );
    Object.defineProperties(Object.prototype, {
      op: { configurable: true, value: 'remove' },
      path: { configurable: true, value: '/mode' },
      from: { configurable: true, value: '/mode' },
    });

    let missingOperationError: unknown;
    let missingFromError: unknown;
    try {
      try {
        applyStateDelta(initialState(), [{} as JsonPatch]);
      } catch (error) {
        missingOperationError = error;
      }
      try {
        applyStateDelta(initialState(), [{ op: 'copy', path: '/extension' }]);
      } catch (error) {
        missingFromError = error;
      }
    } finally {
      for (const [member, descriptor] of descriptors) {
        if (descriptor === undefined) {
          Reflect.deleteProperty(Object.prototype, member);
        } else {
          Object.defineProperty(Object.prototype, member, descriptor);
        }
      }
    }

    expect(missingOperationError).toBeInstanceOf(ProtocolError);
    expect(missingFromError).toBeInstanceOf(ProtocolError);
  });

  it.each([
    { patches: [{ op: 'copy', from: '/constructor', path: '/extension' }] },
    { patches: [{ op: 'move', from: '/toString', path: '/extension' }] },
    { patches: [{ op: 'replace', path: '/toString', value: 'not inherited' }] },
    { patches: [{ op: 'remove', path: '/hasOwnProperty' }] },
    { patches: [{ op: 'test', path: '/constructor', value: {} }] },
  ] satisfies readonly { readonly patches: readonly JsonPatch[] }[])(
    'does not traverse inherited member %#',
    ({ patches }) => {
      expect(() => applyStateDelta(initialState(), patches)).toThrow(ProtocolError);
    },
  );

  it('uses canonical JSON array indices and evaluates a move target after removal', () => {
    const next = applyStateDelta(initialState(), [
      { op: 'add', path: '/extension', value: ['first', 'second', 'third'] },
      { op: 'move', from: '/extension/0', path: '/extension/2' },
      { op: 'add', path: '/extension/-', value: 'fourth' },
    ]);
    expect(next.extension).toEqual(['second', 'third', 'first', 'fourth']);

    const invalidMoves: readonly (readonly JsonPatch[])[] = [
      [{ op: 'add', path: '/extension/01', value: 'leading zero' }],
      [{ op: 'add', path: '/extension/member', value: 'array property' }],
      [{ op: 'remove', path: '/extension/-' }],
      [{ op: 'add', path: '/extension/4', value: 'out of bounds' }],
      [{ op: 'move', from: '/extension/0', path: '/extension/3' }],
    ];
    for (const patches of invalidMoves) {
      expect(() =>
        applyStateDelta(initialState(), [
          { op: 'add', path: '/extension', value: ['first', 'second', 'third'] },
          ...patches,
        ]),
      ).toThrow(ProtocolError);
    }
  });

  it('accepts the empty JSON Pointer for a root test and root replacement', () => {
    const state = initialState();
    const replacement = initialState();
    replacement.mode = 'root-replaced';

    expect(applyStateDelta(state, [{ op: 'test', path: '', value: state }])).toEqual(state);
    const next = applyStateDelta(state, [{ op: 'replace', path: '', value: replacement }]);
    replacement.mode = 'changed-afterward';
    expect(next.mode).toBe('root-replaced');
  });

  it.each([
    { patches: [{ op: 'add', path: '/missing/child', value: true }] },
    { patches: [{ op: 'replace', path: '/missing', value: true }] },
    { patches: [{ op: 'remove', path: '/missing' }] },
    { patches: [{ op: 'add', path: 'not-a-pointer', value: true }] },
    { patches: [{ op: 'merge', path: '/mode', value: 'unsafe' }] },
    { patches: [{ op: '_get', path: '/mode' }] },
    { patches: [{ op: 'replace', path: '/mode' }] },
  ] satisfies readonly { readonly patches: readonly JsonPatch[] }[])(
    'rejects malformed operation %#',
    ({ patches }) => {
      expect(() => applyStateDelta(initialState(), patches)).toThrow(ProtocolError);
    },
  );

  it.each(['move', 'copy'] as const)('requires from for %s', (op) => {
    expect(() => applyStateDelta(initialState(), [{ op, path: '/mode' }])).toThrow(ProtocolError);
  });

  it('turns a failed test into an atomic ProtocolError', () => {
    const state = initialState();
    expect(() =>
      applyStateDelta(state, [
        { op: 'replace', path: '/mode', value: 'changed-on-the-clone' },
        { op: 'test', path: '/mode', value: 'not-the-value' },
      ]),
    ).toThrow(ProtocolError);
    expect(state.mode).toBeNull();
  });

  it('rejects values and documents that JSON cannot represent', () => {
    const cycle: Record<string, unknown> = {};
    cycle.self = cycle;
    const cyclicState = initialState();
    cyclicState.extension = cyclicState;

    expect(() =>
      applyStateDelta(initialState(), [{ op: 'add', path: '/extension', value: cycle }]),
    ).toThrow(ProtocolError);
    expect(() =>
      applyStateDelta(initialState(), [{ op: 'add', path: '/extension', value: Number.NaN }]),
    ).toThrow(ProtocolError);
    expect(() => applyStateDelta(cyclicState, [])).toThrow(ProtocolError);
  });

  it.each([
    { patches: [{ op: 'add', path: '/__proto__', value: { polluted: true } }] },
    { patches: [{ op: 'add', path: '/extension/__proto__/polluted', value: true }] },
    { patches: [{ op: 'copy', from: '/__proto__', path: '/extension' }] },
    { patches: [{ op: 'move', from: '/__proto__/polluted', path: '/extension' }] },
  ] satisfies readonly { readonly patches: readonly JsonPatch[] }[])(
    'keeps source and target prototype pointer %# forbidden',
    ({ patches }) => {
      expect(() => applyStateDelta(initialState(), patches)).toThrow(ProtocolError);
    },
  );

  it('does not modify the object prototype after rejected patches', () => {
    expect(Object.prototype).not.toHaveProperty('polluted');
  });
});
