import { describe, expect, it } from 'vitest';
import type { Capability } from '../auth/types';
import {
  canControlAnySession,
  canOperate,
  controlsFor,
  environmentResetFor,
  retirementFor,
  viewsFor,
} from './access';
import type { AdminSession, ChatHost } from './types';

/**
 * Who is offered what.
 *
 * These are the rules the menu item, the view switch and the controls all read,
 * so what is pinned here is that they cannot disagree — and that a caller who
 * has been granted nothing is offered nothing, which is the case somebody will
 * eventually try by hand.
 */

const DAILY: readonly Capability[] = [
  'create_session',
  'use_own_sessions',
  'terminate_own_session',
  'manage_own_environment_profiles',
];

function session(status: string): AdminSession {
  return { id: 'sesn_1', owner: 'subject-a', status, kernel: 'example-runtime', chat: false };
}

describe('who is offered the panel', () => {
  it('offers nothing to somebody who may only use their own sessions', () => {
    expect(canOperate(DAILY)).toBe(false);
    expect(viewsFor(DAILY)).toEqual([]);
  });

  it('offers nothing at all to a caller granted nothing', () => {
    expect(canOperate([])).toBe(false);
  });

  it('offers each view on its own capability', () => {
    // Matching the API, where the four reads are four separate grants and none
    // implies another.
    expect(viewsFor(['read_user_directory']).map((v) => v.id)).toEqual(['people']);
    expect(viewsFor(['read_activity_log']).map((v) => v.id)).toEqual(['activity']);
    expect(viewsFor(['read_runtime_inventory']).map((v) => v.id)).toEqual(['namespace']);
    expect(viewsFor(['read_all_sessions']).map((v) => v.id)).toEqual(['sessions', 'chat_hosts']);
    expect(viewsFor(['configure_platform']).map((v) => v.id)).toEqual([
      'settings',
      'chat_settings',
    ]);
  });

  it('offers both configuration views from the one server capability', () => {
    expect(canOperate(['configure_platform'])).toBe(true);
    expect(viewsFor(['configure_platform']).map((view) => view.label)).toEqual([
      'Settings',
      'Chat',
    ]);
  });

  it('offers the views in a fixed order however the capabilities arrived', () => {
    const ordered = viewsFor(['read_all_sessions', 'read_user_directory']).map((v) => v.id);
    expect(ordered).toEqual(['people', 'sessions', 'chat_hosts']);
  });

  it('does not offer the panel for the power to act alone', () => {
    // There would be nothing in it: every view is a read, and somebody who can
    // stop a session but not list one has nothing to point at.
    expect(canOperate(['control_any_session'])).toBe(false);
    expect(canControlAnySession(['control_any_session'])).toBe(true);
  });

  it('separates watching from acting', () => {
    // The property the API is built around: a role handed out so somebody can
    // watch the service must not also let them stop it.
    const watcher: readonly Capability[] = [
      'read_all_sessions',
      'read_user_directory',
      'read_activity_log',
      'read_runtime_inventory',
    ];
    expect(canOperate(watcher)).toBe(true);
    expect(canControlAnySession(watcher)).toBe(false);
  });
});

describe('what may be done to a chat host', () => {
  function host(status: ChatHost['status']): ChatHost {
    return {
      id: 'chho_fixture',
      owner: 'subject-a',
      status,
      threadCount: 2,
      settingsRevision: 7,
      createdAt: 0,
    };
  }

  it('separates host inventory from retirement authority', () => {
    expect(retirementFor(host('active'), ['read_all_sessions'])).toMatchObject({
      canRetire: false,
      reason: expect.stringMatching(/watch/i),
    });
    expect(retirementFor(host('active'), ['control_any_session'])).toEqual({ canRetire: true });
    expect(retirementFor(host('paused'), ['control_any_session'])).toEqual({ canRetire: true });
  });

  it('does not offer retirement in transitional or retired states', () => {
    expect(retirementFor(host('reviving'), ['control_any_session']).canRetire).toBe(false);
    expect(retirementFor(host('retired'), ['control_any_session']).reason).toMatch(/already/i);
  });

  it('keeps permanent reset gated but available for a retired host', () => {
    expect(environmentResetFor(['read_all_sessions'])).toMatchObject({
      canReset: false,
      reason: expect.stringMatching(/watch/i),
    });
    expect(environmentResetFor(['control_any_session'])).toEqual({ canReset: true });
  });
});

describe('what may be done to a session', () => {
  it('offers nothing to a caller who may watch but not act', () => {
    const controls = controlsFor(session('running'), ['read_all_sessions']);
    expect(controls.canStop).toBe(false);
    expect(controls.canTerminate).toBe(false);
    // Disabled controls state their reason, per Web_Style.md §14.
    expect(controls.stopReason).toMatch(/watch/i);
    expect(controls.terminateReason).toMatch(/watch/i);
  });

  it('offers both on a running session', () => {
    const controls = controlsFor(session('running'), ['control_any_session']);
    expect(controls.canStop).toBe(true);
    expect(controls.canTerminate).toBe(true);
    expect(controls.stopReason).toBeUndefined();
  });

  it('offers permanent deletion only for Chat rows', () => {
    expect(controlsFor(session('running'), ['control_any_session']).canDeleteChat).toBe(false);
    expect(
      controlsFor({ ...session('terminated'), chat: true }, ['control_any_session']).canDeleteChat,
    ).toBe(true);
    expect(controlsFor({ ...session('running'), chat: true }, ['read_all_sessions'])).toMatchObject(
      {
        canDeleteChat: false,
        deleteChatReason: expect.stringMatching(/watch/i),
      },
    );
  });

  it('will not stop a session whose runtime is already released', () => {
    const controls = controlsFor(session('idle'), ['control_any_session']);
    expect(controls.canStop).toBe(false);
    expect(controls.stopReason).toMatch(/already released/i);
    // Ending it still means something: it stops the owner reviving it.
    expect(controls.canTerminate).toBe(true);
  });

  it('offers nothing on a session that has already ended', () => {
    const controls = controlsFor(session('terminated'), ['control_any_session']);
    expect(controls.canStop).toBe(false);
    expect(controls.canTerminate).toBe(false);
    expect(controls.terminateReason).toMatch(/already ended/i);
  });

  it('still acts on a state this console has never heard of', () => {
    // Deliberately the opposite of the owner's own panel. An operator reaching
    // for this is usually reaching for something misbehaving, and refusing
    // locally would leave them unable to act on a state a newer control plane
    // introduced. The control plane refuses if it disagrees.
    const controls = controlsFor(session('hibernating'), ['control_any_session']);
    expect(controls.canStop).toBe(true);
    expect(controls.canTerminate).toBe(true);
  });
});
