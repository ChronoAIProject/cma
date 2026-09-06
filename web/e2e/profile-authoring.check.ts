import { test, expect, type Page } from '@playwright/test';

// This is local built-UI evidence with intercepted public HTTP, never provider acceptance.
async function layout(page: Page) {
  const result = await page.evaluate(() => ({
    viewport: innerWidth,
    width: document.documentElement.scrollWidth,
    overflow: [
      ...document.querySelectorAll<HTMLElement>(
        '.profile-field, .profile-field-pair, .skill-picker__selected, .skill-picker__version, [role="menu"]',
      ),
    ]
      .filter(
        (element) =>
          element.clientWidth > 0 &&
          (element.scrollWidth > element.clientWidth + 1 ||
            element.getBoundingClientRect().right > innerWidth + 1 ||
            element.getBoundingClientRect().left < -1),
      )
      .map((element) => element.className),
  }));
  expect(result.width).toBeLessThanOrEqual(result.viewport + 1);
  expect(result.overflow).toEqual([]);
}

for (const viewport of [
  { width: 1440, height: 1000 },
  { width: 390, height: 844 },
  { width: 320, height: 740 },
]) {
  test(`LOCAL authoring release and resource states fit ${viewport.width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize(viewport);
    const errors: string[] = [];
    const unexpected: string[] = [];
    const writes: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    let releaseBounds!: () => void;
    const pendingBounds = new Promise<void>((resolve) => {
      releaseBounds = resolve;
    });
    let boundsCalls = 0;
    await page.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== baseURL) {
        unexpected.push(request.url());
        await route.abort();
        return;
      }
      const reply = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
      if (url.pathname === '/auth/session') {
        await reply({
          authenticated: true,
          subject: 'local-author',
          display_name: 'Local author',
          capabilities: ['manage_own_profiles'],
        });
        return;
      }
      if (!url.pathname.startsWith('/api/')) {
        await route.continue();
        return;
      }
      if (request.method() !== 'GET') writes.push(`${request.method()} ${url.pathname}`);
      if (url.pathname === '/api/v1/sandboxes' || url.pathname === '/api/v1/workspace-profiles') {
        await reply({ items: [] });
      } else if (url.pathname === '/api/v1/profile-authoring/skills') {
        await reply({
          items: [
            {
              id: 'research',
              name: 'Repository research',
              versions: ['1234567890.123456789', '1.1'],
              description: 'Published repository research releases',
            },
          ],
        });
      } else if (url.pathname === '/api/v1/profile-authoring/services') {
        await reply([{ id: 'github', name: 'GitHub' }]);
      } else if (url.pathname === '/api/v1/profile-authoring/bounds') {
        boundsCalls += 1;
        if (boundsCalls === 1) {
          await pendingBounds;
          await reply(
            { status: 503, code: 'unavailable', detail: 'Local bounds read failure.' },
            503,
          );
        } else {
          await reply({
            min_cpu_millis: 1000,
            max_cpu_millis: 4000,
            min_memory_mib: 1024,
            max_memory_mib: 8192,
          });
        }
      } else {
        unexpected.push(`${request.method()} ${url.pathname}`);
        await reply({ status: 404, code: 'unexpected_local_route' }, 404);
      }
    });
    await page.goto('/profiles/agent-profiles/new');
    await page.getByRole('button', { name: 'Add Repository research' }).click();
    const version = page.getByRole('button', { name: 'Version for research' });
    await expect(version).toHaveText('1234567890.123456789');
    await version.scrollIntoViewIfNeeded();
    await layout(page);
    await version.click();
    await expect(page.getByRole('menuitemradio', { name: '1.1' })).toBeVisible();
    await layout(page);
    await page.screenshot({ path: info.outputPath('release-menu.png'), fullPage: true });
    await page.getByRole('menuitemradio', { name: '1.1' }).click();
    await expect(version).toHaveText('1.1');
    await page.goto('/profiles/sandbox-profiles/new');
    await expect(page.getByText('Loading resource limits')).toBeVisible();
    await page.getByLabel('Name', { exact: true }).fill('Retained research setup');
    await expect(page.getByRole('radio', { name: 'Custom' })).toBeDisabled();
    await page.getByText('Loading resource limits').scrollIntoViewIfNeeded();
    await layout(page);
    await page.screenshot({ path: info.outputPath('bounds-loading.png'), fullPage: true });
    releaseBounds();
    await expect(page.getByText('Resource limits are unavailable.')).toBeVisible();
    await page.getByRole('button', { name: 'Save draft' }).click();
    expect(writes).toEqual([]);
    await page.getByText('Resource limits are unavailable.').scrollIntoViewIfNeeded();
    await layout(page);
    await page.screenshot({ path: info.outputPath('bounds-error.png'), fullPage: true });
    await page.getByRole('button', { name: 'Try again' }).click();
    await expect(page.getByRole('radio', { name: 'Custom' })).toBeEnabled();
    await page.getByRole('radio', { name: 'Custom' }).check();
    await page.getByLabel('CPU cores').fill('3');
    await page.getByLabel('Memory (GiB)').fill('6');
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('Retained research setup');
    await page.getByRole('checkbox', { name: 'GitHub' }).check();
    await page.getByLabel('CPU cores').scrollIntoViewIfNeeded();
    await layout(page);
    await page.screenshot({ path: info.outputPath('bounds-ready.png'), fullPage: true });
    expect(writes).toEqual([]);
    expect(unexpected).toEqual([]);
    expect(errors).toEqual([]);
    await info.attach('local-authoring-checks', {
      body: JSON.stringify({ viewport, captures: 4, errors, unexpected, writes }),
      contentType: 'application/json',
    });
  });
}
