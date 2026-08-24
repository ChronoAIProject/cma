import { describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import {
  createSessionDriverRegistry,
  driverKindFor,
  useSessionDrivers,
  type DriverRequest,
} from './drivers';
import type { Session, SessionLifecycle } from './types';
import type { TerminalDriver } from '../terminal/types';
import { NO_CAPABILITIES } from '@chronoai/cma-sdk';

function session(id: string, lifecycle: SessionLifecycle, stopReason?: 'error' | 'other'): Session {
  return {
    id,
    kernel: 'codex',
    lifecycle,
    createdAt: 0,
    stopReason,
    capabilities: NO_CAPABILITIES,
  };
}

/** A factory that hands back inert drivers and records what it was asked for. */
function recordingFactory() {
  const requests: DriverRequest[] = [];
  const factory = (request: DriverRequest): TerminalDriver => {
    requests.push(request);
    return { attach: vi.fn(), send: vi.fn(), resize: vi.fn(), detach: vi.fn() };
  };
  return { factory, requests };
}

describe('driverKindFor', () => {
  it('reads a running session over the relay', () => {
    expect(driverKindFor(session('a', 'ready'))).toBe('live');
    expect(driverKindFor(session('a', 'running'))).toBe('live');
  });

  it('reads a session with no sandbox from its stored transcript', () => {
    // Stopping releases what a session costs; it does not hide what it did.
    expect(driverKindFor(session('a', 'idle'))).toBe('transcript');
    expect(driverKindFor(session('a', 'terminated', 'error'))).toBe('transcript');
  });

  it('gives a session with no screen yet nothing to read', () => {
    expect(driverKindFor(session('a', 'provisioning'))).toBe('none');
    expect(driverKindFor(session('a', 'rotating'))).toBe('none');
  });
});

describe('createSessionDriverRegistry', () => {
  it('pairs every session with a driver, or with none where there is nothing to read', () => {
    const { factory } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);

    const nodes = registry.resolve([session('a', 'running'), session('b', 'provisioning')], null);

    expect(nodes.map((node) => node.session.id)).toEqual(['a', 'b']);
    expect(nodes[0]?.driver).not.toBeNull();
    expect(nodes[1]?.driver).toBeNull();
  });

  it('builds nothing the second time the same sessions are resolved', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const first = registry.resolve([session('a', 'running')], null);

    // A new array holding an equivalent session, as setSessions produces on
    // every refresh. Rebuilding here would reconnect a live socket.
    const second = registry.resolve([session('a', 'running')], null);

    expect(second[0]?.driver).toBe(first[0]?.driver);
    expect(requests).toHaveLength(1);
  });

  it('keeps one driver across ready and running, which mean the same thing', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const first = registry.resolve([session('a', 'ready')], null);

    const second = registry.resolve([session('a', 'running')], null);

    expect(second[0]?.driver).toBe(first[0]?.driver);
    expect(requests).toHaveLength(1);
  });

  it('replaces the driver when the session stops', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const live = registry.resolve([session('a', 'running')], null);

    const stopped = registry.resolve([session('a', 'idle')], null);

    // A stopped session reads its transcript; carrying the relay driver over
    // would leave it trying to reconnect to a sandbox that is gone.
    expect(stopped[0]?.driver).not.toBe(live[0]?.driver);
    expect(requests.map((request) => request.kind)).toEqual(['live', 'transcript']);
  });

  it('distinguishes a stopped session from an ended one', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    registry.resolve([session('a', 'idle')], null);

    registry.resolve([session('a', 'terminated', 'error')], null);

    // Both read a transcript, but each closes with its own reason, and a driver
    // carried across would print the wrong one.
    expect(requests).toHaveLength(2);
  });

  it('gives the opening turn to the session it was captured for, and to no other', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);

    registry.resolve([session('a', 'running'), session('b', 'running')], {
      text: 'build me a web scraper',
      sessionId: 'a',
    });

    expect(requests[0]?.openingTurn).toBe('build me a web scraper');
    expect(requests[1]?.openingTurn).toBeUndefined();
  });

  it('does not rebuild a driver when the opening turn arrives later', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const first = registry.resolve([session('a', 'running')], null);

    const second = registry.resolve([session('a', 'running')], { text: 'hello', sessionId: 'a' });

    // The prompt belongs to the first driver a session is ever given. Treating
    // a later one as grounds to rebuild would reconnect the socket, and
    // replaying it would submit the prompt a second time.
    expect(second[0]?.driver).toBe(first[0]?.driver);
    expect(requests).toHaveLength(1);
  });

  it('hands the opening turn over once, however often the driver is rebuilt', () => {
    const { factory, requests } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const opening = { text: 'build me a web scraper', sessionId: 'a' };
    registry.resolve([session('a', 'running')], opening);

    // Stopped, then revived. Each is a different kind of driver, so each is a
    // genuine rebuild — and neither may carry the prompt. The agent answered it
    // the first time; asking again is a second instruction nobody typed.
    registry.resolve([session('a', 'idle')], opening);
    registry.resolve([session('a', 'running')], opening);

    expect(requests).toHaveLength(3);
    expect(requests[0]?.openingTurn).toBe('build me a web scraper');
    expect(requests[1]?.openingTurn).toBeUndefined();
    expect(requests[2]?.openingTurn).toBeUndefined();
  });

  it('forgets a session that has been pruned', () => {
    const { factory } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const first = registry.resolve([session('a', 'running')], null);

    registry.prune([]);
    const again = registry.resolve([session('a', 'running')], null);

    // A session created again under a reused id must not inherit the driver of
    // the one that was removed.
    expect(again[0]?.driver).not.toBe(first[0]?.driver);
  });

  it('keeps the sessions it was told are still listed', () => {
    const { factory } = recordingFactory();
    const registry = createSessionDriverRegistry(factory);
    const first = registry.resolve([session('a', 'running'), session('b', 'running')], null);

    registry.prune(['a']);
    const again = registry.resolve([session('a', 'running'), session('b', 'running')], null);

    expect(again[0]?.driver).toBe(first[0]?.driver);
    expect(again[1]?.driver).not.toBe(first[1]?.driver);
  });
});

