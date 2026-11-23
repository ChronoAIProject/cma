import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalSignIn } from './auth-return-target.fixture';

for (const width of [1440, 390]) {
  for (const [opened, restored] of [
    ['/docs#sign-in-landing-how', '/docs'],
    ['/docs/event-gateway', '/docs/event-gateway'],
    ['/usage', '/usage'],
    ['/operations', '/agent-manager'],
  ]) {
    test(`LOCAL explicit sign-in returns from ${opened} at ${width}px`, async ({
      page,
      baseURL,
    }) => {
      const boundary = new LocalStartup();
      const metrics = new LocalUsageMetrics();
      const auth = new LocalSignIn();
      try {
        await boundary.route(page, baseURL!);
        await metrics.route(page);
        await auth.route(page, baseURL!);
        await page.setViewportSize({ width, height: 900 });
        await page.goto(opened!);
        const start = page.getByRole('button', { name: 'Get Started', exact: true });
        await expect(start).toBeVisible();
        expect(auth.targets).toEqual([]);
        await start.click();
        await expect(page).toHaveURL(`${auth.origin}/provider`);
        expect(auth.targets).toEqual([restored]);
        await page.getByRole('link', { name: 'Complete local sign-in' }).click();
        await expect(page).toHaveURL(`${baseURL}${restored}`);
        const destination =
          restored === '/usage'
            ? page.getByRole('dialog', { name: 'Your usage' })
            : page.getByRole('heading', {
                name:
                  restored === '/docs'
                    ? 'Docs'
                    : restored === '/docs/event-gateway'
                      ? 'Event Gateway Protocol'
                      : /^Good (morning|afternoon|evening)\.$/,
                exact: true,
              });
        await expect(destination).toBeVisible();
        await page.reload();
        await expect(destination).toBeVisible();
        await expect(page).toHaveURL(`${baseURL}${restored}`);
        expect(auth.targets).toEqual([restored]);
        expect(auth.callbacks).toBe(1);
        expect(auth.errors).toEqual([]);
        expect(boundary.writes).toEqual([]);
        expect(boundary.unexpected).toEqual([]);
        expect(boundary.errors).toEqual([]);
      } finally {
        await auth.close();
      }
    });
  }
}
