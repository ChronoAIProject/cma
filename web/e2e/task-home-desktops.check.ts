import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const width of [1440, 320]) {
  test(`LOCAL ${width}px Home preserves independent desktop choices and original recovery`, async ({
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
                  desktop: { type: 'boolean' },
                  machine_included: { type: 'boolean' },
                },
              },
            },
          },
        },
      }),
    );
    await page.route('**/desktop-capability', (route) =>
      route.fulfill({
        json: {
          state: 'available',
          reason: 'verified_runtime',
          runtime_contract: 3,
          min_memory_mib: 2048,
          min_cpu_millis: 1000,
        },
      }),
    );
    const writes: { body: unknown; key?: string }[] = [];
    await page.route('**/api/agent-manager/v1/agents', async (route) => {
      const request = route.request();
      writes.push({ body: request.postDataJSON(), key: request.headers()['idempotency-key'] });
      await route.abort('failed');
    });
    await page.setViewportSize({ width, height: 1000 });
    await page.goto('/');
    await page.getByText('Desktop options', { exact: true }).click();
    const machine = page.getByRole('checkbox', { name: 'Include a shared desktop' });
    const browser = page.getByRole('checkbox', { name: 'Enable desktop' });
    await expect(machine).toBeEnabled();
    await expect(browser).toBeEnabled();
    await expect(machine).not.toBeChecked();
    await expect(browser).not.toBeChecked();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    const task = '  Keep the original task\nand both desktop choices  ';
    await page.getByLabel('Your task').fill(task);
    await machine.check();
    await browser.check();
    expect(writes).toHaveLength(0);
    for (const checkbox of [machine, browser]) {
      const bounds = await checkbox.locator('..').boundingBox();
      expect(bounds!.height).toBeGreaterThanOrEqual(44);
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    }
    await page.screenshot({ path: info.outputPath('home-desktop-options.png'), fullPage: true });
    await page.reload();
    await page.getByText('Desktop selected', { exact: true }).click();
    await expect(machine).toBeChecked();
    await expect(browser).toBeChecked();
    await expect(page.getByLabel('Your task')).toHaveValue(task);
    await expect(browser).toBeEnabled();
    await page.getByRole('button', { name: 'Start task', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Recover original task' })).toBeEnabled();
    await page.reload();
    await page.getByText('Desktop selected', { exact: true }).click();
    await expect(machine).toBeChecked();
    await expect(machine).toBeDisabled();
    await expect(browser).toBeChecked();
    await expect(browser).toBeDisabled();
    expect(writes).toHaveLength(1);
    await page.getByRole('button', { name: 'Recover original task' }).click();
    await expect.poll(() => writes.length).toBe(2);
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]).toEqual({
      body: {
        name: 'New task',
        initial_message: task,
        machine_included: true,
        desktop: true,
      },
      key: expect.any(String),
    });
    await page.screenshot({ path: info.outputPath('home-recovery-frozen.png'), fullPage: true });
    expect(boundary.writes).toEqual([]);
  });
}
