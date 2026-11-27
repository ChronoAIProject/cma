import { expect, test } from '@playwright/test';
import { captureDeletion, RedesignDeletionFixture } from './redesign-deletion.fixture';

const sizes = [
  { width: 1440, height: 900, zoom: 1 },
  { width: 390, height: 844, zoom: 1 },
  { width: 360, height: 780, zoom: 1 },
  { width: 320, height: 740, zoom: 1 },
  { width: 1440, height: 900, zoom: 2 },
] as const;

export function registerDeletionConfirmationCases() {
  for (const theme of ['dark', 'light'])
    for (const { width, height, zoom } of sizes) {
      test.describe(`${theme} ${width}px at ${zoom * 100}% equivalent reflow (device scale)`, () => {
        test.use({ deviceScaleFactor: zoom, hasTouch: width <= 390 });
        test(`LOCAL redesign deletion impact is deliberate, readable and dismissible ${theme} ${width}px ${zoom * 100}%`, async ({
          page,
          baseURL,
        }, info) => {
          await page.setViewportSize({ width: width / zoom, height: height / zoom });
          const fixture = new RedesignDeletionFixture();
          fixture.boundary.workspace = {
            ...fixture.boundary.workspace,
            agents: [
              ...fixture.boundary.workspace.agents,
              ...Array.from({ length: 6 }, (_, index) => ({
                ...fixture.boundary.workspace.agents[0]!,
                id: `agt_preview_${index}`,
                title: `Synthetic affected task ${index + 1}`,
              })),
            ],
          };
          await fixture.route(page, baseURL!);
          await page.route(
            /\/api\/v1\/agents\/agt_preview_\d+\/(channel-usage|provenance)$/,
            (route) => {
              expect(route.request().method()).toBe('GET');
              const path = new URL(route.request().url()).pathname;
              const agentId = path.split('/').at(-2)!;
              return route.fulfill({
                json: path.endsWith('/channel-usage')
                  ? {
                      agent_id: agentId,
                      channel_usage: {
                        active: false,
                        has_history: false,
                        first_used_at: null,
                        last_used_at: null,
                      },
                    }
                  : {
                      agent_id: agentId,
                      created_at: fixture.boundary.agent.created_at,
                      creation_origin: 'legacy_unknown',
                    },
              });
            },
          );
          const dialog = await fixture.openConfirmation(page);
          await page.evaluate(
            ({ theme }) => {
              document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
            },
            { theme, zoom },
          );
          const confirm = dialog.getByRole('button', { name: 'Request deletion', exact: true });
          await expect(confirm).toBeDisabled();
          await expect(dialog.getByRole('checkbox')).not.toBeChecked();
          await expect(dialog.getByText(/full impact may not be listed/)).toBeAttached();
          const list = dialog.getByRole('list', { name: 'Known affected resources' });
          await expect(list.getByText('Synthetic affected task 6')).toBeAttached();
          expect(
            await list.evaluate((element) => element.scrollHeight > element.clientHeight),
          ).toBe(true);
          await list.focus();
          await expect(list).toBeFocused();
          await page.keyboard.press('End');
          await page.clock.runFor(300);
          await expect.poll(() => list.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
          await dialog.getByRole('checkbox').focus();
          await dialog.getByRole('button', { name: 'Keep environment' }).scrollIntoViewIfNeeded();
          await expect(dialog.locator('.deletion-impact__warning')).toBeInViewport({ ratio: 1 });
          await expect(dialog.getByRole('checkbox')).toBeInViewport({ ratio: 1 });
          const keep = dialog.getByRole('button', { name: 'Keep environment' });
          await expect(keep).toHaveClass(/button--primary/);
          expect((await keep.boundingBox())!.height).toBeGreaterThanOrEqual(44);
          expect((await confirm.boundingBox())!.height).toBeGreaterThanOrEqual(44);
          await captureDeletion(page, dialog, info, '11-delete-confirm', theme, width, zoom);
          await dialog.getByRole('checkbox').focus();
          await page.keyboard.press('Space');
          await expect(confirm).toBeEnabled();
          await captureDeletion(
            page,
            dialog,
            info,
            '11-delete-confirm-acknowledgement',
            theme,
            width,
            zoom,
          );
          await fixture.settled();
          expect(fixture.commands).toEqual([]);
          await confirm.focus();
          await page.keyboard.press('Tab');
          await expect(dialog.getByRole('button', { name: 'Close', exact: true })).toBeFocused();
          await page.keyboard.press('Shift+Tab');
          await expect(confirm).toBeFocused();
          if (width <= 390) await keep.tap();
          else await keep.press('Enter');
          await expect(dialog).toHaveCount(0);
          await fixture.settled();
          const reads = fixture.reads.length;
          await page.clock.runFor(10000);
          await fixture.settled();
          expect(fixture.reads).toHaveLength(reads);
          expect(fixture.commands).toEqual([]);
          fixture.assertNoUnexpected();
        });
      });
    }

  test('LOCAL redesign deletion permission loss withdraws impact and confirmation', async ({
    page,
    baseURL,
  }) => {
    const fixture = new RedesignDeletionFixture();
    await fixture.route(page, baseURL!);
    const dialog = await fixture.openConfirmation(page);
    await dialog.getByRole('checkbox').check();
    await expect(
      dialog.getByRole('button', { name: 'Request deletion', exact: true }),
    ).toBeEnabled();
    fixture.readFailure = 403;
    await page.clock.runFor(1500);
    await expect(dialog.getByText(/resource is no longer accessible/)).toBeVisible();
    await expect(dialog.getByText('Sample task', { exact: true })).toHaveCount(0);
    await expect(
      dialog.getByRole('button', { name: 'Request deletion', exact: true }),
    ).toBeDisabled();
    await dialog.getByRole('button', { name: 'Keep environment', exact: true }).click();
    expect(fixture.commands).toEqual([]);
    fixture.assertNoUnexpected();
  });

  test('LOCAL redesign deletion lost reply retains the original request across reload and explicit retry', async ({
    page,
    baseURL,
  }) => {
    const fixture = new RedesignDeletionFixture();
    await fixture.route(page, baseURL!);
    fixture.loseResponse = true;
    const dialog = await fixture.openConfirmation(page);
    await dialog.getByRole('checkbox').check();
    await dialog.getByRole('button', { name: 'Request deletion', exact: true }).click();
    const progress = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
    await expect(progress.getByText('Request unconfirmed', { exact: true })).toBeVisible();
    await expect(
      progress.getByRole('button', { name: 'Check request', exact: true }),
    ).toBeEnabled();
    await fixture.settled();
    await page.clock.runFor(15000);
    await fixture.settled();
    expect(fixture.commands).toHaveLength(1);
    const original = fixture.commands[0]!;
    expect(original.key).toBeTruthy();
    await page.reload();
    await expect(progress.getByText('Request unconfirmed', { exact: true })).toBeVisible();
    expect(fixture.commands).toHaveLength(1);
    fixture.loseResponse = false;
    await progress.getByRole('button', { name: 'Check request', exact: true }).click();
    await expect(progress).toHaveCount(0);
    expect(fixture.commands).toEqual([original, original]);
    fixture.assertNoUnexpected();
  });
}

export function registerDeletionStatusCases() {
  for (const theme of ['dark', 'light'])
    for (const width of [1440, 390])
      test(`LOCAL terminal notice reconciles on existing environment re-entry ${theme} ${width}px`, async ({
        page,
        baseURL,
      }, info) => {
        await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
        const fixture = new RedesignDeletionFixture();
        fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'retiring' };
        await fixture.route(page, baseURL!, true);
        await page.route('**/api/v1/agents/agt_sample_notes', (route) => {
          expect(route.request().method()).toBe('GET');
          return route.fulfill({
            json: {
              ...fixture.boundary.agent,
              id: 'agt_sample_notes',
              title: 'Sample notes',
              worktree: {
                ...fixture.boundary.agent.worktree,
                id: 'wkt_sample_notes',
                agent_id: 'agt_sample_notes',
              },
            },
          });
        });
        const dialog = await fixture.openStatus(page);
        await expect(dialog.getByText('Deleting', { exact: true })).toBeVisible();
        await dialog
          .locator('.modal__footer')
          .getByRole('button', { name: 'Close', exact: true })
          .click();
        await expect(dialog).toHaveCount(0);
        await fixture.settled();
        const reads = fixture.reads.length;
        fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'retired' };
        await page.clock.runFor(10000);
        await fixture.settled();
        expect(fixture.reads).toHaveLength(reads);
        await expect(page.getByText('Deleting 1 environment')).toHaveCount(0);
        await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
        const progress = page.getByRole('dialog', { name: 'Deletion progress', exact: true });
        await expect(progress.getByRole('code')).toHaveText(fixture.boundary.sandbox.id);
        await page.keyboard.press('Escape');
        await expect(progress).toHaveCount(0);
        expect(fixture.reads).toHaveLength(reads);
        await page.evaluate(() => {
          localStorage.setItem(
            'cma:deletion-requests:local-owner:synthetic-other-agent',
            JSON.stringify({
              action: { kind: 'delete_agent', agentId: 'agt_other' },
              key: 'synthetic-other-agent',
              accepted: true,
            }),
          );
        });
        await page.goto(`/sandboxes/${fixture.boundary.sandbox.id}`);
        await page.evaluate(
          (theme) => document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark'),
          theme,
        );
        await expect(page.getByText('Deleting 1 environment')).toHaveCount(0);
        await expect(page.getByText('Deleting 1 Agent')).toBeVisible();
        expect(
          await page.evaluate(() => localStorage.getItem('cma:deletion:local-owner')),
        ).toBeNull();
        expect(
          await page.evaluate(() =>
            localStorage.getItem('cma:deletion-requests:local-owner:synthetic-other-agent'),
          ),
        ).not.toBeNull();
        expect(fixture.commands).toEqual([]);
        await page.screenshot({
          path: info.outputPath(`12-terminal-notice-${theme}-${width}.png`),
        });
        fixture.assertNoUnexpected();
      });

  for (const theme of ['dark', 'light'])
    for (const { width, height, zoom } of sizes) {
      test.describe(`${theme} ${width}px at ${zoom * 100}% equivalent reflow (device scale)`, () => {
        test.use({ deviceScaleFactor: zoom, hasTouch: width <= 390 });
        test(`LOCAL redesign deletion status and reference ${theme} ${width}px ${zoom * 100}%`, async ({
          page,
          baseURL,
        }, info) => {
          await page.setViewportSize({ width: width / zoom, height: height / zoom });
          const fixture = new RedesignDeletionFixture();
          fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'retiring' };
          await fixture.route(page, baseURL!, true);
          const dialog = await fixture.openStatus(page);
          await page.evaluate(
            ({ theme }) => {
              document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
              Object.defineProperty(navigator, 'clipboard', {
                configurable: true,
                value: {
                  writeText: async (value: string) => {
                    sessionStorage.setItem('copied-deletion-reference', value);
                  },
                },
              });
            },
            { theme, zoom },
          );
          await expect(dialog.getByText('Deleting', { exact: true })).toBeVisible();
          await expect(
            dialog.getByText(
              'Check Environments for your current Sandbox capacity while deletion finishes.',
              { exact: true },
            ),
          ).toBeVisible();
          await captureDeletion(page, dialog, info, '12-delete-progress', theme, width, zoom);
          await dialog.getByRole('button', { name: 'Copy reference', exact: true }).click();
          await expect(dialog.getByText('Reference copied.', { exact: true })).toBeVisible();
          expect(
            await page.evaluate(() => sessionStorage.getItem('copied-deletion-reference')),
          ).toBe('synthetic-original-delete-reference');
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
          await dialog.getByRole('button', { name: 'Copy reference', exact: true }).click();
          await expect(dialog.getByText(/Copy unavailable/)).toBeVisible();
          await expect(
            dialog.getByText('synthetic-original-delete-reference', { exact: true }),
          ).toBeVisible();
          await page.keyboard.press('Escape');
          await expect(dialog).toHaveCount(0);
          await fixture.settled();
          const reads = fixture.reads.length;
          await page.clock.runFor(10000);
          await fixture.settled();
          expect(fixture.reads).toHaveLength(reads);
          expect(fixture.commands).toEqual([]);
          fixture.assertNoUnexpected();
        });
      });
    }

  for (const status of [403, 404, 503])
    test(`LOCAL redesign deletion ${status} is unconfirmed until retirement is read`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new RedesignDeletionFixture();
      await fixture.route(page, baseURL!, true);
      const dialog = await fixture.openStatus(page);
      await expect(dialog.getByText('Deletion requested', { exact: true })).toBeVisible();
      fixture.readFailure = status;
      await page.clock.runFor(1500);
      await expect(dialog.getByText('Status unavailable', { exact: true })).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Done', exact: true })).toHaveCount(0);
      await fixture.settled();
      expect(fixture.commands).toEqual([]);
      await page.screenshot({ path: info.outputPath(`12-delete-progress-read-${status}.png`) });
      fixture.readFailure = 0;
      fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'retired' };
      await dialog.getByRole('button', { name: 'Refresh status', exact: true }).click();
      const complete = page.getByRole('dialog', { name: 'Sandbox deleted', exact: true });
      await expect(
        complete.getByText('Cleanup confirmed in this view.', { exact: true }),
      ).toBeVisible();
      await expect(complete.getByText('Deleted', { exact: true })).toBeVisible();
      await page.screenshot({
        path: info.outputPath(`12-delete-progress-confirmed-${status}.png`),
      });
      expect(fixture.commands).toEqual([]);
      fixture.assertNoUnexpected();
    });
}
