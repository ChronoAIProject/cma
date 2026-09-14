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
    let skillCalls = 0;
    let serviceCalls = 0;
    const resolutions: unknown[] = [];
    let releaseRepository!: () => void;
    const pendingRepository = new Promise<void>((resolve) => {
      releaseRepository = resolve;
    });
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
      if (request.method() !== 'GET' && !url.pathname.endsWith('/repositories/resolve'))
        writes.push(`${request.method()} ${url.pathname}`);
      if (
        ['/api/v1/sandboxes', '/api/v1/workspace-profiles', '/api/v1/agent-profiles'].includes(
          url.pathname,
        )
      ) {
        await reply({ items: [] });
      } else if (url.pathname === '/api/v1/profile-authoring/skills') {
        if (++skillCalls === 1) {
          await reply({ status: 503, code: 'profile_skills_unavailable' }, 503);
          return;
        }
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
        if (++serviceCalls === 1)
          await reply({ status: 503, code: 'profile_services_unavailable' }, 503);
        else await reply([{ id: 'github', name: 'GitHub' }]);
      } else if (url.pathname === '/api/v1/profile-authoring/repositories/resolve') {
        resolutions.push(request.postDataJSON());
        const attempt = resolutions.length;
        if (attempt === 1) await pendingRepository;
        if (attempt === 2) {
          await reply({ status: 503, code: 'repository_unavailable' }, 503);
        } else {
          await reply({
            url: 'https://github.com/local-fixture/research',
            reference: attempt === 1 ? 'main' : 'release',
            default_branch: 'main',
            commit_sha: 'a'.repeat(40),
          });
        }
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
    await expect(page.getByText('Skills could not be loaded.')).toBeVisible();
    await page.getByLabel('Name', { exact: true }).fill('Pinned research');
    await page.getByRole('button', { name: 'Try again', exact: true }).click();
    await page.getByRole('button', { name: 'Add Repository research' }).click();
    const version = page.getByRole('button', { name: 'Version for Research' });
    await expect(version).toHaveText('1234567890.123456789');
    await version.scrollIntoViewIfNeeded();
    await layout(page);
    await version.click();
    await expect(page.getByRole('menuitemradio', { name: '1.1' })).toBeVisible();
    await layout(page);
    await page.screenshot({ path: info.outputPath('release-menu.png'), fullPage: true });
    await page.getByRole('menuitemradio', { name: '1.1' }).click();
    await expect(version).toHaveText('1.1');
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('Pinned research');
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
    await page
      .getByText('Resource limits are unavailable.')
      .locator('..')
      .getByRole('button', { name: 'Try again' })
      .click();
    await expect(page.getByRole('radio', { name: 'Custom' })).toBeEnabled();
    await page.getByRole('radio', { name: 'Custom' }).check();
    await page.getByLabel('CPU cores').fill('3');
    await page.getByLabel('Memory (GiB)').fill('6');
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('Retained research setup');
    await expect(page.getByText('Services could not be loaded.')).toBeVisible();
    await page
      .getByText('Services could not be loaded.')
      .locator('..')
      .getByRole('button', { name: 'Try again' })
      .click();
    await page.getByRole('checkbox', { name: 'GitHub' }).check();
    await page.getByLabel('CPU cores').scrollIntoViewIfNeeded();
    await layout(page);
    await page.screenshot({ path: info.outputPath('bounds-ready.png'), fullPage: true });
    await page.goto('/profiles/workspace-profiles/new');
    await page.getByLabel('Name', { exact: true }).fill('Retained repository draft');
    await page
      .getByLabel('Repository URL', { exact: true })
      .fill('https://github.com/local-fixture/research.git');
    await page.getByLabel('Branch or tag', { exact: true }).fill('main');
    await page.getByRole('button', { name: 'Verify repository', exact: true }).click();
    await expect.poll(() => resolutions.length).toBe(1);
    await page.getByLabel('Branch or tag', { exact: true }).fill('release');
    releaseRepository();
    await page.getByRole('button', { name: 'Save draft', exact: true }).click();
    expect(writes).toEqual([]);
    await expect(page.getByText('Repository verified', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Verify repository', exact: true }).click();
    await expect(
      page.getByText(
        'The public repository or reference could not be verified. Check the URL and try again.',
      ),
    ).toBeVisible();
    await layout(page);
    await page.screenshot({ path: info.outputPath('repository-error.png'), fullPage: true });
    await page.getByRole('button', { name: 'Verify repository', exact: true }).click();
    await expect(page.getByText('Repository verified', { exact: true })).toBeVisible();
    await expect(
      page.getByText('Verify the repository and reference before saving.', { exact: true }),
    ).toHaveCount(0);
    await expect(page.getByLabel('Repository URL', { exact: true })).not.toHaveAttribute(
      'aria-invalid',
      'true',
    );
    await expect(page.getByLabel('Repository URL', { exact: true })).toHaveValue(
      'https://github.com/local-fixture/research',
    );
    await expect(page.getByLabel('Branch or tag', { exact: true })).toHaveValue('release');
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('Retained repository draft');
    expect(resolutions).toEqual([
      { url: 'https://github.com/local-fixture/research.git', reference: 'main' },
      { url: 'https://github.com/local-fixture/research.git', reference: 'release' },
      { url: 'https://github.com/local-fixture/research.git', reference: 'release' },
    ]);
    await layout(page);
    await page.screenshot({ path: info.outputPath('repository-ready.png'), fullPage: true });
    expect(writes).toEqual([]);
    expect(unexpected).toEqual([]);
    expect(errors).toEqual([]);
    await info.attach('local-authoring-checks', {
      body: JSON.stringify({
        mocked: true,
        boundary: 'built_browser_public_http',
        viewport,
        captures: 6,
        errors,
        unexpected,
        writes,
        repositoryReads: resolutions.length,
      }),
      contentType: 'application/json',
    });
  });
}
