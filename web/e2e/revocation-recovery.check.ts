import { test, expect, type Page } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL Sandbox revision refusal allows an explicit fresh revoke after reload', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const sandboxId = boundary.sandbox.id;
  const actor = { kind: 'human', subject: 'local-owner' };
  const original = {
    id: 'sandbox-share',
    owner: { kind: 'person', subject: 'local-owner' },
    selector: { kind: 'sandbox', sandbox_id: sandboxId },
    target_org_id: 'research',
    mode: 'org_roles',
    issuer_actor: actor,
    created_at: localTime,
    revoked_at: null,
    revision: 1,
  };
  let revision = 1;
  let revoked = false;
  let grantReads = 0;
  const writes: { method: string; key?: string; revision: number }[] = [];
  await page.route('**/api/v1/organizations', (route) =>
    route.fulfill({
      json: { items: [{ id: 'research', display_name: 'Research team', role: 'admin' }] },
    }),
  );
  await page.route('**/api/v1/resource-access/**', (route) =>
    route.fulfill({
      json: {
        capabilities: ['view_details', 'manage_sharing'],
        revalidate_at: null,
        policy_revision: revision,
      },
    }),
  );
  await page.route('**/api/v1/resource-shares/**', (route) => {
    const request = route.request();
    if (request.method() === 'GET') {
      grantReads++;
      return route.fulfill({ json: { items: revoked ? [] : [{ ...original, revision }] } });
    }
    writes.push({
      method: request.method(),
      key: request.headers()['idempotency-key'],
      revision:
        request.method() === 'PATCH'
          ? (request.postDataJSON() as { expected_revision: number }).expected_revision
          : Number(new URL(request.url()).searchParams.get('expected_revision')),
    });
    if (request.method() === 'PATCH') {
      revision = 2;
      return route.fulfill({
        status: 409,
        json: { code: 'resource_access_changed', detail: 'Read the current revision.' },
      });
    }
    expect(request.method()).toBe('DELETE');
    expect(writes.at(-1)!.revision).toBe(2);
    revision = 3;
    revoked = true;
    return route.fulfill({
      json: {
        operation_id: 'c'.repeat(64),
        actor,
        at: localTime,
        policy_revision: revision,
        share: { ...original, revision, revoked_at: localTime },
      },
    });
  });
  const open = async () => {
    await page.getByRole('button', { name: 'More actions', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Sharing', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Current grants' })).toBeVisible();
  };
  await page.goto(`/sandboxes/${sandboxId}`);
  await open();
  const previousReads = grantReads;
  await page.getByRole('button', { name: 'Access for Research team', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'View only', exact: true }).click();
  await page.getByRole('button', { name: 'Save access', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('share change was rejected');
  await expect(page.getByRole('button', { name: 'Revoke', exact: true })).toBeEnabled();
  expect(grantReads).toBeGreaterThan(previousReads);
  expect(writes).toHaveLength(1);
  await page.reload();
  await open();
  await expect(page.getByRole('button', { name: 'Revoke', exact: true })).toBeEnabled();
  await expect(page.getByLabel('Saved share change')).toHaveCount(0);
  expect(writes).toHaveLength(1);
  await page.getByRole('button', { name: 'Revoke', exact: true }).click();
  await page.getByRole('button', { name: 'Revoke grant', exact: true }).click();
  await expect(page.getByText('Access revoked.', { exact: true })).toBeVisible();
  expect(writes).toHaveLength(2);
  expect(writes[0]!.revision).toBe(1);
  expect(writes[1]!.key).toBeTruthy();
  expect(writes[1]!.key).not.toBe(writes[0]!.key);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});

// Synthetic HTTP boundaries exercise the real browser; they do not qualify a runtime.
for (const entry of ['details', 'header', 'success'] as const) {
  test(`LOCAL ${entry} revocation preserves the original request through lost reply and reload`, async ({
    page,
    baseURL,
  }) => {
    await agentRevocationRecovery(page, baseURL!, entry);
  });
}

async function agentRevocationRecovery(
  page: Page,
  baseURL: string,
  entry: 'details' | 'header' | 'success',
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
  expect(commands).toHaveLength(2);
  expect(commands[1]).toEqual(commands[0]);
  expect(commands[0]!.key).toBeTruthy();
  await page.reload();
  await open();
  if (entry !== 'success')
    await expect(page.getByRole('button', { name: 'Revoke link', exact: true })).toBeDisabled();
  expect(commands).toHaveLength(2);
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
  expect(commands).toHaveLength(2);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
}

