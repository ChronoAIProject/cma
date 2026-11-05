import { readFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

const identity = JSON.parse(readFileSync('dist/build-info.json', 'utf8')) as {
  version: string;
  revision: string;
  local: boolean;
};
const expected = `CMA ${identity.version} (${identity.revision || 'local'})${identity.local && identity.revision ? ' (local)' : ''}`;

for (const clipboard of ['available', 'denied', 'absent'] as const) {
  test(`LOCAL compiled build identity with clipboard ${clipboard}`, async ({ page, baseURL }) => {
    const fixture = new LocalAgentV2();
    try {
      await fixture.route(page, baseURL!);
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
