import { randomUUID } from 'node:crypto';
import type { ProfileDocument, SandboxProfileContent } from '@chronoai/cma-sdk';
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
  const fixtureWorkspace = h.fixture.profiles.workspace;
  const repository = await h.get<{
    content: { location: { type: string; url?: string; reference?: string } };
  }>(
    page,
    `/api/v1/workspace-profiles/${fixtureWorkspace.id}/revisions/${fixtureWorkspace.revision}`,
  );
  if (repository.content.location.type !== 'git_repository' || !repository.content.location.url)
    throw new Error(
      'Required E2E J5 prerequisite: Workspace fixture must pin a verifiable public repository',
    );
  const skills = await h.get<{ items: { id: string; name: string; versions: string[] }[] }>(
    page,
    '/api/v1/profile-authoring/skills',
  );
  const skill = skills.items.find((entry) => entry.versions.length > 0);
  if (!skill)
    throw new Error(
      'Required E2E J5 prerequisite: live Ornn catalogue must expose a published release',
    );
  const bounds = await h.get<{ min_cpu_millis: number; min_memory_mib: number }>(
    page,
    '/api/v1/profile-authoring/bounds',
  );
  const services = await h.get<{ id: string; name: string }[]>(
    page,
    '/api/v1/profile-authoring/services',
  );
  const service = services.find((entry) => h.fixture.baselineServices.includes(entry.id));
  if (!service)
    throw new Error(
      'Required E2E J5 prerequisite: service catalogue must expose a baseline service',
    );
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
    if (kind === 'agent') {
      await page.getByLabel('Search Ornn skills', { exact: true }).fill(skill.id);
      await page.getByRole('button', { name: `Add ${skill.name}`, exact: true }).click();
      await expect(
        page.getByRole('button', { name: `Version for ${skill.id}`, exact: true }),
      ).toHaveText(skill.versions[0]!);
      await page
        .getByLabel('System prompt', { exact: true })
        .fill('Act as a careful repository researcher. Preserve repository instructions.');
    }
    if (kind === 'workspace') {
      await page
        .getByLabel('Repository URL', { exact: true })
        .fill(repository.content.location.url);
      await page
        .getByLabel('Branch or tag', { exact: true })
        .fill(repository.content.location.reference ?? '');
      await page.getByRole('button', { name: `Add ${names.agent}`, exact: true }).click();
      const writes = h.commands.filter((row) => row.path === '/api/v1/workspace-profiles').length;
      await page.getByRole('button', { name: 'Save draft', exact: true }).click();
      expect(h.commands.filter((row) => row.path === '/api/v1/workspace-profiles')).toHaveLength(
        writes,
      );
      await page.getByRole('button', { name: 'Verify repository', exact: true }).click();
      await expect(page.getByText('Repository verified', { exact: true })).toBeVisible();
    }
    if (kind === 'sandbox') {
      await page.getByRole('radio', { name: 'Custom', exact: true }).check();
      await page
        .getByLabel('CPU cores', { exact: true })
        .fill(String(bounds.min_cpu_millis / 1000));
      await page
        .getByLabel('Memory (GiB)', { exact: true })
        .fill(String(bounds.min_memory_mib / 1024));
      await page.getByRole('checkbox', { name: service.name, exact: true }).check();
      await page.getByRole('button', { name: `Add ${names.workspace}`, exact: true }).click();
    }
    ids[kind] = await publishProfile(page, h, kind);
  }
  const second = await h.open('secondOwner');
  for (const kind of ['agent', 'sandbox'] as const) {
    await second.goto(`${h.config.baseURL}/marketplace/${kind}/${ids[kind]}`);
    await expect(second.getByRole('heading', { name: names[kind], exact: true })).toBeVisible();
    await second.getByRole('button', { name: 'Customize a private copy', exact: true }).click();
    await second.waitForURL(new RegExp(`/profiles/${kind}-profiles/[^/]+$`));
    const forkId = new URL(second.url()).pathname.split('/').at(-1)!;
    expect(forkId).not.toBe(ids[kind]);
    const source = await h.get<ProfileDocument<unknown>>(
      page,
      `/api/v1/${kind}-profiles/${ids[kind]}`,
    );
    const fork = await h.get<ProfileDocument<unknown>>(
      second,
      `/api/v1/${kind}-profiles/${forkId}`,
    );
    expect(fork.owner).toBe(h.fixture.accounts.secondOwner.subject);
    expect(fork.forked_from).toEqual({ id: ids[kind], revision: 1 });
    expect(fork.draft).toEqual(source.draft);
    if (kind === 'sandbox') {
      expect((fork.draft as SandboxProfileContent).workspace_profiles).toEqual([
        { id: ids.workspace, revision: 1 },
      ]);
    }
    await h.flush();
    expect(h.journal.resources).toContainEqual({
      kind: `${kind}-profiles`,
      id: forkId,
      subject: h.fixture.accounts.secondOwner.subject,
    });
    await h.capture(second, `j5-second-owner-${kind}-fork`);
  }
  await page.goto(`${h.config.baseURL}/triggers/manage`);
  for (const kind of ['sandbox', 'workspace', 'agent'] as const) {
    await page.locator(`#publisher-${kind}`).click();
    await page
      .getByRole('menu', { name: `${kind} profile`, exact: true })
      .locator(`[data-id="${ids[kind]}:1"]`)
      .click();
  }
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