for (const success of [false, true]) {
  for (const change of ['revoke', 'set_mode'] as const) {
    test(`LOCAL Sandbox ${change} ${success ? 'success' : 'pending'} retains the original command and resumes status after reload`, async ({
      page,
      baseURL,
    }) => {
      const boundary = new LocalStartup();
      await boundary.route(page, baseURL!);
      const sandboxId = boundary.sandbox.id;
      const selector = { kind: 'sandbox', sandbox_id: sandboxId };
      const operation = 'b'.repeat(64);
      const actor = { kind: 'human', subject: 'local-owner' };
      const original = {
        id: 'sandbox-share',
        owner: { kind: 'person', subject: 'local-owner' },
        selector,
        target_org_id: 'research',
        mode: 'org_roles',
        issuer_actor: actor,
        created_at: localTime,
        revoked_at: null,
        revision: 1,
      };
      let lost = true;
      let finished = false;
      let statusUnavailable = false;
      let unresolved = 1;
      let statusReads = 0;
      const writes: { method: string; body: unknown; key?: string; revision: string | null }[] = [];
      await page.route('**/api/v1/organizations', (route) =>
        route.fulfill({
          json: { items: [{ id: 'research', display_name: 'Research team', role: 'admin' }] },
        }),
      );
      await page.route('**/api/v1/resource-access/**', (route) =>
        route.fulfill({
          json: {
            capabilities: ['view_details', 'manage_sharing'],
            revalidate_at: null,
            policy_revision: 1,
          },
        }),
      );
      await page.route('**/api/v1/resource-shares/**', (route) => {
        const request = route.request();
        if (request.method() === 'GET')
          return route.fulfill({
            json: {
              items:
                finished && change === 'revoke'
                  ? []
                  : [finished ? { ...original, revision: 2, mode: 'view' } : original],
            },
          });
        writes.push({
          method: request.method(),
          body: request.postData() ? request.postDataJSON() : null,
          key: request.headers()['idempotency-key'],
          revision: new URL(request.url()).searchParams.get('expected_revision'),
        });
        if (lost) return route.abort('failed');
        if (success) {
          finished = true;
          return route.fulfill({
            json: {
              operation_id: operation,
              actor,
              at: localTime,
              policy_revision: 2,
              share: {
                ...original,
                mode: change === 'revoke' ? 'org_roles' : 'view',
                revision: 2,
                revoked_at: change === 'revoke' ? localTime : null,
              },
              revocation: {
                operation_id: operation,
                kind: 'sandbox',
                id: sandboxId,
                share_id: original.id,
                expected_revision: 1,
              },
            },
          });
        }
        return route.fulfill({
          status: 409,
          headers: {
            Link: `</api/v1/resource-revocations/${operation}?kind=sandbox&id=${sandboxId}>; rel="status"`,
          },
          json: { code: 'resource_revocation_pending', detail: 'Pending' },
        });
      });
      await page.route('**/api/v1/resource-revocations/**', (route) => {
        statusReads++;
        expect(route.request().method()).toBe('GET');
        if (statusUnavailable)
          return route.fulfill({ status: 503, json: { code: 'resource_revocation_unavailable' } });
        return route.fulfill({
          json: {
            operation_id: operation,
            selector,
            share_id: original.id,
            expected_revision: 1,
            state: finished ? 'effective' : 'evidence_received',
            participants: 1,
            stopped: 1,
            unresolved_effects: unresolved,
            deadline_unix_ms: 11000,
            deadline_exceeded: !finished,
            receipt: finished
              ? {
                  operation_id: operation,
                  actor,
                  at: localTime,
                  policy_revision: 2,
                  share: {
                    ...original,
                    mode: change === 'revoke' ? 'org_roles' : 'view',
                    revision: 2,
                    revoked_at: change === 'revoke' ? localTime : null,
                  },
                }
              : null,
          },
        });
      });
      const open = async () => {
        await page.getByRole('button', { name: 'More actions', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Sharing', exact: true }).click();
        await expect(page.getByRole('heading', { name: 'Current grants' })).toBeVisible();
      };
      await page.goto(`/sandboxes/${sandboxId}`);
      await open();
      if (change === 'revoke') {
        await page.getByRole('button', { name: 'Revoke', exact: true }).click();
        await page.getByRole('button', { name: 'Revoke grant', exact: true }).click();
      } else {
        await page.getByRole('button', { name: 'Access for Research team', exact: true }).click();
        await page.getByRole('menuitemradio', { name: 'View only', exact: true }).click();
        await page.getByRole('button', { name: 'Save access', exact: true }).click();
      }
      await expect(page.getByRole('button', { name: 'Retry saved share change' })).toBeEnabled();
      expect(writes).toHaveLength(1);
      await page.reload();
      await open();
      expect(writes).toHaveLength(1);
      lost = false;
      await page.getByRole('button', { name: 'Retry saved share change' }).click();
      if (!success) await expect(page.getByLabel('Revocation status')).toBeVisible();
      else
        await expect(
          page.getByText(/Previously submitted external requests may still complete/),
        ).toBeVisible();
      expect(writes).toHaveLength(2);
      expect(writes[1]).toEqual(writes[0]);
      await page.reload();
      await open();
      if (!success) await expect(page.getByLabel('Revocation status')).toBeVisible();
      else
        await expect(
          page.getByText(/Previously submitted external requests may still complete/),
        ).toBeVisible();
      expect(writes).toHaveLength(2);
      finished = true;
      await expect(
        page.getByText(
          `${change === 'revoke' ? 'Access revoked.' : 'Access change confirmed.'} Previously submitted external requests may still complete.`,
        ),
      ).toBeVisible();
      const previousReads = statusReads;
      statusUnavailable = true;
      await page.reload();
      await open();
      await expect(
        page.getByText(/Previously submitted external requests may still complete/),
      ).toBeVisible();
      await expect(
        page.getByText(/The latest external request status could not be checked/),
      ).toBeVisible();
      expect(statusReads).toBeGreaterThan(previousReads);
      if (change === 'set_mode')
        await expect(page.getByRole('button', { name: 'Revoke', exact: true })).toBeEnabled();
      statusUnavailable = false;
      unresolved = 0;
      await page.getByRole('button', { name: 'Check external requests', exact: true }).click();
      await expect(page.getByRole('region', { name: 'Unresolved external requests' })).toHaveCount(
        0,
      );
      await page.reload();
      await open();
      await expect(page.getByRole('region', { name: 'Unresolved external requests' })).toHaveCount(
        0,
      );
      expect(writes).toHaveLength(2);
      expect(writes[0]!.key).toBeTruthy();
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
    });
  }
}