describe('useSessionDrivers', () => {
  it('holds one registry for the life of the component', () => {
    const sessions = [session('a', 'idle')] as readonly Session[];
    const { result, rerender } = renderHook(
      ({ list }: { list: readonly Session[] }) => useSessionDrivers(list, null),
      { initialProps: { list: sessions } },
    );
    const first = result.current.nodes[0]?.driver;

    rerender({ list: sessions });

    // A registry rebuilt on a re-render would drop every live connection it
    // was holding.
    expect(result.current.nodes[0]?.driver).toBe(first);
  });

  it('drops a driver once its session leaves the list', () => {
    const { result, rerender } = renderHook(
      ({ list }: { list: readonly Session[] }) => useSessionDrivers(list, null),
      { initialProps: { list: [session('a', 'idle')] as readonly Session[] } },
    );
    const first = result.current.nodes[0]?.driver;

    rerender({ list: [] });
    rerender({ list: [session('a', 'idle')] });

    expect(result.current.nodes[0]?.driver).not.toBe(first);
  });

  it('reconnects the session it was given and leaves every other driver alone', () => {
    const list = [session('a', 'idle'), session('b', 'idle')] as readonly Session[];
    const { result } = renderHook(() => useSessionDrivers(list, null));
    const before = result.current.nodes.map((node) => node.driver);

    act(() => {
      result.current.reconnect('a');
    });

    /*
     * Reconnect is offered in a panel describing one session, and the canvas
     * may have a dozen more attached. Rebuilding those would clear their
     * scrollbacks and redial their sockets for a button nobody pressed on them.
     */
    expect(result.current.nodes[0]?.driver).not.toBe(before[0]);
    expect(result.current.nodes[1]?.driver).toBe(before[1]);
  });
});
