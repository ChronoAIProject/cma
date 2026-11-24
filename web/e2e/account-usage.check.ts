import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';

const measured = process.env.CMA_E2E_MEASURED_USAGE === 'true';

test('LOCAL selected Usage names survive Apply, reopen and exact URL recovery', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  const agentName = 'Research Agent with a detailed readable title '.repeat(8).trim();
  const agent = {
    ...boundary.agent,
    id: 'agt_usage',
    owner_title: agentName,
    sandbox: { ...boundary.agent.sandbox, id: 'sbx-1' },
  };
  const agentReads: URL[] = [];
  const requests: URL[] = [];
  page.on('request', (request) => requests.push(new URL(request.url())));
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.route(/\/api\/v1\/agents(?:\?|$)/, (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get('sandbox') !== 'sbx-1') return route.fallback();
    agentReads.push(url);
    return route.fulfill({
      json: {
        items: [
          {
            ...agent,
            sandbox_id: agent.sandbox.id,
            workspace_id: agent.workspace.id,
            worktree_id: agent.worktree.id,
          },
        ],
      },
    });
  });
  await page.route('**/api/v1/agents/agt_usage', (route) => {
    agentReads.push(new URL(route.request().url()));
    return route.fulfill({ json: agent });
  });
  let releaseReport!: () => void;
  const resumed = new Promise<void>((resolve) => {
    releaseReport = resolve;
  });
  await page.route(/\/usage\/metrics\/reports\/[^/?]+$/, async (route) => {
    if (metrics.pending) await resumed;
    await route.fallback();
  });
  await page.goto('/agent-manager');
  await page.getByRole('button', { name: /Account menu for/ }).click();
  await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await dialog.getByRole('button', { name: 'Sandbox: All my Sandboxes', exact: true }).click();
  await dialog.getByRole('button', { name: 'Choose sandbox', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Research Sandbox (sbx-1)', exact: true }).click();
  const sandboxButton = dialog.getByRole('button', {
    name: 'Sandbox: Research Sandbox',
    exact: true,
  });
  const agentButton = dialog.getByRole('button', { name: `Agent: ${agentName}`, exact: true });
  await expect(sandboxButton).toBeVisible();
  await expect(page).toHaveURL(/\/usage$/);
  await dialog.getByRole('button', { name: 'Apply usage filters' }).click();
  await dialog
    .getByRole('button', { name: 'Agent: All Agents in this Sandbox', exact: true })
    .click();
  await dialog.getByRole('button', { name: 'Usage Agent', exact: true }).click();
  await page.getByRole('menuitemradio', { name: `${agentName} (agt_usage)`, exact: true }).click();
  await expect(agentButton).toBeVisible();
  expect(new URL(page.url()).searchParams.has('agent')).toBe(false);
  metrics.pending = true;
  await dialog.getByRole('button', { name: 'Apply usage filters' }).click();
  await expect(dialog.getByText('Preparing usage report…')).toBeVisible();
  await expect(agentButton).toHaveCount(0);
  await expect(sandboxButton).toBeVisible();
  metrics.pending = false;
  releaseReport();
  await expect(agentButton).toBeVisible();
  const detailReads = () => metrics.reads.filter((url) => /\/sandboxes\/sbx-1$/.test(url.pathname));
  expect(detailReads()).toHaveLength(0);
  expect(agentReads).toHaveLength(1);
  const selectionURL = page.url();
  expect(new URL(selectionURL).searchParams.get('sandbox')).toBe('sbx-1');
  expect(new URL(selectionURL).searchParams.get('agent')).toBe('agt_usage');
  expect(metrics.creates.at(-1)?.input.filters).toMatchObject({
    sandbox_id: 'sbx-1',
    agent_id: 'agt_usage',
  });
  expect(selectionURL).not.toContain('Research');
  await page.setViewportSize({ width: 390, height: 480 });
  const usageBody = dialog.locator('.modal__body');
  expect(await usageBody.evaluate((node) => node.scrollHeight - node.clientHeight)).toBeGreaterThan(
    60,
  );
  await sandboxButton.click();
  const sandboxSearch = dialog.getByRole('textbox', { name: 'Search sandbox choices' });
  await sandboxSearch.focus();
  const scrollTop = await usageBody.evaluate((node) => {
    node.scrollTop = 60;
    return node.scrollTop;
  });
  expect(scrollTop).toBeGreaterThan(0);
  await page.keyboard.press('Escape');
  await expect(sandboxSearch).toHaveCount(0);
  await expect(dialog).toBeVisible();
  await expect(sandboxButton).toBeFocused();
  await expect(agentButton).toBeVisible();
  await expect(page).toHaveURL(selectionURL);
  expect(await usageBody.evaluate((node) => node.scrollTop)).toBe(scrollTop);
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await page.goForward();
  await expect(agentButton).toBeVisible();
  await expect(sandboxButton).toBeVisible();
  expect(agentReads).toHaveLength(1);
  const inventoryReads = metrics.reads.filter((url) => url.pathname.endsWith('/sandboxes')).length;
  await page.reload();
  await expect(agentButton).toBeVisible();
  await expect(sandboxButton).toBeVisible();
  expect(detailReads()).toHaveLength(1);
  expect(agentReads).toHaveLength(2);
  expect(agentReads[1]!.pathname).toBe('/api/v1/agents/agt_usage');
  // Reload reads the report's first table page; label recovery never scans inventory pages.
  expect(metrics.reads.filter((url) => url.pathname.endsWith('/sandboxes'))).toHaveLength(
    inventoryReads + 1,
  );
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    const title = agentButton.locator('.usage-discovery-value');
    await expect(title).toHaveAttribute('title', agentName);
    expect(
      await title.evaluate((node) => ({
        truncated: node.scrollWidth > node.clientWidth,
        overflow: getComputedStyle(node).textOverflow,
      })),
    ).toEqual({ truncated: true, overflow: 'ellipsis' });
    await agentButton.scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`selected-names-${width}.png`) });
  }
  expect(
    await page.evaluate(() =>
      Object.values(localStorage).some((value) => value.includes('Research Agent')),
    ),
  ).toBe(false);
  expect(requests.filter((url) => url.pathname.includes('/admin/usage/metrics/options'))).toEqual(
    [],
  );
  expect(requests.every((url) => url.origin === baseURL)).toBe(true);
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

