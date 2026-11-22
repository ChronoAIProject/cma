import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import {
  loadOutboundIntent,
  outboundRecoveryKey,
  outboundStorageKey,
  storeOutboundIntent,
  useOutboundJournal,
  type OutboundIntent,
} from './outboundJournal';

const intent: OutboundIntent = {
  draft: {
    key: 'original-key',
    body: { source_id: 'relay_telegram', route_id: 'route', message: { text: 'original draft' } },
  },
  receipt: {
    delivery_id: 'delivery',
    route_id: 'route',
    status: 'committed',
    expires_at: 2000000000,
    rendering: { outcome: 'rendered' },
    delivery: { outcome: 'unknown', reason: 'provider_failure' },
  },
  uncertain: true,
};
const activeKey = outboundStorageKey('alice', 'record');
const previousKey = outboundRecoveryKey('alice', 'record');
function setup() {
  const original = sessionStorage;
  vi.stubGlobal('sessionStorage', {
    getItem: original.getItem.bind(original),
    setItem: original.setItem.bind(original),
    removeItem: original.removeItem.bind(original),
    clear: original.clear.bind(original),
  });
  vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', 'true');
  storeOutboundIntent(activeKey, intent);
  return renderHook(() => useOutboundJournal('alice', 'record'));
}
afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it('archives exact intent before removing the compatible active slot', () => {
  const { result } = setup();
  const writes = vi.spyOn(sessionStorage, 'setItem');
  const remove = vi.spyOn(sessionStorage, 'removeItem');
  act(() => {
    expect(result.current.archive()).toBe(true);
  });
  expect(writes.mock.invocationCallOrder[0]).toBeLessThan(remove.mock.invocationCallOrder[0]!);
  expect(sessionStorage.getItem(activeKey)).toBeNull();
  expect(loadOutboundIntent(previousKey).intent).toEqual(intent);
  expect(result.current.previous).toEqual(intent);
  expect(result.current.intent).toBeUndefined();
  expect(outboundRecoveryKey('bob', 'record')).not.toBe(previousKey);
  expect(outboundRecoveryKey('alice', 'other-record')).not.toBe(previousKey);
});

it.each([undefined, 'false'])(
  'refuses new recovery writes with the flag %s and retains readable recovery',
  (flag) => {
    const mounted = setup();
    act(() => {
      mounted.result.current.archive();
    });
    mounted.unmount();
    storeOutboundIntent(activeKey, intent);
    vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', flag);
    const { result } = renderHook(() => useOutboundJournal('alice', 'record'));
    expect(result.current.previous).toEqual(intent);
    act(() => {
      expect(result.current.archive()).toBe(false);
    });
    expect(loadOutboundIntent(activeKey).intent).toEqual(intent);
  },
);

it('never evicts an occupied recovery slot', () => {
  const prior = { ...intent, draft: { ...intent.draft, key: 'prior-key' } };
  storeOutboundIntent(previousKey, prior);
  const { result } = setup();
  act(() => {
    expect(result.current.archive()).toBe(false);
  });
  expect(result.current.recoveryError).toContain('still retained');
  expect(loadOutboundIntent(previousKey).intent).toEqual(prior);
  expect(loadOutboundIntent(activeKey).intent).toEqual(intent);
  act(() => {
    expect(result.current.retainPrevious()).toBe(false);
  });
});

it('refuses to overwrite an unreadable recovery slot', () => {
  sessionStorage.setItem(previousKey, '{broken');
  const { result } = setup();
  act(() => {
    expect(result.current.archive()).toBe(false);
  });
  expect(sessionStorage.getItem(previousKey)).toBe('{broken');
  expect(result.current.intent).toEqual(intent);
});

it('preserves the active intent on archive storage failure', () => {
  const { result } = setup();
  vi.spyOn(sessionStorage, 'setItem').mockImplementation(() => {
    throw new Error('full');
  });
  act(() => {
    expect(result.current.archive()).toBe(false);
  });
  expect(result.current.intent).toEqual(intent);
  expect(result.current.error).toBeUndefined();
  expect(result.current.recoveryError).toContain('could not be saved');
  expect(loadOutboundIntent(activeKey).intent).toEqual(intent);
});

it('recovers an interrupted archive without losing or replacing either intent', () => {
  const mounted = setup();
  const remove = vi.spyOn(sessionStorage, 'removeItem').mockImplementation(() => {
    throw new Error('unavailable');
  });
  act(() => {
    expect(mounted.result.current.archive()).toBe(false);
  });
  expect(loadOutboundIntent(activeKey).intent).toEqual(intent);
  expect(loadOutboundIntent(previousKey).intent).toEqual(intent);
  mounted.unmount();
  remove.mockRestore();
  const { result } = renderHook(() => useOutboundJournal('alice', 'record'));
  act(() => {
    expect(result.current.archive()).toBe(true);
  });
  expect(sessionStorage.getItem(activeKey)).toBeNull();
  expect(result.current.previous).toEqual(intent);
});

it('only dismisses a previous receipt after authoritative terminal confirmation', () => {
  const { result } = setup();
  act(() => {
    result.current.archive();
  });
  const resolved = {
    ...intent,
    receipt: { ...intent.receipt!, delivery: { outcome: 'confirmed' as const } },
    uncertain: false,
  };
  act(() => {
    expect(result.current.retainPrevious(resolved)).toBe(true);
  });
  act(() => {
    expect(result.current.retainPrevious()).toBe(true);
  });
  expect(sessionStorage.getItem(previousKey)).toBeNull();
});

it('retains the old previous receipt when a status update cannot persist', () => {
  const { result } = setup();
  act(() => {
    result.current.archive();
  });
  vi.spyOn(sessionStorage, 'setItem').mockImplementation(() => {
    throw new Error('full');
  });
  act(() => {
    expect(result.current.retainPrevious({ ...intent, uncertain: false })).toBe(false);
  });
  expect(result.current.previous).toEqual(intent);
  expect(result.current.recoveryError).toContain('status could not be saved');
});

it.each([1, 2])(
  'reads retained v%s journals with expiry and rendering and preserves their request',
  (version) => {
    const saved = structuredClone(intent);
    if (version === 2)
      saved.draft.body.message = {
        native_attachments: [
          { kind: 'image', source: { type: 'url', url: 'https://media.example/image.png' } },
        ],
      };
    sessionStorage.setItem(activeKey, JSON.stringify({ version, intent: saved }));
    expect(loadOutboundIntent(activeKey).intent).toEqual(saved);
    storeOutboundIntent(activeKey, saved);
    expect(JSON.parse(sessionStorage.getItem(activeKey)!).version).toBe(version);
  },
);
