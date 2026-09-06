import { randomUUID } from 'node:crypto';
import type { Page } from '@playwright/test';
import { test, expect, type Harness } from './harness';

async function publishProfile(page: Page, h: Harness, kind: 'agent' | 'workspace' | 'sandbox') {
  await page.getByRole('button', { name: 'Save draft', exact: true }).click();
  await page.waitForURL(new RegExp(`/profiles/${kind}-profiles/(?!new$)[^/]+$`));
  await page.getByRole('button', { name: 'Publish revision', exact: true }).click();
  await page
    .getByRole('dialog')
    .getByRole('button', { name: 'Publish revision 1', exact: true })
    .click();
  await page.getByRole('button', { name: 'Visibility', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Public', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Visibility', exact: true })).toContainText(
    'Public',
  );
  await h.flush();
  const id = h.journal.resources.filter((row) => row.kind === `${kind}-profiles`).at(-1)?.id;
  expect(id).toBeTruthy();
  return id!;
}

test('J5 profiles publish to Marketplace, second owner forks, wizard publishes embeds and stats', async ({
  harness: h,
}) => {
  const page = await h.open('owner');
  const suffix = randomUUID().slice(0, 8);
  const names = {
    agent: `E2E Agent ${suffix}`,
    workspace: `E2E Workspace ${suffix}`,
    sandbox: `E2E Sandbox ${suffix}`,
  };
  const ids: Partial<Record<'agent' | 'workspace' | 'sandbox', string>> = {};
  for (const kind of ['agent', 'workspace', 'sandbox'] as const) {
    await page.goto(`${h.config.baseURL}/profiles/${kind}-profiles/new`);
    await page.getByLabel('Name', { exact: true }).fill(names[kind]);
    await page
      .getByLabel('Description', { exact: true })
      .fill('Scoped staging verification fixture');
    if (kind === 'workspace') {
      await page.getByRole('radio', { name: 'Empty directory', exact: true }).check();
      await page.getByLabel('Empty directory name', { exact: true }).fill(`e2e-${suffix}`);
      await page.getByRole('button', { name: `Add ${names.agent}`, exact: true }).click();
    }
    if (kind === 'sandbox')
      await page.getByRole('button', { name: `Add ${names.workspace}`, exact: true }).click();
    ids[kind] = await publishProfile(page, h, kind);
  }
  const second = await h.open('secondOwner');
  await second.goto(`${h.config.baseURL}/marketplace/agent/${ids.agent}`);
  await expect(second.getByRole('heading', { name: names.agent, exact: true })).toBeVisible();
  await second.getByRole('button', { name: 'Fork to my profiles', exact: true }).click();
  await second.waitForURL(/\/profiles\/agent-profiles\/[^/]+$/);
  expect(new URL(second.url()).pathname.split('/').at(-1)).not.toBe(ids.agent);
  await h.capture(second, 'j5-second-owner-fork');
  await page.goto(`${h.config.baseURL}/triggers/manage`);
  for (const kind of ['sandbox', 'workspace', 'agent'] as const)
    await page.locator(`#publisher-${kind}`).selectOption(`${ids[kind]}:1`);
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.locator('[aria-label="Agent reuse"] input[value="always_new"]').check();
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.locator('#trigger-title').fill(`E2E Trigger ${suffix}`);
  await page.locator('#trigger-description').fill('Scoped staging verification');
  await page
    .locator('#trigger-first_message')
    .fill('Reply with the current working directory. Do not modify files.');
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.locator('#placement-label').fill('Staging HTML');
  await page.locator('#placement-source').fill(h.fixture.embeds.html.url);
  await page.getByRole('button', { name: 'Add source URL', exact: true }).click();
  await expect(page.locator('#placement-label')).toHaveValue('');
  await page.getByRole('button', { name: 'Next', exact: true }).click();
  await page.locator('.trigger-review input[type="checkbox"]').check();
  await page.getByRole('button', { name: 'Publish revision 1', exact: true }).click();
  await page.waitForURL(/\/triggers\/manage\/[^/]+\/embed$/);
  await expect(page.getByLabel('README or Markdown snippet', { exact: true })).not.toBeEmpty();
  await page.getByRole('tab', { name: 'Web page', exact: true }).click();
  await expect(page.getByLabel('Web page snippet', { exact: true })).not.toBeEmpty();
  await page.getByRole('tab', { name: 'Page with JavaScript', exact: true }).click();
  await expect(page.getByLabel('Subresource integrity', { exact: true })).toContainText('sha384-');
  await expect(page.locator('.trigger-stats')).not.toContainText('Loading');
  await expect(page.locator('.trigger-stats')).not.toContainText('Unavailable');
  await h.capture(page, 'j5-published-embed');
});

for (const kind of ['markdown', 'html', 'javascript', 'react'] as const) {
  test(`G-T1-${kind} real embed click-through preserves placement attribution`, async ({
    harness: h,
  }) => {
    const page = await h.open('visitor');
    const fixture = h.fixture.embeds[kind];
    const response = await page.goto(fixture.url);
    if (kind === 'javascript') {
      expect(response?.headers()['content-security-policy']).toBeTruthy();
      expect(response?.headers()['content-security-policy']).not.toContain("'unsafe-inline'");
    }
    if (kind === 'markdown') {
      const image = page.locator('a[href*="/t/"] img').first();
      await expect(image).toBeVisible();
      expect(await image.evaluate((element: HTMLImageElement) => element.naturalWidth > 0)).toBe(
        true,
      );
      expect(new URL((await image.getAttribute('src')) ?? '').origin).not.toBe(h.config.baseURL);
    }
    const popup = page
      .context()
      .waitForEvent('page', { timeout: 10_000 })
      .catch(() => undefined);
    // A published fixture exposes exactly one CMA link/button with this accessible name.
    await page.getByRole('link', { name: 'Open in CMA', exact: true }).click();
    const landing = new URL(page.url()).origin === h.config.baseURL ? page : await popup;
    expect(landing).toBeDefined();
    await landing!.waitForURL(/\/t\/[^/]+\/[^/]+$/);
    expect(new URL(landing!.url()).pathname.endsWith(`/${fixture.placementId}`)).toBe(true);
    await landing!.locator('.public-actions button').click();
    const launch = await h.currentLaunch(landing!);
    expect(launch.placement_id).toBe(fixture.placementId);
    expect(launch.referrer_origin).toBe(new URL(fixture.url).origin);
    await h.ready(landing!, launch.id);
    const owner = await h.open('owner');
    const stats = await h.get<{
      placements: { placement_id: string; launches_total: number; agents_ready_total: number }[];
    }>(owner, `/api/v1/triggers/${launch.trigger_id}/stats`);
    const placement = stats.placements.find((row) => row.placement_id === fixture.placementId);
    expect(placement?.launches_total).toBeGreaterThan(0);
    expect(placement?.agents_ready_total).toBeGreaterThan(0);
    await h.capture(landing!, `g-t1-${kind}-ready`);
  });
}
