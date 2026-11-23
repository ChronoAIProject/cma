import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const target of ['landing', 'first-visit'] as const)
  for (const theme of ['light', 'dark'] as const)
    test(`LOCAL default-off production ${target} ${theme}`, async ({ page, baseURL }, info) => {
      const boundary = new LocalStartup();
      const signIns: string[] = [];
      const rendererRequests: string[] = [];
      const failures: string[] = [];
      await boundary.route(page, baseURL!);
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.emulateMedia({ reducedMotion: 'no-preference', colorScheme: theme });
      await page.addInitScript((appearance) => {
        localStorage.setItem('cma:appearance', appearance);
        const state = { contexts: 0, backdrops: 0 };
        Object.defineProperty(window, '__defaultOffProbe', { value: state });
        const getContext = HTMLCanvasElement.prototype.getContext;
        HTMLCanvasElement.prototype.getContext = function (
          this: HTMLCanvasElement,
          kind: string,
          ...args: unknown[]
        ) {
          if (this.closest('.topographic-backdrop')) state.contexts++;
          return Reflect.apply(getContext, this, [kind, ...args]);
        };
        new MutationObserver((records) => {
          for (const record of records)
            for (const node of record.addedNodes)
              if (
                node instanceof Element &&
                (node.matches('.topographic-backdrop') ||
                  node.querySelector('.topographic-backdrop'))
              )
                state.backdrops++;
        }).observe(document, { childList: true, subtree: true });
        addEventListener('unhandledrejection', () => {
          document.documentElement.dataset.defaultOffRejection = 'true';
        });
      }, theme);
      page.on('request', (request) => {
        if (/topographic-surface[.-]/.test(request.url()))
          rendererRequests.push(new URL(request.url()).pathname);
      });
      page.on('pageerror', () => failures.push('pageerror'));
      if (target === 'landing') {
        await page.route('**/auth/session', (route) =>
          route.fulfill({ json: { authenticated: false, can_silent_sso: false } }),
        );
        await page.route('**/auth/login**', (route) => {
          signIns.push(new URL(route.request().url()).searchParams.get('return_to') ?? '');
          return route.fulfill({
            contentType: 'text/html',
            body: '<p>Local sign-in destination</p>',
          });
        });
      } else {
        await page.route(
          (url) => url.pathname === '/api/v1/sandboxes',
          (route) => route.fulfill({ json: { items: [] } }),
        );
      }
      await page.route('**/api/v2/ui/task-experience', (route) =>
        route.fulfill({
          json: {
            object: 'cma.ui.task_experience',
            schema_version: 1,
            account_id: 'local-owner',
            capabilities: {
              task_home_admission: true,
              desktop_enable: false,
              suggested_followup_admission: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
      await page.goto('/');
      const action = page.getByRole('button', {
        name: target === 'landing' ? 'Get Started' : 'Start task',
        exact: true,
      });
      await expect(action).toBeVisible();
      if (target === 'first-visit')
        await expect(page.getByRole('heading', { name: 'Give it a job.' })).toBeVisible();
      await expect(page.locator('.topographic-backdrop')).toHaveCount(0);
      // Observe beyond the enabled effect's full four-second playback window.
      await page.waitForTimeout(4500);
      const probe = await page.evaluate(() => {
        const observed = window as unknown as {
          __defaultOffProbe: { contexts: number; backdrops: number };
        };
        return {
          ...observed.__defaultOffProbe,
          rejected: document.documentElement.dataset.defaultOffRejection,
        };
      });
      expect(probe).toEqual({ contexts: 0, backdrops: 0, rejected: undefined });
      expect(rendererRequests).toEqual([]);
      expect(failures).toEqual([]);
      await page.screenshot({ path: info.outputPath(`${target}-${theme}-default-off.png`) });
      await info.attach('default-off', {
        body: JSON.stringify({
          source: process.env.CMA_SOURCE_SHA,
          flag: 'unset',
          target,
          theme,
          probe,
        }),
        contentType: 'application/json',
      });
      if (target === 'landing') {
        await action.click();
        await expect.poll(() => signIns.length).toBe(1);
      } else {
        await page
          .getByRole('textbox', { name: 'Your task', exact: true })
          .fill('Keep the default Home flow.');
        await expect(action).toBeEnabled();
      }
      expect(boundary.writes).toEqual([]);
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
    });
