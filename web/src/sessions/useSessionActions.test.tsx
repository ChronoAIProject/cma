import { act, renderHook } from '@testing-library/react';
import { useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { NO_CAPABILITIES } from '../agui';
import {
  changePlane,
  createSession,
  deleteSession,
  DeploymentAtCapacity,
  reviveSession,
  SessionLimitReached,
  stopSession,
} from '../api/client';
import type { Session, SessionLifecycle } from './types';
import { useSessionActions, type FailureNotice } from './useSessionActions';

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>();
  return {
    ...actual,
    changePlane: vi.fn(),
    createSession: vi.fn(),
    deleteSession: vi.fn(),
    reviveSession: vi.fn(),
    stopSession: vi.fn(),
  };
});

const PAUSED: Session = {
  id: 'sesn_paused',
  kernel: 'codex',
  lifecycle: 'idle',
  createdAt: 0,
  capabilities: NO_CAPABILITIES,
};

const STARTING: Session = {
  ...PAUSED,
  id: 'sesn_starting',
  lifecycle: 'provisioning',
};

function renderActions() {
  const refreshAllowance = vi.fn();
  const report = vi.fn();
  const hook = renderHook(() => {
    const [sessions, setSessions] = useState<readonly Session[]>([PAUSED]);
    const [activeId, setActiveId] = useState<string | null>(PAUSED.id);
    const [openingForId, setOpeningForId] = useState<string | null>(null);
    const [notice, setNotice] = useState<FailureNotice | null>(null);
    const [acting, setActing] = useState(false);
    const [focusHistory, setFocusHistory] = useState<readonly string[]>([PAUSED.id]);
    const actions = useSessionActions({
      sessions,
      activeId,
      setSessions,
      setActiveId,
      setOpeningForId,
      setNotice,
      setActing,
      select: setActiveId,
      report,
      applyLifecycle: (id: string, lifecycle: SessionLifecycle) => {
        setSessions((previous) =>
          previous.map((session) => (session.id === id ? { ...session, lifecycle } : session)),
        );
      },
      setFocusHistory,
      refreshAllowance,
    });
    return { actions, sessions, activeId, openingForId, notice, acting, focusHistory };
  });
  return { ...hook, refreshAllowance, report };
}

beforeEach(() => {
  vi.mocked(createSession).mockReset().mockResolvedValue(STARTING);
  vi.mocked(stopSession).mockReset().mockResolvedValue();
  vi.mocked(reviveSession).mockReset().mockResolvedValue(STARTING);
  vi.mocked(deleteSession).mockReset().mockResolvedValue();
  vi.mocked(changePlane)
    .mockReset()
    .mockResolvedValue({ ...PAUSED, lifecycle: 'rotating' });
});

describe('refreshing the live-session allowance', () => {
  it('refreshes after create, stop, revive and delete have each settled', async () => {
    const { result, refreshAllowance } = renderActions();
    await act(() => result.current.actions.start());
    await act(() => result.current.actions.endSession(PAUSED.id));
    await act(() => result.current.actions.reviveSessionById(PAUSED.id));
    await act(() => result.current.actions.removeSession(PAUSED.id));
    expect(refreshAllowance).toHaveBeenCalledTimes(4);
  });

  it('does not refresh until a creation attempt settles', async () => {
    let resolve!: (session: Session) => void;
    vi.mocked(createSession).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const { result, refreshAllowance } = renderActions();
    const attempt = result.current.actions.start();
    expect(refreshAllowance).not.toHaveBeenCalled();
    resolve(STARTING);
    await act(() => attempt);
    expect(refreshAllowance).toHaveBeenCalledOnce();
  });

  it('refreshes after a failed action too', async () => {
    vi.mocked(createSession).mockRejectedValueOnce(new Error('refused'));
    const { result, refreshAllowance } = renderActions();
    await act(async () => {
      await expect(result.current.actions.start()).rejects.toThrow('refused');
    });
    expect(refreshAllowance).toHaveBeenCalledOnce();
  });

  it('does not refresh after a plane change, which replaces rather than adds a runtime', async () => {
    const { result, refreshAllowance } = renderActions();
    await act(() => result.current.actions.changeSessionPlane(PAUSED.id, 'conversation'));
    expect(refreshAllowance).not.toHaveBeenCalled();
  });
});

describe('creation failures handled by the open dialog', () => {
  it('does not leave a duplicate global notice for deployment capacity', async () => {
    const refusal = new DeploymentAtCapacity(
      503,
      'this deployment has no room',
      2_000,
      4_096,
      750,
      1_536,
    );
    vi.mocked(createSession).mockRejectedValueOnce(refusal);
    const { result, refreshAllowance, report } = renderActions();

    await act(async () => {
      await expect(result.current.actions.start()).rejects.toBe(refusal);
    });

    expect(report).not.toHaveBeenCalled();
    expect(refreshAllowance).toHaveBeenCalledOnce();
  });
});

describe('a revive refused at the live-session limit', () => {
  it('keeps the service sentence and code while leaving the session stopped', async () => {
    const detail =
      '4 sessions are already running for this owner and the limit is 3; nothing new starts until fewer than 3 are running';
    vi.mocked(reviveSession).mockRejectedValueOnce(new SessionLimitReached(409, detail, 3, 4));
    const { result, refreshAllowance, report } = renderActions();
    await act(() => result.current.actions.reviveSessionById(PAUSED.id));

    expect(result.current.sessions).toEqual([PAUSED]);
    expect(result.current.notice).toEqual({
      failed: detail,
      unchanged: 'The session stays stopped.',
      code: 'session_limit_reached',
    });
    expect(report).not.toHaveBeenCalled();
    expect(refreshAllowance).toHaveBeenCalledOnce();
  });
});
