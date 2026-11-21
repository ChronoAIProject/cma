import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

// Synthetic HTTP boundaries exercise the real browser; they do not qualify a runtime.
test('LOCAL revocation preserves the original request through lost reply and reload', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const operation = 'a'.repeat(64);
  const link = {
    id: 'share',
    agent_id: 'agt_contract',
    audience: { kind: 'public' },
    mode: 'view',
    shared_by: { subject: 'local-owner', display_name: 'Owner', avatar_url: null },
    created_at: localTime,
    revoked_at: null,
    revision: 1,
    accepted_count: 1,
  };
  const commands: { key?: string; revision: string | null }[] = [];
  let lost = true;
  let effective = false;
  let statusReads = 0;
  await page.route('**/api/v1/organizations', (route) => route.fulfill({ json: { items: [] } }));
  await page.route('**/api/v1/agents/agt_contract/share-links**', async (route) => {
    const request = route.request();
    if (request.method() === 'GET') return route.fulfill({ json: { items: [link] } });
    expect(request.method()).toBe('DELETE');
    commands.push({
      key: request.headers()['idempotency-key'],
      revision: new URL(request.url()).searchParams.get('expected_revision'),
    });
    if (lost) return route.abort('failed');
    return route.fulfill({
      status: 409,
      headers: {
        Link: `</api/v1/resource-revocations/${operation}?kind=agent&id=agt_contract>; rel="status"`,
      },
      json: { code: 'resource_revocation_pending', detail: 'Pending' },
    });
  });
  await page.route('**/api/v1/resource-revocations/**', async (route) => {
    statusReads++;
    expect(route.request().method()).toBe('GET');
    const actor = { kind: 'human', subject: 'local-owner' };
    const selector = { kind: 'agent', agent_id: 'agt_contract' };
    return route.fulfill({
      json: {
        operation_id: operation,
        selector,
        share_id: 'share',
        expected_revision: 1,
        state: effective ? 'effective' : 'pending',
        participants: 1,
        stopped: effective ? 1 : 0,
        unresolved_effects: 1,
        deadline_unix_ms: 11000,
        deadline_exceeded: !effective,
        receipt: effective
          ? {
              operation_id: operation,
              share: {
                id: 'share',
                owner: { kind: 'person', subject: 'local-owner' },
                selector,
                target_org_id: 'org',
                mode: 'view',
                issuer_actor: actor,
                created_at: localTime,
                revoked_at: localTime,
                revision: 2,
              },
              actor,
              at: localTime,
              policy_revision: 2,
            }
          : null,
      },
    });
  });
  const open = async () => {
    await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
    await page.getByRole('button', { name: 'Manage share links', exact: true }).click();
  };
  await page.goto('/agents/agt_contract');
  await open();
  await page.getByRole('button', { name: 'Revoke link', exact: true }).click();
  await page.getByRole('button', { name: 'Revoke access', exact: true }).click();
  await expect(page.getByRole('alert')).toBeVisible();
  expect(commands).toHaveLength(1);
  await page.reload();
  await open();
  expect(commands).toHaveLength(1);
  lost = false;
  await page.getByRole('button', { name: 'Revoke link', exact: true }).click();
  await page.getByRole('button', { name: 'Revoke access', exact: true }).click();
  await expect(
    page.getByText(
      'Revocation is taking longer than expected. Access removal has not been confirmed.',
    ),
  ).toBeVisible();
  expect(commands).toHaveLength(2);
  expect(commands[1]).toEqual(commands[0]);
  expect(commands[0]!.key).toBeTruthy();
  await page.reload();
  await open();
  await expect(page.getByRole('button', { name: 'Revoke link', exact: true })).toBeDisabled();
  expect(commands).toHaveLength(2);
  effective = true;
  await expect(
    page.getByText('Access revoked. Previously submitted external requests may still complete.'),
  ).toBeVisible();
  expect(statusReads).toBeGreaterThan(1);
  expect(commands).toHaveLength(2);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
