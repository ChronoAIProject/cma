import { expect, test, type Page } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

const names = [
  'Synthetic research environment with a descriptive name '.repeat(4),
  'synthetic_environment_with_a_very_long_unbroken_name_'.repeat(5),
  '合成環境の長い名前 — Synthetic environment '.repeat(6),
];
const ids = names.map((_, index) => `sbx_fixture_${index}_${'long_identifier_'.repeat(5)}`);

async function routeDeletions(page: Page, fixture: LocalAgentV2) {
  const sandboxes = names.map((name, index) => ({
    ...fixture.boundary.sandbox,
    id: ids[index]!,
    name,
    status: index === 0 ? ('active' as const) : ('retiring' as const),
    workspaces: [],
    workspace_count: 0,
    agent_count: 0,
    worktree_count: 0,
  }));
  await page.addInitScript((id) => {
    localStorage.setItem(
      'cma:deletion:local-owner',
      JSON.stringify({
        key: 'synthetic-accepted-deletion',
        accepted: true,
        action: { kind: 'delete_sandbox', sandboxId: id },
      }),
    );
  }, ids[0]!);
  await page.route('**/api/v1/sandboxes**', (route) => {
    const url = new URL(route.request().url());
    if (route.request().method() !== 'GET') return route.fallback();
    if (url.pathname === '/api/v1/sandboxes')
      return route.fulfill({ json: { items: [fixture.boundary.sandbox, ...sandboxes] } });
    const sandbox = sandboxes.find((item) => url.pathname === `/api/v1/sandboxes/${item.id}`);
    if (sandbox) return route.fulfill({ json: sandbox });
    if (sandboxes.some((item) => url.pathname === `/api/v1/sandboxes/${item.id}/events`))
      return route.fulfill({ json: [] });
    return route.fallback();
  });
}

for (const theme of ['light', 'dark']) {
  for (const [width, zoom] of [
    [1280, 1],
    [390, 1],
    [1280, 2],
  ] as const) {
    test(`LOCAL deletion progress rows fit ${width}px at ${zoom * 100}% zoom in ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      const writes: string[] = [];
      page.on('request', (request) => {
        if (
          new URL(request.url()).pathname.startsWith('/api/') &&
          !['GET', 'HEAD'].includes(request.method())
        )
          writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
      });
      try {
        await page.setViewportSize({ width, height: 1000 });
        await fixture.route(page, baseURL!);
        await routeDeletions(page, fixture);
        await page.goto('/agents/agt_contract');
        await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
        await page.evaluate(
          ({ theme, zoom }) => {
            document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
            document.documentElement.style.zoom = String(zoom);
            Object.defineProperty(navigator, 'clipboard', {
              configurable: true,
              value: {
                writeText: async (value: string) => {
                  sessionStorage.setItem('synthetic-copied-id', value);
                },
              },
            });
          },
          { theme, zoom },
        );
        const sidebar = page.getByRole('complementary', {
          name: 'Application navigation',
          exact: true,
          includeHidden: true,
        });
        if (await sidebar.evaluate((element) => element.classList.contains('sidebar--collapsed')))
          await page.getByRole('button', { name: 'Show sidebar', exact: true }).click();
        const summary = sidebar.getByText('Deletion progress (3)', { exact: true });
        await summary.focus();
        await page.keyboard.press('Enter');
        const progress = sidebar.locator('details').filter({ hasText: 'Deletion progress (3)' });
        for (const name of names) {
          const label = progress.getByText(name, { exact: true });
          await expect(label).toBeVisible();
          expect(
            await label.evaluate((element) => {
              const bounds = element.getBoundingClientRect();
              const range = document.createRange();
              range.selectNodeContents(element);
              const text = range.getBoundingClientRect();
              return text.top >= bounds.top - 1 && text.bottom <= bounds.bottom + 1;
            }),
          ).toBe(true);
        }
        const rows = progress.getByRole('listitem');
        await expect(rows).toHaveCount(3);
        const geometry = async () =>
          rows.evaluateAll((elements) =>
            elements.every((element, index) => {
              const row = element.getBoundingClientRect();
              const previous = elements[index - 1]?.getBoundingClientRect();
              const container = element.closest('details')!.getBoundingClientRect();
              return (
                element.scrollWidth <= element.clientWidth + 1 &&
                row.left >= container.left &&
                row.right <= container.right + 1 &&
                (previous === undefined || previous.bottom < row.top)
              );
            }),
          );
        expect(await geometry()).toBe(true);
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await expect(rows.nth(0)).toContainText('Deletion requested');
        await expect(rows.nth(1)).toContainText('Deleting');
        await page.keyboard.press('Tab');
        const view = rows.first().getByRole('button', { name: `View status for ${names[0]}` });
        await expect(view).toBeFocused();
        await page.keyboard.press('Tab');
        await expect(
          rows.first().getByRole('button', { name: `Copy identifier for ${names[0]}` }),
        ).toBeFocused();
        await page.keyboard.press('Enter');
        expect(await page.evaluate(() => sessionStorage.getItem('synthetic-copied-id'))).toBe(
          ids[0],
        );
        await expect(rows.first().getByRole('status')).toHaveText('Identifier copied.');
        await page.evaluate(() =>
          Object.defineProperty(navigator, 'clipboard', {
            configurable: true,
            value: {
              writeText: async () => {
                throw new DOMException('Synthetic denial', 'NotAllowedError');
              },
            },
          }),
        );
        await rows
          .nth(1)
          .getByRole('button', { name: `Copy identifier for ${names[1]}` })
          .click();
        await expect(rows.nth(1).getByRole('status')).toHaveText(
          'Copy unavailable. Select the full identifier below.',
        );
        await expect(rows.nth(1).locator('.deletion-progress__full')).toHaveText(ids[1]!);
        expect(await geometry()).toBe(true);
        await page.screenshot({ path: info.outputPath(`deletion-${width}-${zoom}-${theme}.png`) });
        await view.focus();
        await page.keyboard.press('Enter');
        await expect(
          page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true }),
        ).toBeVisible();
        expect(writes).toEqual([]);
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }
}
