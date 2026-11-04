import { test, expect } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { LocalAgentV2 } from './agent-v2.fixture';

test('native operations survive real public history, keyboard replay and authorized UI reload without extra execution', async ({
  page,
  baseURL,
}, info) => {
  const addressFile = process.env.CMA_REPLAY_ADDRESS_FILE;
  const release = process.env.CMA_REPLAY_BROWSER_RELEASE;
  if (!addressFile || !release)
    throw new Error('The running pinned native/public proof is required');
  const origin = readFileSync(addressFile, 'utf8');
  const fixture = new LocalAgentV2();
  fixture.enabled = false;
  const writes: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/api/') && request.method() !== 'GET')
      writes.push(`${request.method()} ${request.url()}`);
  });
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fixture.route(page, baseURL!);
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
    await computer.getByRole('tab', { name: 'Operation output' }).click();
    const replay = computer.getByRole('region', { name: 'Operation record replay' });
    await expect(replay.locator('pre')).toHaveText('native-replay-4\n');
    await replay.getByRole('button', { name: 'Previous operation' }).click();
    await expect(replay.locator('pre')).toHaveText('native-replay-3\n');
    await replay.getByRole('button', { name: 'Previous operation' }).press('ArrowLeft');
    await expect(replay.locator('pre')).toHaveText('native-replay-2\n');
    const cursor = await replay.getByText(/^Cursor /).innerText();
    await page.screenshot({ path: info.outputPath('native-operation-2-desktop.png') });
    await page.reload();
    await computer.getByRole('tab', { name: 'Operation output' }).click();
    await expect(replay.locator('pre')).toHaveText('native-replay-2\n');
    await expect(replay.getByText(cursor, { exact: true })).toBeVisible();
    await expect(replay.getByText('Turn 3', { exact: true })).toBeVisible();
    await expect(
      replay.locator('input, textarea, iframe, canvas, [contenteditable=true]'),
    ).toHaveCount(0);
    await replay.getByRole('button', { name: 'Return Live' }).press('End');
    await expect(replay.locator('pre')).toHaveText('native-replay-4\n');
    await page.setViewportSize({ width: 390, height: 844 });
    await replay.getByRole('button', { name: 'Previous operation' }).click();
    await expect(replay.locator('pre')).toHaveText('native-replay-3\n');
    expect(await replay.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
      true,
    );
    await expect
      .poll(async () => {
        const box = (await computer.boundingBox())!;
        return box.x + box.width;
      })
      .toBeLessThanOrEqual(390);
    await page.screenshot({ path: info.outputPath('native-operation-3-mobile.png') });
    expect(writes).toEqual([]);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    writeFileSync(
      release,
      JSON.stringify({ cursor, replayed: [4, 3, 2, 2, 4, 3], mutation_requests: writes }),
    );
  } finally {
    await fixture.close(page);
  }
});
