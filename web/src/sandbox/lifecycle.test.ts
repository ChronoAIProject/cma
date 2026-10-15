import { describe, expect, it } from 'vitest';
import { sandboxFixture } from '../test/hierarchy';
import {
  sandboxChildStatus,
  sandboxChildrenReadable,
  sandboxLifecycleAction,
  sandboxLifecycleLabel,
  sandboxRefreshDelay,
  sandboxUnavailableNotice,
} from './lifecycle';

describe('public Sandbox lifecycle presentation', () => {
  it('keeps existing Agents readable and working while their service key updates', () => {
    const sandbox = sandboxFixture('credential_rotating');
    expect(sandboxChildrenReadable(sandbox)).toBe(true);
    expect(sandboxChildStatus(sandbox, 'working')).toBe('working');
    expect(sandboxLifecycleLabel(sandbox)).toBe('Updating access');
    expect(sandboxUnavailableNotice(sandbox)).toContain(
      'Agents and conversations remain available',
    );
  });
  it.each(['active', 'paused'] as const)(
    'does not infer %s eligibility when unreported',
    (status) => {
      expect(
        sandboxLifecycleAction({ ...sandboxFixture(status), lifecycle: undefined }),
      ).toMatchObject({
        allowed: false,
        reason: 'Action availability is not reported.',
      });
    },
  );

  it.each(['active', 'paused'] as const)('holds stale %s eligibility', (status) => {
    expect(sandboxLifecycleAction(sandboxFixture(status), true)).toMatchObject({
      allowed: false,
      reason: 'Status is unavailable. Reconnecting.',
    });
  });

  it.each(['capture', 'resume_writers'] as const)(
    'keeps Live during %s without changing action eligibility',
    (phase) => {
      const sandbox = sandboxFixture();
      const saving = {
        ...sandbox,
        lifecycle: {
          pause: { allowed: false, reason: 'checkpoint_in_progress' as const },
          revive: { allowed: false, reason: 'not_paused' as const },
          checkpoint: { phase, startedAt: '2026-09-08T00:00:00Z', epoch: 7 },
        },
      };
      expect(sandboxLifecycleLabel(saving)).toBe('Live');
      expect(sandboxRefreshDelay(saving)).toBe(1500);
      expect(sandboxLifecycleAction(saving)).toMatchObject({
        kind: 'pause_sandbox',
        allowed: false,
      });
    },
  );

  it('uses eligibility while preserving the separate Resume command', () => {
    expect(sandboxLifecycleAction(sandboxFixture('paused'))).toMatchObject({
      kind: 'revive_sandbox',
      label: 'Resume',
      allowed: true,
    });
    expect(sandboxLifecycleAction(sandboxFixture())).toMatchObject({
      kind: 'pause_sandbox',
      label: 'Pause',
      allowed: true,
    });
    expect(sandboxRefreshDelay(sandboxFixture())).toBe(5000);
  });

  it('reports a recorded lifecycle operation independently of the status word', () => {
    const sandbox = sandboxFixture('paused');
    const resuming = {
      ...sandbox,
      lifecycle: {
        pause: { allowed: false, reason: 'not_active' as const },
        revive: { allowed: false, reason: 'work_in_progress' as const },
        operation: {
          purpose: 'revive' as const,
          phase: 'provision',
          startedAt: '2026-09-08T00:00:00Z',
        },
      },
    };
    expect(sandboxLifecycleLabel(resuming)).toBe('Resuming');
    expect(sandboxLifecycleAction(resuming)).toMatchObject({
      allowed: false,
      reason: expect.stringContaining('work is in progress'),
    });
    expect(sandboxRefreshDelay(resuming)).toBe(1500);
  });

  it('keeps Pausing while its checkpoint is captured', () => {
    const sandbox = sandboxFixture('pausing');
    expect(
      sandboxLifecycleLabel({
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          checkpoint: { phase: 'capture', startedAt: '2026-09-08T00:00:00Z', epoch: 7 },
          operation: { purpose: 'pause', phase: 'checkpoint', startedAt: '2026-09-08T00:00:00Z' },
        },
      }),
    ).toBe('Pausing');
  });
});
