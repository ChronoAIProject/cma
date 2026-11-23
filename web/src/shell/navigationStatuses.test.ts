import { expect, it } from 'vitest';
import type { SandboxLifecycle, SandboxStatus } from '../api/types';
import { sandboxFixture } from '../test/hierarchy';
import { sandboxStatusPresentation } from '../sandbox/presentation';

const statuses = {
  active: 'live',
  paused: 'paused',
  failed: 'needs_attention',
  provisioning: 'changing',
  installing: 'changing',
  pausing: 'changing',
  reviving: 'changing',
  credential_rotating: 'changing',
  rotating: 'changing',
  retiring: 'deleting',
  retired: 'hidden',
} satisfies Record<SandboxStatus, string>;

for (const status of Object.keys(statuses) as SandboxStatus[]) {
  it(`classifies ${status} with lifecycle and deletion precedence`, () => {
    const sandbox = sandboxFixture(status);
    expect(sandboxStatusPresentation(sandbox)).toMatchObject({ status, group: statuses[status] });
    expect(sandboxStatusPresentation({ status }).group).toBe(statuses[status]);
    for (const purpose of ['pause', 'revive', 'image_rotation'] as const) {
      for (const result of ['in_progress', 'recoverable_pending', 'failed'] as const) {
        const lifecycle: SandboxLifecycle = {
          ...sandbox.lifecycle!,
          result: { state: result, continuing: result !== 'failed', actions: ['observe'] },
          operation: { purpose, phase: 'provision', startedAt: sandbox.createdAt },
        };
        expect(sandboxStatusPresentation({ status, lifecycle }).group).toBe(
          status === 'failed' || status === 'retiring' || status === 'retired'
            ? statuses[status]
            : 'changing',
        );
        expect(sandboxStatusPresentation({ status, lifecycle }, true).group).toBe(
          status === 'retired' ? 'hidden' : 'deleting',
        );
      }
    }
    expect(sandboxStatusPresentation(sandbox, true).group).toBe(
      status === 'retired' ? 'hidden' : 'deleting',
    );
  });
}
