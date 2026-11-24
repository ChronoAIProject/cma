import { expect, type Page } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

export const discoveryLinks =
  '<https://fixture.invalid/profile>; rel="profile", </openapi.json>; rel="service-desc"; title="API, schema"';

export async function agentRevocationRecovery(
  page: Page,
  baseURL: string,
  entry: 'details' | 'header' | 'success' | 'conflicting',
) {
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
  let conflicting = entry === 'conflicting';
  const expectedWrites = conflicting ? 3 : 2;
  let effective = false;
  let statusUnavailable = false;
  let unresolved = 1;
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
    if (entry === 'success') {
      effective = true;
      return route.fulfill({
        json: {
          ...link,
          revision: 2,
          revoked_at: localTime,
          revocation: {
            operation_id: operation,
            kind: 'agent',
            id: 'agt_contract',
            share_id: 'share',
            expected_revision: 1,
          },
        },
      });
    }
    const statusLink = `</api/v1/resource-revocations/${operation}?kind=agent&id=agt_contract>; rel="status"`;
    return route.fulfill({
      status: 409,
      headers: {
        Link: `${discoveryLinks}, ${statusLink}${conflicting ? `, ${statusLink.replace(operation, 'b'.repeat(64))}` : ''}`,
      },
      json: { code: 'resource_revocation_pending', detail: 'Pending' },
    });
  });
  await page.route('**/api/v1/resource-revocations/**', async (route) => {
    statusReads++;
    expect(route.request().method()).toBe('GET');
    const target = new URL(route.request().url());
    expect(target.pathname).toBe(`/api/v1/resource-revocations/${operation}`);
    expect(target.searchParams.get('kind')).toBe('agent');
    expect(target.searchParams.get('id')).toBe('agt_contract');
    if (statusUnavailable)
      return route.fulfill({ status: 503, json: { code: 'resource_revocation_unavailable' } });
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
        unresolved_effects: unresolved,
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
    if (entry === 'details') {
      await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
      await page.getByRole('button', { name: 'Manage share links', exact: true }).click();
    } else await page.getByRole('button', { name: 'Share', exact: true }).click();
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
  if (conflicting) {
    await expect(page.getByRole('alert')).toContainText('Pending');
    expect(statusReads).toBe(0);
    expect(commands).toHaveLength(2);
    expect(commands[1]).toEqual(commands[0]);
    await page.reload();
    await open();
    await expect(page.getByRole('button', { name: 'Revoke link', exact: true })).toBeEnabled();
    expect(statusReads).toBe(0);
    expect(commands).toHaveLength(2);
    conflicting = false;
    await page.getByRole('button', { name: 'Revoke link', exact: true }).click();
    await page.getByRole('button', { name: 'Revoke access', exact: true }).click();
  }
  if (entry !== 'success')
    await expect(
      page.getByText(
        'Revocation is taking longer than expected. Access removal has not been confirmed.',
      ),
    ).toBeVisible();
  else
    await expect(
      page.getByText('Access revoked. Previously submitted external requests may still complete.'),
    ).toBeVisible();
  expect(commands).toHaveLength(expectedWrites);
  if (entry === 'conflicting') expect(commands[2]).toEqual(commands[0]);
  expect(commands[1]).toEqual(commands[0]);
  expect(commands[0]!.key).toBeTruthy();
  await page.reload();
  await open();
  if (entry !== 'success')
    await expect(page.getByRole('button', { name: 'Revoke link', exact: true })).toBeDisabled();
  expect(commands).toHaveLength(expectedWrites);
  effective = true;
  await expect(
    page.getByText('Access revoked. Previously submitted external requests may still complete.'),
  ).toBeVisible();
  expect(statusReads).toBeGreaterThan(1);
  const previousReads = statusReads;
  statusUnavailable = true;
  await page.reload();
  await open();
  await expect(
    page.getByText('Access revoked. Previously submitted external requests may still complete.'),
  ).toBeVisible();
  await expect(
    page.getByText(/The latest external request status could not be checked/),
  ).toBeVisible();
  expect(statusReads).toBeGreaterThan(previousReads);
  statusUnavailable = false;
  unresolved = 0;
  await page.getByRole('button', { name: 'Check external requests', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Unresolved external requests' })).toHaveCount(0);
  await page.reload();
  await open();
  await expect(page.getByRole('region', { name: 'Unresolved external requests' })).toHaveCount(0);
  expect(commands).toHaveLength(expectedWrites);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
}
