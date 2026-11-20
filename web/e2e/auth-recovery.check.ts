import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { ReplicaFixture } from './replica-recovery.fixture';
import { binding } from './agent-v2.fixture';
import { LocalStartup } from './agent-startup-fixtures';

const enabled = process.env.CMA_RECOVERY_ENABLED === 'true';
for (const resource of ['sandbox', 'workspace'] as const) {
  test(`LOCAL protected ${resource} access recheck recovers without writes with recovery flag ${enabled}`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalStartup();
    fixture.sandbox = { ...fixture.sandbox, status: 'credential_rotating' };
    fixture.workspace = { ...fixture.workspace, status: 'ready' };
    const routePath = resource === 'sandbox' ? '/sandboxes/sbx_route' : '/workspaces/wks_route';
    let unavailable = false;
    let refusals = 0;
    await fixture.route(page, baseURL!);
    await page.route('**/api/v1/resource-access/workspace/wks_route', (route) =>
      route.fulfill({
        json: {
          capabilities: ['view_details'],
          owner: { kind: 'person', is_current_person: true },
          revalidate_at: null,
          policy_revision: 1,
        },
      }),
    );
    await page.route(`**/api/v1${routePath}`, async (route) => {
      if (!unavailable) return route.fallback();
      refusals++;
      return route.fulfill({
        status: 503,
        headers: { 'retry-after': '3', 'cache-control': 'no-store' },
        contentType: 'application/problem+json',
        body: JSON.stringify({
          type: 'https://spec.chrono-ai.fun/cma/1.0/problems/identity-unavailable',
          title: 'Identity service temporarily unavailable',
          status: 503,
          detail: 'The identity service is temporarily unavailable. Please retry shortly.',
          code: 'identity_unavailable',
          retryable: true,
        }),
      });
    });
    await page.goto(routePath);
    await expect(page.locator('main h1')).toHaveText(fixture[resource].name);
    unavailable = true;
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect.poll(() => refusals, { timeout: 10_000 }).toBeGreaterThan(0);
    expect(fixture.writes).toEqual([]);
    const recoveredName = `Freshly verified ${resource}`;
    if (resource === 'sandbox') fixture.sandbox = { ...fixture.sandbox, name: recoveredName };
    else fixture.workspace = { ...fixture.workspace, name: recoveredName };
    unavailable = false;
    await page
      .getByRole('button', {
        name: `Check ${resource === 'sandbox' ? 'Sandbox' : 'Workspace'} access`,
      })
      .click();
    await expect(page.locator('main h1')).toHaveText(recoveredName, { timeout: 10_000 });
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(fixture.unexpected).toEqual([]);
  });
}

for (const action of ['send', 'cancel'] as const) {
  for (const phase of ['before-submit', 'confirmation'] as const) {
    test(`LOCAL typed auth outage ${action} ${phase} automatic ${enabled}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new ReplicaFixture();
      if (action === 'cancel') fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      let failedAt = 0;
      let faultPosts = -1;
      let faultActive = true;
      let authFailures = 0;
      try {
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        const input = page.getByRole('textbox', { name: 'Message', exact: true });
        const stop = page.getByRole('button', { name: 'Stop response', exact: true });
        await expect(input).toBeVisible();
        await expect.poll(() => fixture.streams.size).toBe(1);
        if (action === 'send') await input.fill('Auth recovery request');
        else await expect(stop).toBeEnabled();
        await page.route('**/auth/session', async (route) => {
          // A background read must not consume a single injected error before
          // the mutation's independent identity lane reaches the same outage.
          if (
            faultActive &&
            (phase === 'before-submit' || fixture.writes.length > 0) &&
            (!failedAt || performance.now() - failedAt < 500)
          ) {
            if (!failedAt) {
              failedAt = performance.now();
              faultPosts = fixture.writes.length;
            }
            authFailures += 1;
            return route.fulfill({
              status: 503,
              headers: { 'retry-after': '1', 'cache-control': 'no-store' },
              json: {
                type: 'about:blank',
                status: 503,
                code: 'identity_unavailable',
                retryable: true,
              },
            });
          }
          return route.fallback();
        });
        const submit = () =>
          action === 'send'
            ? page.getByRole('button', { name: 'Send message', exact: true }).click()
            : stop.click();
        await submit();
        await expect.poll(() => failedAt).toBeGreaterThan(0);
        expect(faultPosts).toBe(phase === 'confirmation' ? 1 : 0);
        if (!enabled) {
          await expect(
            page
              .getByRole('alert')
              .filter({ hasText: 'CMA had a problem on its side. Try again in a moment.' }),
          ).toBeVisible();
          expect(fixture.writes.length).toBe(faultPosts);
          faultActive = false;
          if (phase === 'confirmation')
            await page.getByRole('button', { name: 'Recover pending request' }).click();
          else {
            if (action === 'send')
              await expect(page.locator('.message--user')).toContainText('Not sent');
            await submit();
          }
        }
        if (action === 'send') {
          await expect(page.locator('.message--user')).toHaveCount(1);
          await expect(page.locator('.message--user')).toContainText('Sent');
        } else {
          await expect
            .poll(() => fixture.base.reads.includes('/api/v2/commands/cmd_control'))
            .toBe(true);
          await expect(page.getByRole('button', { name: 'Recover pending request' })).toHaveCount(
            0,
          );
          await expect(stop).toBeVisible();
        }
        const recoveredAt = performance.now();
        if (enabled) expect(recoveredAt - failedAt).toBeGreaterThanOrEqual(1000);
        expect(recoveredAt - failedAt).toBeLessThan(10_000);
        expect(fixture.writes).toHaveLength(!enabled && phase === 'confirmation' ? 2 : 1);
        expect(fixture.effects.size).toBe(1);
        if (action === 'cancel') {
          fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
          await expect(stop).toHaveCount(0);
        }
        if (fixture.writes.length === 2) {
          expect(fixture.writes[1]!.key).toBe(fixture.writes[0]!.key);
          expect(fixture.writes[1]!.body).toBe(fixture.writes[0]!.body);
        }
        expect(fixture.errors).toEqual([]);
        expect(fixture.base.errors).toEqual([]);
        await writeFile(
          info.outputPath('auth-recovery.json'),
          JSON.stringify(
            {
              source: process.env.CMA_RECOVERY_SOURCE ?? 'unspecified',
              enabled,
              action,
              phase,
              scope: 'compiled Chrome loopback auth/Agent HTTP fixtures, not production',
              recoveryMs: recoveredAt - failedAt,
              postsBeforeFault: faultPosts,
              posts: fixture.writes.length,
              effects: fixture.effects.size,
              authFailures,
              sameKey: fixture.writes.every((write) => write.key === fixture.writes[0]!.key),
            },
            null,
            2,
          ),
        );
      } finally {
        await fixture.close(page);
      }
    });
  }
}

for (const status of [401, 403]) {
  test(`LOCAL auth HTTP ${status} cannot authorize mutation despite retry metadata`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new ReplicaFixture();
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(input).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      await input.fill('Refused request');
      await page.route('**/auth/session', (route) =>
        route.fulfill({ status, json: { authenticated: false, retryable: true } }),
      );
      await page.getByRole('button', { name: 'Send message', exact: true }).click();
      await expect(
        page.getByText('Your sign-in changed. Open the Agent again to continue.'),
      ).toBeVisible();
      expect(fixture.writes).toHaveLength(0);
      expect(fixture.effects.size).toBe(0);
    } finally {
      await fixture.close(page);
    }
  });
}
