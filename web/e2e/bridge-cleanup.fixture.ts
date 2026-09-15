import type { Page } from '@playwright/test';
import type {
  BridgeChildCleanupStatus,
  BridgeCleanupAllocationPage,
  BridgeConfigurationDetail,
} from '../../sdk/typescript/src/index';
import { LocalStartup, recorded } from './agent-startup-fixtures';

export async function cleanupBoundary(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const inventory = recorded<BridgeCleanupAllocationPage>(
    'bridge-child-cleanup.json',
    'bridge-cleanup-list',
  );
  const row = inventory.items[0]!;
  const before = recorded<BridgeChildCleanupStatus>(
    'bridge-child-cleanup.json',
    'bridge-cleanup-unrequested',
  );
  const accepted = recorded<BridgeChildCleanupStatus>(
    'bridge-child-cleanup.json',
    'bridge-cleanup-explicit-acceptance',
  );
  const originalDetail = recorded<BridgeConfigurationDetail>(
    'bridge-management.json',
    'bridge-setup-configuration-success',
  );
  const detail = {
    ...originalDetail,
    bridge: {
      ...originalDetail.bridge,
      id: row.bridge_id,
      revision: accepted.receipt!.request.expected_bridge_revision,
    },
  };
  const state = {
    manager: true,
    accepted: false,
    writes: [] as { body: unknown; key: string | undefined }[],
    cursors: [] as (string | null)[],
  };
  await page.route('**/api/v1/bridge-connectors**', (route) =>
    route.fulfill({ json: { bots: [], keys: [], routes: [] } }),
  );
  await page.route('**/api/v1/sandbox-profiles**', (route) =>
    route.fulfill({ json: { items: [] } }),
  );
  await page.route('**/api/v1/bridges**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname.endsWith('/allocations')) {
      state.cursors.push(url.searchParams.get('cursor'));
      return route.fulfill({
        json: url.searchParams.has('cursor')
          ? inventory
          : { items: [], next_cursor: 'opaque/+/v1?part=1' },
      });
    }
    if (url.pathname.endsWith('/child-cleanup')) {
      if (request.method() === 'GET')
        return route.fulfill({ json: state.accepted ? accepted : before });
      state.writes.push({
        body: request.postDataJSON(),
        key: request.headers()['idempotency-key'],
      });
      state.accepted = true;
      if (state.writes.length === 1) return route.abort('connectionreset');
      return route.fulfill({ status: 202, json: accepted });
    }
    if (url.pathname.endsWith('/configuration')) return route.fulfill({ json: detail });
    if (url.pathname.endsWith('/invoke-readiness'))
      return route.fulfill({
        json: {
          bridge_id: row.bridge_id,
          revision: detail.bridge.revision,
          config_revision: detail.bridge.config_revision,
          status: 'not_applicable',
          candidate: null,
        },
      });
    if (url.pathname.endsWith('/invoke-grants') || url.pathname.endsWith('/deliveries'))
      return route.fulfill({ json: { items: [], next_cursor: null } });
    if (url.pathname === `/api/v1/bridges/${row.bridge_id}`)
      return route.fulfill({ json: { ...detail.bridge, can_manage: state.manager } });
    boundary.unexpected.push(`${request.method()} ${url.pathname}`);
    return route.fulfill({ status: 500, json: { code: 'unexpected_fixture_request' } });
  });
  return { boundary, row, state };
}
