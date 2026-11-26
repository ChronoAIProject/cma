import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { insideViewport, noEffects } from './resource-lifecycle.fixture';

for (const width of [1440, 390]) {
  test(`LOCAL named sandbox sections and technical details at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_contract/skills', (route) => {
      expect(route.request().method()).toBe('GET');
      return route.fulfill({
        json: {
          items: [
            {
              id: 'skill-fixture',
              name: 'TypeScript',
              source: 'ornn',
              version: '1.2',
              status: 'installed',
            },
          ],
          pending: false,
          can_install: true,
        },
      });
    });
    await page.route('**/api/v1/profile-authoring/skills*', (route) => {
      expect(route.request().method()).toBe('GET');
      return route.fulfill({
        json: { items: [{ id: 'skill-fixture', name: 'TypeScript', versions: ['1.2'] }] },
      });
    });
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    const main = page.locator('.sandbox-page');
    await expect(main.getByRole('tab', { name: 'Overview', exact: true })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(main.getByText('Model access', { exact: true })).toBeVisible();
    await expect(main.getByText('Chrono LLM (public)', { exact: true })).toBeVisible();
    await main.getByRole('tab', { name: 'Workspaces', exact: true }).click();
    const row = main
      .getByRole('row')
      .filter({ has: page.locator(`a[href="/workspaces/${boundary.workspace.id}"]`) });
    await expect(row.getByText('Guarded', { exact: true })).toBeVisible();
    await expect(row.getByText('gpt-proof', { exact: true })).toBeVisible();
    await expect(row.locator(`a[href="/agents/${boundary.agent.id}"]`)).toBeVisible();
    await row
      .getByRole('button', { name: 'New agent in project', exact: true })
      .click({ trial: true });
    await main.getByRole('tab', { name: 'Access', exact: true }).click();
    await expect(main.getByRole('link', { name: 'GitHub', exact: true })).toHaveAttribute(
      'href',
      'https://catalog.example.test/services/catalog-github',
    );
    await expect(main.getByText('Workspace limit', { exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
    await page.screenshot({
      path: info.outputPath(`sandbox-sections-${width}.png`),
      fullPage: true,
    });
    const more = page.getByRole('button', { name: 'More actions', exact: true });
    await more.click();
    await page.getByRole('menuitem', { name: 'Technical details', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Technical details', exact: true });
    await expect(dialog.getByText('Workspace limit', { exact: true })).toBeVisible();
    await insideViewport(dialog);
    for (const dark of [false, true]) {
      await page.evaluate(
        (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
        dark,
      );
      await dialog.getByRole('button', { name: 'Close', exact: true }).click({ trial: true });
      await page.screenshot({
        path: info.outputPath(`technical-${width}-${dark}.png`),
        fullPage: true,
      });
    }
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await expect(more).toBeFocused();
    await page.goto(`/agents/${boundary.agent.id}`);
    await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
    const skills = page.getByRole('region', { name: 'Agent skills', exact: true });
    await expect(skills.getByText('TypeScript', { exact: true })).toBeVisible();
    await expect(skills.getByRole('list', { name: 'Agent skill status' })).toContainText(
      '1.2 · Installed',
    );
    await page.getByRole('button', { name: 'Close Agent details', exact: true }).click();
    noEffects(boundary);
  });
}

const runtimeVersion = `registry.example/runtime@sha256:${'a'.repeat(64)}`;
const nextRuntime = `registry.example/runtime@sha256:${'b'.repeat(64)}`;
const lifecycleCases = [
  ['latest', 'Runtime up to date', 'done'],
  ['unknown', 'Runtime version unconfirmed', 'neutral'],
  ['mismatch', 'Runtime version unconfirmed', 'neutral'],
  ['retiring', 'Environment retiring', 'neutral'],
  ['retired', 'Environment retired', 'neutral'],
  ['read-failure', 'Runtime status unavailable', 'warning'],
  ['unconfirmed', 'Runtime update unconfirmed', 'warning'],
] as const;

for (const width of [1440, 390]) {
  for (const [state, title, tone] of lifecycleCases) {
    test(`LOCAL environment lifecycle ${state} at ${width}px`, async ({ page, baseURL }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      if (state === 'read-failure') {
        await page.clock.install({ time: new Date('2026-11-23T00:00:00Z') });
        await page.clock.pauseAt(new Date('2026-11-23T00:00:01Z'));
      }
      await page.addInitScript(() => {
        Object.defineProperty(navigator, 'clipboard', {
          value: { writeText: () => Promise.reject(new Error('Clipboard denied')) },
          configurable: true,
        });
      });
      const boundary = new LocalStartup();
      boundary.sandbox = {
        ...boundary.sandbox,
        status: state === 'retired' || state === 'retiring' ? state : 'active',
        runtime_version: runtimeVersion,
        runtime_update: {
          current_version: state === 'unknown' ? null : runtimeVersion,
          latest_version:
            state === 'mismatch' || state === 'unconfirmed' ? nextRuntime : runtimeVersion,
          state: state === 'unconfirmed' ? 'available' : 'up_to_date',
          update_available: state === 'unconfirmed',
          can_update: state === 'unconfirmed',
          preserves_home: true,
          schedule: { timezone: 'Asia/Singapore', local_time: '02:00' },
        },
      };
      await boundary.route(page, baseURL!);
      let failedRead = false;
      await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, (route) => {
        expect(route.request().method()).toBe('GET');
        return route.fulfill(
          failedRead
            ? {
                status: 503,
                json: { code: 'unavailable', detail: 'Fixture status unavailable', status: 503 },
              }
            : { json: boundary.sandbox },
        );
      });
      const commands: { body: unknown; key: string | undefined }[] = [];
      await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}/runtime-update`, (route) => {
        expect(route.request().method()).toBe('POST');
        commands.push({
          body: route.request().postDataJSON(),
          key: route.request().headers()['idempotency-key'],
        });
        return route.abort('connectionfailed');
      });
      await page.goto(`/sandboxes/${boundary.sandbox.id}`);
      const main = page.locator('.sandbox-page');
      const runtime = main.getByRole('region', { name: 'Runtime', exact: true });
      await expect(runtime).toBeVisible();
      if (state === 'read-failure') {
        await expect(runtime.getByRole('status')).toHaveText('Runtime up to date');
        failedRead = true;
        await page.clock.runFor(5001);
      }
      if (state === 'unconfirmed') {
        await runtime.getByRole('button', { name: 'Update runtime', exact: true }).click();
        await expect(runtime.getByRole('alert')).toContainText(
          'The update request is not yet confirmed.',
        );
        boundary.sandbox.runtime_update = {
          ...boundary.sandbox.runtime_update!,
          latest_version: 'registry.example/runtime:newer',
        };
        await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
        await expect(
          runtime.getByText('registry.example/runtime:newer', { exact: true }),
        ).toHaveCount(1);
      }
      await expect(runtime.locator('.sandbox-recovery__title')).toHaveText(title);
      await expect(runtime.locator('.statedot')).toHaveAttribute('data-state', tone);
      if (state !== 'latest') {
        await expect(runtime.getByText('Runtime up to date', { exact: true })).toHaveCount(0);
        await expect(runtime.getByText('Runtime updated', { exact: true })).toHaveCount(0);
      }
      if (state === 'retired' || state === 'retiring') {
        await expect(
          runtime.getByText('Your existing home disk, files and conversations are preserved.', {
            exact: true,
          }),
        ).toHaveCount(0);
        await expect(
          main.getByText('Resources are available when this Sandbox has an Agent.', {
            exact: true,
          }),
        ).toHaveCount(0);
      }
      if (state === 'retired') await expect(runtime).toContainText('no longer running');
      const summary = runtime.locator('summary');
      const identity = runtime.getByText(boundary.sandbox.id, { exact: true });
      await expect(identity).toBeHidden();
      for (const theme of ['light', 'dark']) {
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await main.evaluate((element) => element.scrollTo({ top: 0 }));
        await page.screenshot({
          path: info.outputPath(`${state}-${width}-${theme}.png`),
          fullPage: true,
        });
        await summary.focus();
        await page.keyboard.press('Shift+Tab');
        await page.keyboard.press('Tab');
        await expect(summary).toBeFocused();
        expect(await summary.evaluate((element) => element.matches(':focus-visible'))).toBe(true);
        expect(await summary.evaluate((element) => getComputedStyle(element).boxShadow)).not.toBe(
          'none',
        );
        expect((await summary.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        await page.keyboard.press('Enter');
        await expect(identity).toBeVisible();
        const copy = runtime.getByRole('button', { name: 'Copy technical details', exact: true });
        expect((await copy.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        await copy.click();
        const fallback = runtime.getByRole('textbox', { name: 'Technical details to copy' });
        await expect(fallback).toBeFocused();
        await expect(fallback).toHaveAttribute('readonly', '');
        expect(
          await fallback.evaluate(
            (element: HTMLTextAreaElement) => element.selectionEnd - element.selectionStart,
          ),
        ).toBe((await fallback.inputValue()).length);
        expect(await fallback.inputValue()).toContain(`Sandbox ID: ${boundary.sandbox.id}`);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await page.screenshot({
          path: info.outputPath(`${state}-details-${width}-${theme}.png`),
          fullPage: true,
        });
        await summary.focus();
        await page.keyboard.press('Enter');
        await expect(identity).toBeHidden();
      }
      if (state === 'unconfirmed') {
        expect(commands).toHaveLength(1);
        expect(commands[0]!.body).toEqual({ target_version: nextRuntime });
        expect(commands[0]!.key).toBeTruthy();
        await runtime.getByRole('button', { name: 'Retry update request', exact: true }).click();
        await expect(runtime.getByRole('alert')).toContainText(
          'The update request is not yet confirmed.',
        );
        expect(commands).toHaveLength(2);
        expect(commands[1]).toEqual(commands[0]);
      } else expect(commands).toEqual([]);
      noEffects(boundary);
    });
  }
}

for (const viewport of [
  { width: 320, height: 900 },
  { width: 360, height: 900 },
  { width: 720, height: 450 },
]) {
  test(`LOCAL environment details reflow at ${viewport.width}x${viewport.height}`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize(viewport);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const boundary = new LocalStartup();
    boundary.sandbox.runtime_update = {
      current_version: runtimeVersion,
      latest_version: runtimeVersion,
      state: 'up_to_date',
      update_available: false,
      can_update: false,
      preserves_home: true,
      schedule: { timezone: 'Asia/Singapore', local_time: '02:00' },
    };
    await boundary.route(page, baseURL!);
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    const runtime = page.getByRole('region', { name: 'Runtime', exact: true });
    const summary = runtime.locator('summary');
    await summary.focus();
    await page.keyboard.press('Enter');
    for (const theme of ['light', 'dark']) {
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      await runtime.getByRole('button', { name: 'Copy technical details' }).click({ trial: true });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      expect(await runtime.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
        true,
      );
      await page.screenshot({
        path: info.outputPath(`details-reflow-${viewport.width}-${theme}.png`),
        fullPage: true,
      });
    }
    // 720x450 is the layout viewport equivalent of 1440x900 at 200%, not native zoom.
    noEffects(boundary);
  });
}