test('LOCAL account Usage preserves the Agent, owner scope, focus and browser history', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await editor.fill('Keep this unsent draft');
  await editor.evaluate((node) => node.setAttribute('data-retained-usage-proof', 'true'));
  const trigger = page.getByRole('button', { name: /Account menu for/ });
  await trigger.click();
  await expect(page.getByRole('menuitem', { name: 'Settings', exact: true })).toHaveCount(0);
  const appearance = page.getByRole('group', { name: 'Appearance' });
  for (const theme of ['System', 'Light', 'Dark']) {
    await appearance.getByRole('menuitemradio', { name: theme, exact: true }).click();
    await expect(
      appearance.getByRole('menuitemradio', { name: theme, exact: true }),
    ).toHaveAttribute('aria-checked', 'true');
  }
  await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await expect(dialog.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
  expect(metrics.creates.every((read) => !read.admin && !read.input.filters?.owner)).toBe(true);
  await expect(page.getByRole('dialog')).toHaveCount(1);
  if (measured) {
    await dialog.getByRole('button', { name: 'Memory working set', exact: true }).click();
    await expect(
      dialog.getByText('View Memory working set chart data', { exact: true }),
    ).toBeVisible();
  } else {
    await expect(dialog.getByRole('region', { name: 'Allocated capacity now' })).toContainText(
      '2 cores',
    );
    await expect(dialog.getByRole('region', { name: 'Allocated resource-hours' })).toContainText(
      '480',
    );
    await expect(dialog.getByRole('button', { name: 'Memory working set' })).toHaveCount(0);
    await expect(dialog.getByRole('region', { name: 'Measured resource charts' })).toHaveCount(0);
  }
  expect(metrics.creates).toHaveLength(1);
  await dialog.getByRole('button', { name: 'Research Sandbox', exact: true }).click();
  await dialog.getByRole('button', { name: 'Lifecycle history', exact: true }).click();
  await expect(dialog.getByRole('table', { name: 'Usage intervals' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Close usage details' }).click();
  for (const theme of ['light', 'dark', 'system']) {
    await page.emulateMedia({ colorScheme: theme === 'dark' ? 'dark' : 'light' });
    await page.evaluate((value) => {
      localStorage.setItem('cma:appearance', value);
      window.dispatchEvent(new StorageEvent('storage', { key: 'cma:appearance' }));
    }, theme);
    for (const width of [1440, 768, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await dialog.locator('.modal__body').evaluate((node) => node.scrollTo(0, 0));
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      await expect(dialog.getByRole('button', { name: 'Close', exact: true })).toBeInViewport();
      await page.screenshot({
        path: info.outputPath(`usage-${theme}-${width}.png`),
        animations: 'disabled',
      });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  // The responsive Agent panel can briefly take focus as an overlay during resize.
  // Wait for its desktop column before starting the dialog's keyboard-trap check.
  await expect(page.getByRole('separator', { name: 'Resize the details column' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Close', exact: true }).focus();
  await page.keyboard.press('Shift+Tab');
  expect(await dialog.evaluate((node) => node.contains(document.activeElement))).toBe(true);
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(page).toHaveURL(/\/agents\/agt_contract$/);
  await expect(editor).toHaveValue('Keep this unsent draft');
  await expect(editor).toHaveAttribute('data-retained-usage-proof', 'true');
  await expect(trigger).toBeFocused();
  await page.goForward();
  await expect(dialog).toBeVisible();
  await page.goBack();
  await expect(dialog).not.toBeVisible();
  if (!measured)
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

for (const path of ['/usage', '/settings/usage', '/settings/general', '/settings']) {
  test(`LOCAL direct usage and legacy link ${path}`, async ({ page, baseURL }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    metrics.measuredVisible = measured;
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await page.addInitScript(() => localStorage.setItem('cma:appearance', 'light'));
    await page.goto(`${path}?range=7d`);
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    await expect(dialog).toBeVisible();
    await expect(page).toHaveURL(/\/usage\?range=7d$/);
    await expect(dialog.getByText('Appearance is now in your account menu.')).toHaveCount(
      path === '/settings' || path === '/settings/general' ? 1 : 0,
    );
    expect(await page.evaluate(() => localStorage.getItem('cma:appearance'))).toBe('light');
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(page).toHaveURL(/\/agent-manager$/);
    if (!measured)
      expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });
}

for (const status of [403, 410, 429, 503]) {
  test(`LOCAL owner usage reports ${status} without claiming zero usage`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    metrics.failure = status;
    const transient = status === 429 || status === 503;
    // Use a bounded server floor while preserving the normal four attempts.
    // The separate backoff checks cover a full 30-second Retry-After deadline.
    if (transient) metrics.retryAfterSeconds = 1;
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    metrics.measuredVisible = measured;
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await page.goto('/usage');
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    if (transient) {
      await expect(
        dialog.getByText('Retrying usage automatically…', { exact: true }),
      ).toBeVisible();
      await expect(dialog.getByRole('alert')).toHaveCount(0);
    }
    await expect(dialog.getByRole('alert')).toBeVisible({ timeout: transient ? 15_000 : 5_000 });
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/reports'))).toHaveLength(
      transient ? 4 : 1,
    );
    await expect(dialog.getByText('No Sandboxes match this selection.')).toHaveCount(0);
    if (status === 429)
      await expect(dialog.getByRole('button', { name: 'Waiting to retry' })).toBeDisabled();
    if (!measured)
      expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });
}

test('LOCAL pending owner report resolves and empty selections are explicit', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  metrics.pending = true;
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.goto('/usage');
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await expect(dialog.getByText('Preparing usage report…')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
  expect(metrics.creates).toHaveLength(1);
  metrics.pending = false;
  await dialog.getByRole('searchbox', { name: 'Sandbox name or ID starts with' }).fill('empty');
  await dialog.getByRole('button', { name: 'Apply usage filters' }).click();
  await expect(dialog.getByText('No Sandboxes match this selection.')).toBeVisible();
  if (!measured)
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
  expect(boundary.writes).toEqual([]);
});

test('LOCAL mobile Usage returns focus to the retained account trigger', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
  const trigger = page.getByRole('button', { name: /Account menu for/ });
  await trigger.click();
  await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await expect(dialog).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(trigger).toBeFocused();
  await expect(trigger).toBeInViewport();
  if (!measured)
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
  expect(boundary.writes).toEqual([]);
});
