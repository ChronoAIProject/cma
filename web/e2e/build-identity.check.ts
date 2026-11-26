import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

const am = { version: '0.22.34', source_sha: 'b'.repeat(40) };
const eg = { version: '0.11.16', source_sha: 'c'.repeat(40) };

for (const clipboard of ['available', 'denied', 'absent'] as const) {
  test(`Compiled build identity with clipboard ${clipboard}`, async ({
    page,
    baseURL,
    request,
  }) => {
    // Read the actual serving artifact, so an unrelated local dist cannot supply evidence.
    const response = await request.get('/build-info.json');
    expect(response.ok()).toBe(true);
    const identity = (await response.json()) as {
      version: string;
      revision: string;
      local: boolean;
    };
    expect(identity).toEqual({
      version: expect.any(String),
      revision: expect.any(String),
      local: expect.any(Boolean),
    });
    if (process.env.CMA_WEB_IMAGE_URL) {
      expect(identity.local).toBe(false);
      expect(process.env.CMA_EXPECTED_WEB_REVISION).toMatch(/^[0-9a-f]{40}$/);
      expect(identity.revision).toBe(process.env.CMA_EXPECTED_WEB_REVISION);
      expect(identity.version).toBe(process.env.CMA_EXPECTED_WEB_VERSION);
    }
    const web = `${identity.version} (${identity.revision || 'local'})${identity.local && identity.revision ? ' (local)' : ''}`;
    const expected = `Web ${web} · AM ${am.version} (${am.source_sha}) · EG ${eg.version} (${eg.source_sha})`;
    const fixture = new LocalAgentV2();
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v1/build-info', (route) => route.fulfill({ json: am }));
      await page.route('**/event-gateway/v1/readyz', (route) =>
        route.fulfill({ status: 503, json: { ready: false, build: eg } }),
      );
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(fixture.items.length),
            active_turn: null,
          },
        }),
      );
      await page.addInitScript((mode) => {
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value:
            mode === 'absent'
              ? undefined
              : {
                  writeText: async (value: string) => {
                    if (mode === 'denied') throw new Error('denied');
                    document.documentElement.dataset.copiedBuild = value;
                  },
                },
        });
      }, clipboard);
      await page.goto('/agents/agt_contract');
      const draft = page.getByRole('textbox', { name: 'Message', exact: true });
      await draft.fill('Unsent local fixture draft');
      await page.getByRole('button', { name: /Account menu for/ }).click();
      const copy = page.getByRole('menuitem', { name: /Copy build version/ });
      const webLabel = identity.version + (identity.local ? ' (local)' : '');
      await expect(copy).toHaveAccessibleName(
        `Copy build version · Web ${webLabel} · AM ${am.version} · EG ${eg.version}`,
      );
      await expect(copy).toBeEnabled();
      await copy.focus();
      await page.keyboard.press('Enter');
      if (clipboard === 'available') {
        await expect(page.getByRole('menuitem', { name: 'Build version copied' })).toBeVisible();
        expect(await page.locator('html').getAttribute('data-copied-build')).toBe(expected);
      } else {
        await expect(copy).toContainText('Could not copy');
        await expect(copy.locator('code')).toHaveText(expected);
        expect(await page.evaluate(() => window.getSelection()?.toString())).toBe(expected);
        await expect(copy).toBeEnabled();
      }
      await page.keyboard.press('Escape');
      await expect(draft).toHaveValue('Unsent local fixture draft');
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
