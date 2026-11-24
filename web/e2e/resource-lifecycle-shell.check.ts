import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { navigation, noEffects } from './resource-lifecycle.fixture';

for (const width of [1440, 390]) {
  test(`LOCAL sign-in CTA preserves one handover and return target at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.route('**/auth/session', (route) =>
      route.fulfill({ json: { authenticated: false } }),
    );
    const targets: (string | null)[] = [];
    await page.route('**/auth/login?*', (route) => {
      targets.push(new URL(route.request().url()).searchParams.get('return_to'));
      return route.fulfill({ contentType: 'text/html', body: '<h1>Identity sign-in</h1>' });
    });
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'ChronoAI Managed Agent' })).toBeVisible();
    const cta = page.getByRole('main').getByRole('button', { name: 'Get Started', exact: true });
    await expect(cta).toBeEnabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
    await page.screenshot({ path: info.outputPath(`sign-in-${width}.png`), fullPage: true });
    await cta.dblclick();
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(targets).toEqual(['/agent-manager']);

    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/');
    await cta.focus();
    await expect(cta).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(targets).toEqual(['/agent-manager', '/agent-manager']);

    await page.addInitScript(() => {
      const getContext = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function (
        this: HTMLCanvasElement,
        ...args: Parameters<typeof getContext>
      ) {
        if (String(args[0]).startsWith('webgl') || args[0] === 'experimental-webgl') return null;
        return getContext.apply(this, args);
      } as typeof getContext;
    });
    await page.goto('/');
    await cta.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(targets).toEqual(['/agent-manager', '/agent-manager', '/agent-manager']);

    await page.goto('/');
    const service = page.getByRole('button', { name: 'Open Event Gateway', exact: true });
    await service.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(targets).toEqual([
      '/agent-manager',
      '/agent-manager',
      '/agent-manager',
      '/event-gateway',
    ]);
    noEffects(boundary);
  });

  test(`LOCAL authenticated home and navigation boundaries at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    await navigation(page, true);
    const sidebar = page.getByRole('complementary', { name: 'Application navigation' });
    await expect(sidebar.getByRole('link', { name: 'CMA home' })).toBeVisible();
    await expect(sidebar.getByRole('button', { name: 'New agent', exact: true })).toBeVisible();
    const geometry = await sidebar.evaluate((element) => {
      const rect = (selector: string) => element.querySelector(selector)!.getBoundingClientRect();
      const head = rect('.sidebar__head'),
        navigation = rect('.sidebar__navigation'),
        account = rect('.sidebar__account'),
        actions = rect('.sidebar__actions'),
        create = rect('.sidebar__new'),
        toggle = rect('.sidebar__toggle'),
        bounds = element.getBoundingClientRect();
      return {
        fills:
          navigation.height > 0 &&
          navigation.top >= actions.bottom &&
          navigation.bottom <= account.top &&
          account.top - navigation.bottom <= 12,
        ordered: actions.top >= head.bottom && account.bottom <= bounds.bottom,
        overlap:
          Math.min(create.right, toggle.right) > Math.max(create.left, toggle.left) &&
          Math.min(create.bottom, toggle.bottom) > Math.max(create.top, toggle.top),
        fits: [create, toggle, account, navigation].every(
          (box) => box.left >= bounds.left && box.right <= bounds.right + 1,
        ),
      };
    });
    expect(geometry).toEqual({ fills: true, ordered: true, overlap: false, fits: true });
    await info.attach('navigation-geometry', {
      body: JSON.stringify(geometry),
      contentType: 'application/json',
    });
    await sidebar.getByRole('link', { name: 'CMA home' }).click();
    await expect(page).toHaveURL(`${baseURL}/agent-manager`);
    await navigation(page, false);
    await expect(
      page.getByRole('heading', { name: /^Good (morning|afternoon|evening)\.$/ }),
    ).toBeVisible();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Get Started', exact: true })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath(`home-${width}.png`), fullPage: true });
    await page.goto('/');
    await expect(
      page.getByRole('heading', { name: /^Good (morning|afternoon|evening)\.$/ }),
    ).toBeVisible();
    noEffects(boundary);
  });
}