for (const width of [1440, 390]) {
  for (const recovery of ['manual', 'online'] as const) {
    test(`LOCAL Environment access recovers from 503 by ${recovery} at ${width}px`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      boundary.sandbox.status = 'retired';
      await boundary.route(page, baseURL!);
      let unavailable = true;
      const observations: string[] = [];
      await page.route(
        /\/api\/v1\/(organizations|sandboxes|resource-access\/sandbox\/sbx_route)$/,
        (route) => {
          const path = new URL(route.request().url()).pathname;
          expect(route.request().method()).toBe('GET');
          observations.push(path);
          return unavailable
            ? route.fulfill({
                status: 503,
                json: { status: 503, detail: 'Fixture status unavailable' },
              })
            : route.fallback();
        },
      );
      await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, (route) => {
        expect(route.request().method()).toBe('GET');
        observations.push(new URL(route.request().url()).pathname);
        return route.fallback();
      });
      await page.goto(`/sandboxes/${boundary.sandbox.id}`);
      const heading = page.getByRole('heading', { name: 'Sandbox unavailable', exact: true });
      await expect(heading).toBeVisible();
      const result = await page.evaluate(
        async (id) => (await fetch(`/api/v1/sandboxes/${id}`)).json(),
        boundary.sandbox.id,
      );
      expect(result.status).toBe('retired');
      await expect(heading).toBeVisible();
      await expect(
        page.getByRole('button', { name: 'Retry environment check', exact: true }),
      ).toBeVisible();
      for (const theme of ['light', 'dark']) {
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        await page.screenshot({
          path: info.outputPath(`access-503-${recovery}-${width}-${theme}.png`),
          fullPage: true,
        });
      }
      unavailable = false;
      const before = observations.length;
      if (recovery === 'manual')
        await page.getByRole('button', { name: 'Retry environment check', exact: true }).click();
      else await page.evaluate(() => window.dispatchEvent(new Event('online')));
      await expect(
        page
          .locator('.sandbox-page')
          .getByRole('heading', { name: boundary.sandbox.name, exact: true }),
      ).toBeVisible();
      await expect(
        page.locator('.sandbox-page').getByText('Deleted', { exact: true }),
      ).toBeVisible();
      await expect(heading).toHaveCount(0);
      const reads = observations.slice(before);
      const access = `/api/v1/resource-access/sandbox/${boundary.sandbox.id}`;
      const detail = `/api/v1/sandboxes/${boundary.sandbox.id}`;
      expect(reads.filter((path) => path === access).length).toBeGreaterThanOrEqual(2);
      expect(reads.indexOf(detail)).toBeGreaterThan(reads.indexOf(access));
      expect(reads.lastIndexOf(access)).toBeGreaterThan(reads.indexOf(detail));
      for (const theme of ['light', 'dark']) {
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        await page.screenshot({
          path: info.outputPath(`access-recovered-${recovery}-${width}-${theme}.png`),
          fullPage: true,
        });
      }
      noEffects(boundary);
    });
  }
}
