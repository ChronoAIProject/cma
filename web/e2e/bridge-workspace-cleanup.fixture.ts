import type { Page } from '@playwright/test';
import type {
  BridgeChildCleanupStatus,
  BridgeWorkspaceCleanupStatus,
  BridgeConfigurationDetail,
} from '../../sdk/typescript/src/index';
import { recorded } from './agent-startup-fixtures';
import { cleanupBoundary } from './bridge-cleanup.fixture';

export async function workspaceCleanupBoundary(page: Page, baseURL: string) {
  const parent = await cleanupBoundary(page, baseURL);
  const before = recorded<BridgeWorkspaceCleanupStatus>(
    'bridge-workspace-cleanup.json',
    'bridge-workspace-cleanup-unrequested',
  );
  const accepted = recorded<BridgeWorkspaceCleanupStatus>(
    'bridge-workspace-cleanup.json',
    'bridge-workspace-cleanup-explicit-acceptance',
  );
  const originalChild = recorded<BridgeChildCleanupStatus>(
    'bridge-child-cleanup.json',
    'bridge-cleanup-explicit-acceptance',
  );
  const child: BridgeChildCleanupStatus = {
    ...originalChild,
    target: before.target,
    receipt: {
      ...originalChild.receipt!,
      id: before.child_cleanup_receipt_id!,
      target: before.target,
    },
    progress: 'complete',
    holds: [],
    agent_capacity_released: true,
  };
  const row = { ...parent.row, target: before.target };
  const detail = recorded<BridgeConfigurationDetail>(
    'bridge-management.json',
    'bridge-setup-configuration-success',
  );
  const state = {
    accepted: false,
    runtimeSupported: true,
    workspaceRevision: before.workspace_revision,
    bridgeRevision: accepted.receipt!.request.expected_bridge_revision,
    complete: false,
    writes: [] as { body: unknown; bytes: string | null; key: string | undefined }[],
    childWrites: 0,
    cursors: [] as (string | null)[],
  };
  await page.route(`**/api/v1/bridges/${row.bridge_id}`, (route) =>
    route.fulfill({
      json: {
        ...detail.bridge,
        id: row.bridge_id,
        revision: state.bridgeRevision,
        can_manage: parent.state.manager,
      },
    }),
  );
  await page.route('**/api/v1/bridges/*/allocations**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname.endsWith('/allocations')) {
      state.cursors.push(url.searchParams.get('cursor'));
      return route.fulfill({
        json: url.searchParams.has('cursor')
          ? { items: [], next_cursor: null }
          : { items: [row], next_cursor: 'workspace/next?opaque=1' },
      });
    }
    if (url.pathname.endsWith('/child-cleanup')) {
      if (request.method() !== 'GET') {
        state.childWrites++;
        return route.fulfill({ status: 409, json: { code: 'unexpected_child_consent' } });
      }
      return route.fulfill({ json: child });
    }
    if (url.pathname.endsWith('/workspace-cleanup')) {
      if (request.method() === 'POST') {
        state.writes.push({
          body: request.postDataJSON(),
          bytes: request.postData(),
          key: request.headers()['idempotency-key'],
        });
        state.accepted = true;
        if (state.writes.length === 1) return route.abort('connectionreset');
      }
      const value = {
        ...(state.accepted ? accepted : before),
        runtime_supported: state.runtimeSupported,
        workspace_revision: state.workspaceRevision,
        ...(state.complete
          ? { progress: 'complete', holds: [], workspace_capacity_released: true }
          : {}),
      };
      return route.fulfill({ status: request.method() === 'POST' ? 202 : 200, json: value });
    }
    parent.boundary.unexpected.push(`${request.method()} ${url.pathname}`);
    return route.fulfill({ status: 500, json: { code: 'unexpected_fixture_request' } });
  });
  return { ...parent, row, workspace: state };
}
