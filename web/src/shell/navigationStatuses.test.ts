import { expect, it } from 'vitest';
import type { SandboxLifecycle, SandboxStatus } from '../api/types';
import { sandboxFixture } from '../test/hierarchy';
import { sandboxNavigationGroup } from './navigationStatuses';

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
    expect(sandboxNavigationGroup(sandbox)).toBe(statuses[status]);
    expect(sandboxNavigationGroup({ status })).toBe(statuses[status]);
    for (const purpose of ['pause', 'revive', 'image_rotation'] as const) {
      for (const result of ['in_progress', 'recoverable_pending', 'failed'] as const) {
        const lifecycle: SandboxLifecycle = {
          ...sandbox.lifecycle!,
          result: { state: result, continuing: result !== 'failed', actions: ['observe'] },
          operation: { purpose, phase: 'provision', startedAt: sandbox.createdAt },
        };
        expect(sandboxNavigationGroup({ status, lifecycle })).toBe(
          status === 'failed' || status === 'retiring' || status === 'retired'
            ? statuses[status]
            : 'changing',
        );
        expect(sandboxNavigationGroup({ status, lifecycle }, true)).toBe(
          status === 'retired' ? 'hidden' : 'deleting',
        );
      }
    }
    expect(sandboxNavigationGroup(sandbox, true)).toBe(
      status === 'retired' ? 'hidden' : 'deleting',
    );
  });
}
