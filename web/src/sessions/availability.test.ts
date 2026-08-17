import { describe, expect, it } from 'vitest';
import { abilitiesFor, listable } from './availability';
import type { Session, SessionLifecycle } from './types';
import { NO_CAPABILITIES } from '../agui';

/**
 * The promise a session owner is given, as assertions.
 *
 * A session you did not stop stays usable; one you stopped stays readable and
 * can be brought back; one that ended leaves the list. Each of those is a
 * sentence someone said about how this should behave, and each is one that a
 * plausible refactor could quietly break.
 */

function session(lifecycle: SessionLifecycle, stopReason?: 'error' | 'other'): Session {
  return {
    id: `sesn_${lifecycle}`,
    kernel: 'codex',
    lifecycle,
    createdAt: 0,
    stopReason,
    capabilities: NO_CAPABILITIES,
  };
}

const ALL: readonly SessionLifecycle[] = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
];

describe('what a session allows', () => {
  it('lets a running session be used', () => {
    expect(abilitiesFor(session('running')).interactive).toBe(true);
    expect(abilitiesFor(session('ready')).interactive).toBe(true);
  });

  it('does not let a stopped session be typed into, but offers to revive it', () => {
    // The complaint this answers: a stopped session that looks live, takes
    // keystrokes and does nothing with them.
    const can = abilitiesFor(session('idle'));
    expect(can.interactive).toBe(false);
    expect(can.canRevive).toBe(true);
    expect(can.reason).toMatch(/revive/i);
  });

  it('keeps a stopped session in the list', () => {
    // Stopping is not deleting. Someone who stopped a session to save money
    // still has the session.
    expect(abilitiesFor(session('idle')).listed).toBe(true);
  });

  it('offers revive only where there is something to revive', () => {
    for (const lifecycle of ALL) {
      if (lifecycle === 'idle') {
        continue;
      }
      expect(abilitiesFor(session(lifecycle)).canRevive).toBe(false);
    }
  });

  it('refuses everything but reading while a session is being moved', () => {
    // A rotation is brief and must not be interfered with: input would go to a
    // sandbox being torn down, and stopping mid-handover would leave the
    // session with neither runtime.
    const can = abilitiesFor(session('rotating'));
    expect(can.interactive).toBe(false);
    expect(can.canStop).toBe(false);
    expect(can.canDelete).toBe(false);
    expect(can.canRevive).toBe(false);
    expect(can.reason).toMatch(/updated runtime/i);
  });

  it('lets a session that is still starting be abandoned', () => {
    // Otherwise a session that takes minutes to start is one nobody can call
    // off, however obviously unwanted it has become.
    expect(abilitiesFor(session('provisioning')).canStop).toBe(true);
    expect(abilitiesFor(session('provisioning')).interactive).toBe(false);
  });

  it('never leaves a session unusable without saying why', () => {
    for (const lifecycle of ALL) {
      const can = abilitiesFor(session(lifecycle));
      if (!can.interactive) {
        expect(can.reason, `${lifecycle} must explain itself`).toBeTruthy();
      }
    }
  });

  it('does not offer reconnecting where it would fail', () => {
    // Every state that cannot be typed into also cannot be reattached to, so
    // the two can never disagree in front of someone.
    for (const lifecycle of ALL) {
      const can = abilitiesFor(session(lifecycle));
      if (!can.interactive) {
        expect(can.canReconnect, `${lifecycle} must not offer reconnect`).toBe(false);
      }
    }
  });
});

describe('which sessions are listed', () => {
  it('drops sessions the owner ended', () => {
    // Asked for directly: an ended session must not stay in the left list.
    expect(abilitiesFor(session('terminated', 'other')).listed).toBe(false);
  });

  it('keeps a session that ended by failing', () => {
    // The opposite of hiding a problem. A session that died on its own is
    // exactly the one its owner needs to find.
    const failed = abilitiesFor(session('terminated', 'error'));
    expect(failed.listed).toBe(true);
    expect(failed.reason).toMatch(/failed/i);
  });

  it('lists everything that is not ended', () => {
    const live = ALL.filter((l) => l !== 'terminated').map((l) => session(l));
    expect(listable(live)).toHaveLength(live.length);
  });

  it('filters a mixed list without reordering it', () => {
    const sessions = [
      session('running'),
      session('terminated', 'other'),
      session('idle'),
      session('provisioning'),
    ];
    expect(listable(sessions).map((s) => s.lifecycle)).toEqual(['running', 'idle', 'provisioning']);
  });

  it('handles an empty list', () => {
    expect(listable([])).toEqual([]);
  });
});
