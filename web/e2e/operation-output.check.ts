import { test, expect } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { LocalAgentV2 } from './agent-v2.fixture';

test('native operations survive real public history, complete scrollback and authorized UI reload without extra execution', async ({
  page,
  baseURL,
}, info) => {
  const addressFile = process.env.CMA_REPLAY_ADDRESS_FILE;
  const release = process.env.CMA_REPLAY_BROWSER_RELEASE;
  if (!addressFile || !release)
    throw new Error('The running pinned native/public proof is required');
  const origin = readFileSync(addressFile, 'utf8');
  const fixture = new LocalAgentV2();
  fixture.enabled = true;
  const writes: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/api/') && request.method() !== 'GET')
      writes.push(`${request.method()} ${request.url()}`);
  });
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/ui/task-experience', (route) =>
      route.fulfill({
        json: {
          object: 'cma.ui.task_experience',
          schema_version: 1,
          account_id: 'local-owner',
          capabilities: {
            task_home_admission: false,
            suggested_followup_admission: false,
            desktop_enable: false,
            browser_takeover: false,
            replay_presentation: true,
          },
        },
      }),
    );
    await page.route(
      /\/api\/v2\/agents\/agt_contract\/events\/(snapshot|head)(\?|$)/,
      async (route) => {
        const url = new URL(route.request().url());
        const response = await route.fetch({ url: `${origin}${url.pathname}${url.search}` });
        expect(response.status()).toBe(200);
        await route.fulfill({ response });
      },
    );
    await page.route('**/api/v1/agents/agt_contract/environment', (route) => {
      const recorded = {
        state: 'available',
        reason: 'operation_records_only',
        scope: 'agent_turn_history',
        writable: false,
      };
      const unsupported = {
        state: 'unsupported',
        reason: 'not_supported_by_contract',
        scope: 'none',
        writable: false,
      };
      return route.fulfill({
        json: {
          agent_id: 'agt_contract',
          policy_revision: 1,
          runtime_incarnation: null,
          revalidate_at: null,
          files: { ...recorded, scope: 'mounted_home' },
          operation_output: recorded,
          replay: recorded,
          browser: unsupported,
          control: unsupported,
        },
      });
    });
    await page.goto('/agents/agt_contract');
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Terminal' }).click();
    const output = computer.locator('#computer-page-activity');
    for (const number of [1, 2, 3, 4])
      await expect(output.getByText(`native-replay-${number}`, { exact: true })).toBeVisible();
    await expect(output.getByRole('slider')).toHaveCount(0);
    await expect(
      output.getByRole('button', { name: /Previous operation|Return Live/ }),
    ).toHaveCount(0);
    await computer.getByRole('button', { name: 'Close computer' }).click();
    await page.getByRole('button', { name: /^Ran 4 commands/ }).click();
    const jump = page.getByRole('button', { name: 'View in Terminal', exact: true }).last();
    await jump.click();
    await expect(computer.getByRole('tab', { name: 'Terminal' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    const selected = output.locator('[data-command-id]').last();
    await expect(selected).toBeFocused();
    await expect
      .poll(async () => (await selected.boundingBox())!.y - (await output.boundingBox())!.y)
      .toBeCloseTo(16, 0);
    await expect(selected).toHaveAttribute('data-highlighted', 'true');
    await expect(selected).not.toHaveAttribute('data-highlighted', { timeout: 2500 });
    await page.screenshot({ path: info.outputPath('native-operation-output-desktop.png') });
    await page.reload();
    await computer.getByRole('tab', { name: 'Terminal' }).click();
    for (const number of [1, 2, 3, 4])
      await expect(output.getByText(`native-replay-${number}`, { exact: true })).toBeVisible();
    await expect(
      output.locator('input, textarea, iframe, canvas, [contenteditable=true]'),
    ).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await output.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
      true,
    );
    await expect
      .poll(async () => {
        const box = (await computer.boundingBox())!;
        return box.x + box.width;
      })
      .toBeLessThanOrEqual(390);
    await page.screenshot({ path: info.outputPath('native-operation-output-mobile.png') });
    expect(writes).toEqual([]);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    writeFileSync(
      release,
      JSON.stringify({
        observed: [1, 2, 3, 4],
        reload_preserved_scrollback: true,
        mutation_requests: writes,
      }),
    );
  } finally {
    await fixture.close(page);
  }
});
