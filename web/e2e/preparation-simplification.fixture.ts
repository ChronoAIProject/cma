import type { Page } from '@playwright/test';
import { redesignHomeFixture } from './redesign-home.fixture';

export const acceptedAt = '2026-11-22T05:58:00Z';
export const progressAt = '2026-11-22T05:59:00Z';
export async function preparationFixture(page: Page, baseURL: string) {
  const local = await redesignHomeFixture(page, baseURL, 'empty');
  let bound = false;
  let failed = false;
  let readError = false;
  let stalled = false;
  let completed = false;
  const writes: { body: string; key?: string }[] = [];
  local.boundary.agent = {
    ...local.boundary.agent,
    title: undefined,
    status: 'starting',
    first_message: { state: 'queued', purpose: 'task', queued_at: acceptedAt },
  };
  await page.route('**/api/agent-manager/v1/agents', (route) => {
    writes.push({
      body: route.request().postData()!,
      key: route.request().headers()['idempotency-key'],
    });
    return route.fulfill({ status: 202, json: { id: 'preparation-operation', status: 'queued' } });
  });
  await page.route('**/api/agent-manager/v1/agent-lifecycle/preparation-operation', (route) =>
    readError
      ? route.fulfill({ status: 503, json: { code: 'unavailable' } })
      : route.fulfill({
          json: {
            id: 'preparation-operation',
            status: completed ? 'succeeded' : failed ? 'failed' : 'running',
            phase: stalled ? 'recoverable_pending' : 'sandbox_runtime',
            accepted_at: acceptedAt,
            ...(bound ? { agent_id: local.boundary.agent.id } : {}),
            ...(failed ? { error: 'Synthetic current setup failure' } : {}),
            events: [
              {
                sequence: 1,
                phase: 'sandbox_runtime',
                status: 'running',
                message: 'Recorded progress',
                occurred_at: progressAt,
              },
            ],
          },
        }),
  );
  return {
    ...local,
    creates: writes,
    bind: () => {
      bound = true;
    },
    fail: () => {
      failed = true;
    },
    stall: () => {
      stalled = true;
    },
    readError: (value: boolean) => {
      readError = value;
    },
    complete: () => {
      completed = true;
      local.boundary.agent = {
        ...local.boundary.agent,
        status: 'ready',
        first_message: {
          state: 'execution_observed',
          purpose: 'task',
          queued_at: acceptedAt,
          turn_sequence: 1,
        },
      };
    },
  };
}
