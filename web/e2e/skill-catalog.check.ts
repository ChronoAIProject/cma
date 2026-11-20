import { test, expect } from '@playwright/test';
import {
  openSkills,
  privateBody,
  problem,
  secondSkill,
  skill,
  SkillCatalogFixture,
} from './skill-catalog.fixture';

for (const scenario of [
  {
    name: 'sign-in',
    status: 401,
    code: 'not_authenticated',
    message: 'Sign in again',
    retry: false,
  },
  {
    name: 'permission',
    status: 403,
    code: 'forbidden',
    message: 'do not have permission',
    retry: false,
  },
  {
    name: 'rate limit',
    status: 429,
    code: 'rate_limited',
    message: 'Too many catalog requests',
    retry: true,
  },
  {
    name: 'unavailable',
    status: 503,
    code: 'profile_skills_unavailable',
    message: 'temporarily unavailable',
    retry: true,
  },
  { name: 'network', message: 'could not be reached', retry: true },
  { name: 'HTML', message: 'invalid response', retry: true },
  { name: 'malformed', message: 'invalid response', retry: true },
]) {
  test(`LOCAL Ready Agent catalog ${scenario.name} is safe and independent of installed skills`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new SkillCatalogFixture();
    boundary.catalog = (route) =>
      scenario.status !== undefined
        ? problem(route, scenario.status, scenario.code!)
        : scenario.name === 'network'
          ? route.abort('connectionfailed')
          : scenario.name === 'HTML'
            ? route.fulfill({ contentType: 'text/html', body: privateBody })
            : route.fulfill({
                json: {
                  items: [{ id: skill.id, name: skill.name, versions: [null] }],
                  detail: privateBody,
                },
              });
    await boundary.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    const details = await openSkills(page);
    await expect(details.getByText(scenario.message, { exact: false })).toBeVisible();
    await expect(details.getByText('Saved skill', { exact: true })).toBeVisible();
    await expect(
      details.getByRole('textbox', { name: 'Search available Ornn skills' }),
    ).toBeDisabled();
    await expect(details.getByText('No matching Ornn skills.')).toHaveCount(0);
    const retry = details.getByRole('button', { name: 'Retry Ornn catalog' });
    await expect(retry).toHaveCount(scenario.retry ? 1 : 0);
    if (scenario.status)
      await expect(details.getByText(`HTTP status: ${scenario.status}`)).toBeVisible();
    if (scenario.status === 429) {
      await expect(retry).toBeDisabled();
      expect(boundary.catalogReads).toHaveLength(1);
      await expect(retry).toBeEnabled();
      expect(boundary.catalogReads).toHaveLength(1);
    }
    expect(await page.content()).not.toContain('synthetic-private-body');
    expect(boundary.installs).toEqual([]);
    boundary.assertReadBoundary();
  });
}

test('LOCAL explicit recovery preserves failed pages and install keys across retries and reload', async ({
  page,
  baseURL,
}) => {
  const boundary = new SkillCatalogFixture();
  let failures = 2;
  let pageFailures = 1;
  boundary.catalog = (route) => {
    if (failures-- > 0) return problem(route, 503, 'profile_skills_unavailable');
    const params = new URL(route.request().url()).searchParams;
    if (!params.get('q')) return route.fulfill({ json: { items: [] } });
    if (!params.has('cursor'))
      return route.fulfill({ json: { items: [skill], next: 'page/two+' } });
    if (pageFailures-- > 0) return problem(route, 503, 'profile_skills_unavailable');
    return route.fulfill({ json: { items: [skill, secondSkill] } });
  };
  await boundary.route(page, baseURL!);
  await page.goto('/agents/agt_contract');
  const details = await openSkills(page);
  const retry = details.getByRole('button', { name: 'Retry Ornn catalog' });
  await retry.click();
  await expect(details.getByText('HTTP status: 503')).toBeVisible();
  await expect(details.getByText('No matching Ornn skills.')).toHaveCount(0);
  await retry.click();
  await expect(details.getByText('No matching Ornn skills.')).toBeVisible();
  const search = details.getByRole('textbox', { name: 'Search available Ornn skills' });
  await expect(search).toBeEnabled();
  await search.fill('review');
  const install = details.getByRole('button', { name: 'Install Review', exact: true });
  await expect(install).toBeEnabled();
  await details.getByRole('button', { name: 'More Ornn skills' }).click();
  const pageRetry = details.getByRole('button', { name: 'Retry skill page' });
  await expect(pageRetry).toBeVisible();
  await expect(install).toBeDisabled();
  await expect(search).toHaveValue('review');
  const beforeRetry = boundary.catalogReads.at(-1);
  await pageRetry.click();
  await expect(details.getByRole('button', { name: 'Install Search' })).toBeEnabled();
  expect(boundary.catalogReads.at(-1)).toEqual(beforeRetry);
  expect(beforeRetry).toEqual({ query: 'review', cursor: 'page/two+' });
  await expect(
    details.getByRole('list', { name: 'Available Ornn skills' }).getByRole('listitem'),
  ).toHaveCount(2);
  expect(boundary.installs).toHaveLength(0);
  boundary.loseInstallResponse = true;
  await install.click();
  await details.getByRole('button', { name: 'Retry installation' }).click();
  await expect(details.getByText('Waiting for the Agent to confirm installation.')).toBeVisible();
  expect(boundary.installs).toHaveLength(2);
  expect(boundary.installs[0]).toEqual(boundary.installs[1]);
  expect(boundary.installs[0]?.key).toBeTruthy();
  expect(boundary.installs[0]?.body).toEqual({ id: 'review', version: '1.2' });
  boundary.installState = 'installed';
  await expect(details.getByText('1.2 · Installed', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Close Agent details' }).click();
  await openSkills(page);
  await expect(details.getByText('1.2 · Installed', { exact: true })).toBeVisible();
  await page.reload();
  await openSkills(page);
  await expect(details.getByText('1.2 · Installed', { exact: true })).toBeVisible();
  await expect(details.getByText('No matching Ornn skills.')).toBeVisible();
  expect(boundary.installedReads).toBeGreaterThan(2);
  expect(boundary.installs).toHaveLength(2);
  boundary.assertReadBoundary();
});
