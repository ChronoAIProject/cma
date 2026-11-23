import { describe, expect, it, vi } from 'vitest';
import type { SandboxSummary } from '../api/types';
import { navigationFixture } from '../test/hierarchy';
import type { TreePermissions } from '../shell/Tree';
import { agentHeaderActionHandler, agentHeaderMenu } from './headerActions';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

function parked(): SandboxSummary {
  const sandbox = navigationFixture('rotating')[0]!.sandbox;
  return {
    ...sandbox,
    lifecycle: {
      ...sandbox.lifecycle!,
      operation: {
        purpose: 'image_rotation',
        phase: 'conversation_reopen',
        startedAt: '2026-09-06T00:10:00Z',
      },
    },
    runtimeUpdate: {
      currentVersion: 'runtime-before',
      latestVersion: 'runtime-after',
      updateAvailable: true,
      state: 'failed',
      canUpdate: true,
      schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
      preservesHome: true,
    },
  };
}

describe('Agent header actions after a failed runtime update', () => {
  it('lists Delete environment with the destructive rows and routes it to the Sandbox', () => {
    const sandbox = parked();
    const menu = agentHeaderMenu('stopped', permissions, { sandbox, viewerId: sandbox.owner });
    const ids = menu.map((entry) => entry.id);
    expect(ids.indexOf('before-destructive')).toBeLessThan(ids.indexOf('delete_sandbox'));
    const environment = menu.find((entry) => entry.id === 'delete_sandbox');
    expect(environment).toMatchObject({ label: 'Delete environment' });
    expect(environment && 'disabled' in environment && environment.disabled).toBeFalsy();
    const onAction = vi.fn();
    agentHeaderActionHandler('agt_1', false, onAction, vi.fn(), sandbox.id)!('delete_sandbox');
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      kind: 'delete_sandbox',
      sandboxId: sandbox.id,
    });
  });

  it('keeps Agent actions bound to the Agent', () => {
    const onAction = vi.fn();
    agentHeaderActionHandler('agt_1', false, onAction, vi.fn(), 'sbx_1')!('delete_agent');
    expect(onAction).toHaveBeenCalledExactlyOnceWith({ kind: 'delete_agent', agentId: 'agt_1' });
  });
});
