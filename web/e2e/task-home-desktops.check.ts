import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const width of [1440, 320]) {
  test(`LOCAL ${width}px Home preserves shared desktop choice and original recovery`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.route('**/api/v2/ui/task-experience', (route) =>
      route.fulfill({
        json: {
          object: 'cma.ui.task_experience',
          schema_version: 1,
          account_id: 'local-owner',
          capabilities: {
            task_home_admission: true,
            desktop_enable: true,
            suggested_followup_admission: false,
            browser_takeover: false,
            replay_presentation: false,
          },
        },
      }),
    );
    await page.route('**/openapi.json', (route) =>
      route.fulfill({
        json: {
          openapi: '3.1.0',
          info: { title: 'CMA', version: '1' },
          paths: {},
          components: {
            schemas: {
              AgentLifecycleCreateRequest: {
                properties: {
                  initial_message: { type: 'string' },
                  machine_included: { type: 'boolean' },
                },
              },
            },
          },
        },
      }),
    );
    const capabilityReads: string[] = [];
    page.on('request', (request) => {
      if (request.url().includes('/desktop-capability')) capabilityReads.push(request.url());
    });
    const writes: { body: unknown; key?: string }[] = [];
    await page.route('**/api/agent-manager/v1/agents', async (route) => {
      const request = route.request();
      writes.push({ body: request.postDataJSON(), key: request.headers()['idempotency-key'] });
      await route.abort('failed');
    });
    await page.setViewportSize({ width, height: 1000 });
    await page.goto('/');
    await page.getByRole('button', { name: /^Cloud computer/ }).click();
    const machine = page.getByRole('menuitemcheckbox', { name: 'Shared desktop' });
    const browser = page.getByRole('menuitemcheckbox', { name: 'Agent browser' });
    await expect(machine).toBeEnabled();
    await expect(browser).toHaveCount(0);
    await expect(machine).not.toBeChecked();

    await expect(page.getByRole('dialog')).toHaveCount(0);
    const task = '  Keep the original task\nand shared desktop choice  ';
    await machine.click();

    expect(writes).toHaveLength(0);
    for (const checkbox of [machine]) {
      const bounds = await checkbox.boundingBox();
      expect(bounds!.height).toBeGreaterThanOrEqual(44);
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    }
    await page.screenshot({ path: info.outputPath('home-desktop-options.png'), fullPage: true });
    await page.keyboard.press('Escape');
    await page.getByLabel('Your task').fill(task);
    await page.reload();
    await page.getByRole('button', { name: /^Cloud computer/ }).click();
    await expect(machine).toBeChecked();
    await expect(browser).toHaveCount(0);
    await expect(page.getByLabel('Your task')).toHaveValue(task);
    await expect(browser).toHaveCount(0);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Start task', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Recover original task' })).toBeEnabled();
    await page.reload();
    await page.getByRole('button', { name: /^Cloud computer/ }).click();
    await expect(machine).toBeChecked();
    await expect(machine).toBeDisabled();
    await expect(browser).toHaveCount(0);

    expect(writes).toHaveLength(1);
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Recover original task' }).click();
    await expect.poll(() => writes.length).toBe(2);
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]).toEqual({
      body: {
        name: 'New task',
        initial_message: task,
        machine_included: true,
      },
      key: expect.any(String),
    });
    await page.screenshot({ path: info.outputPath('home-recovery-frozen.png'), fullPage: true });
    expect(boundary.writes).toEqual([]);
    expect(capabilityReads).toEqual([]);
  });
}

for (const width of [1440, 390]) {
  test(`LOCAL ${width}px pending checks and refocus do not flash or move the composer`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    const policyReads: ((admitted: boolean) => Promise<void>)[] = [];
    await page.route('**/api/v2/ui/task-experience', (route) => {
      policyReads.push((admitted) =>
        route.fulfill({
          json: {
            object: 'cma.ui.task_experience',
            schema_version: 1,
            account_id: 'local-owner',
            capabilities: {
              task_home_admission: admitted,
              desktop_enable: false,
              suggested_followup_admission: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
    });
    const supportReads: (() => Promise<void>)[] = [];
    await page.route('**/openapi.json', (route) => {
      supportReads.push(() =>
        route.fulfill({
          json: {
            openapi: '3.1.0',
            info: { title: 'CMA', version: '1' },
            paths: {},
            'x-cma-task-repository': true,
            components: {
              schemas: {
                AgentLifecycleCreateRequest: {
                  properties: { initial_message: { type: 'string' } },
                },
              },
            },
          },
        }),
      );
    });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/');
    const home = page.locator('[data-task-home]');
    const fallback = home.getByText('Task creation is unavailable.', { exact: false });
    const manual = home.getByRole('button', { name: 'New agent', exact: true });
    await expect(home.getByLabel('Your task')).toBeVisible();
    await page.evaluate(() => document.fonts.ready);
    await expect
      .poll(() =>
        page.locator('.frame__center').evaluate((node) => node.getBoundingClientRect().right),
      )
      .toBe(width);
    await expect.poll(() => policyReads.length).toBeGreaterThan(0);
    await expect.poll(() => supportReads.length).toBeGreaterThan(0);
    await home.getByLabel('Your task').fill('A stable landing');
    await expect(fallback).toHaveCount(0);
    await expect(manual).toHaveCount(0);
    await expect(home.getByRole('button', { name: 'Start task' })).toBeDisabled();
    for (const text of ['Checking task creation…', 'Checking repository support…']) {
      const status = home.getByText(text, { exact: true });
      await expect(status).toHaveClass('sr-only');
      expect((await status.boundingBox())!.height).toBeLessThanOrEqual(1);
    }
    const composer = home.locator('.task-home__composer');
    const before = await composer.boundingBox();
    await Promise.all(policyReads.map((resolve) => resolve(true)));
    await expect(home.getByRole('button', { name: 'Start task' })).toBeDisabled();
    await Promise.all(supportReads.map((resolve) => resolve()));
    await expect(home.getByRole('button', { name: 'Start task' })).toBeEnabled();
    await expect(fallback).toHaveCount(0);
    await expect(manual).toHaveCount(0);
    expect(await composer.boundingBox()).toEqual(before);
    for (const event of ['focus', 'visibilitychange']) {
      const count = policyReads.length;
      await page.evaluate((name) => {
        (name === 'focus' ? window : document).dispatchEvent(new Event(name));
      }, event);
      await expect.poll(() => policyReads.length).toBeGreaterThan(count);
      await expect(fallback).toHaveCount(0);
      await expect(manual).toHaveCount(0);
      await expect(home.getByRole('button', { name: 'Start task' })).toBeEnabled();
      expect(await composer.boundingBox()).toEqual(before);
      await policyReads.at(-1)!(event === 'focus');
    }
    await expect(fallback).toBeVisible();
    await expect(manual).toBeVisible();
    await expect(home.getByRole('button', { name: 'Start task' })).toBeDisabled();
    expect(boundary.writes).toEqual([]);
  });
}
