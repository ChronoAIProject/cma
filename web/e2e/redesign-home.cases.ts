import { expect, test, type Page, type TestInfo } from '@playwright/test';
import { redesignHomeFixture, type HomeState } from './redesign-home.fixture';

async function prepare(page: Page, theme: string, width = 1440) {
  await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
}

async function capture(page: Page, info: TestInfo, theme: string, name: string) {
  await page.evaluate(() => document.fonts.ready);
  await expect
    .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
    .toBe(true);
  const content = await page.locator('.task-home').boundingBox();
  expect(content).not.toBeNull();
  expect(content!.x).toBeGreaterThanOrEqual(0);
  expect(content!.x + content!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  await page.screenshot({ path: info.outputPath(`${theme}/${name}.png`), fullPage: false });
}

export function registerRedesignHomeCases() {
  const screens: readonly [HomeState, string][] = [
    ['crew', '01-home'],
    ['empty', '02-home-first-visit'],
    ['near', '03-capacity-near-full'],
    ['full', '04-capacity-full'],
    ['capacity-error', '05-capacity-read-failed'],
    ['access-error', '19-access'],
  ];
  for (const theme of ['dark', 'light']) {
    for (const [state, name] of screens) {
      test(`LOCAL redesign P1 ${theme} ${name} uses observed facts without creating work`, async ({
        page,
        baseURL,
      }, info) => {
        await prepare(page, theme);
        const fixture = await redesignHomeFixture(page, baseURL!, state);
        await page.goto('/agent-manager');
        const input = page.getByLabel('Your task', { exact: true });
        if (state === 'empty')
          await expect(page.getByRole('heading', { name: 'Give it a job.' })).toBeVisible();
        else
          await expect(
            page.getByRole('region', { name: 'Your crew' }).getByRole('button'),
          ).toHaveCount(3);
        if (state !== 'empty') {
          for (const card of await page.locator('.task-crew__card').all()) {
            const top = await card.locator('.task-crew__top').boundingBox();
            const title = await card.locator('strong').boundingBox();
            expect(top).not.toBeNull();
            expect(title).not.toBeNull();
            expect(title!.y).toBeGreaterThanOrEqual(top!.y + top!.height);
          }
        }
        if (state === 'crew' || state === 'empty')
          await expect(page.getByRole('region', { name: 'Sandbox capacity' })).toHaveCount(0);
        if (state === 'near')
          await expect(page.getByText('1 environment slot left', { exact: true })).toBeVisible();
        if (state === 'full') {
          await input.fill('Draft a weekly summary of the sample project.');
          await expect(page.getByText('All 5 Sandbox environment slots are in use')).toBeVisible();
          await expect(page.getByText(/2 environments are being deleted/)).toBeVisible();
          await expect(
            page.getByRole('button', { name: 'Start task', exact: true }),
          ).toBeDisabled();
        }
        if (state === 'capacity-error') {
          await input.fill('Draft kept while capacity is unknown');
          await expect(
            page.getByText('Capacity could not be checked', { exact: true }),
          ).toBeVisible();
          await expect(page.getByText(/All .* slots are in use/)).toHaveCount(0);
          await expect(
            page.getByRole('button', { name: 'Start task', exact: true }),
          ).toBeDisabled();
        }
        if (state === 'access-error') {
          await input.fill('Draft kept while organizations are unavailable');
          await page.getByRole('button', { name: 'Review access' }).click();
          await expect(page.getByRole('dialog', { name: 'Access' })).toContainText(
            'Your organizations didn’t load',
          );
        }
        await capture(page, info, theme, name);
        if (state === 'access-error') {
          fixture.organizationsRecover();
          await page
            .getByRole('dialog', { name: 'Access' })
            .getByRole('button', { name: 'Try again' })
            .click();
          await expect(page.getByText('Organization choices are available')).toBeVisible();
          await page.keyboard.press('Escape');
          await expect(page.getByRole('button', { name: 'Review access' })).toBeFocused();
          await expect(input).toHaveValue('Draft kept while organizations are unavailable');
        }
        if (state === 'capacity-error') {
          fixture.capacityRecovers();
          await page.getByRole('button', { name: 'Check again' }).click();
          await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeEnabled();
          await expect(input).toHaveValue('Draft kept while capacity is unknown');
        }
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
      });
    }
    test(`LOCAL redesign P1 ${theme} accepted then unconfirmed setup keeps the original request`, async ({
      page,
      baseURL,
    }, info) => {
      await prepare(page, theme);
      const fixture = await redesignHomeFixture(page, baseURL!, 'accepted');
      await page.goto('/agent-manager');
      await page
        .getByLabel('Your task', { exact: true })
        .fill('Draft a weekly summary of the sample project.');
      await page.getByRole('button', { name: 'Start task', exact: true }).click();
      await expect(page.getByRole('region', { name: 'Task preparation' })).toContainText(
        'Preparation is still in progress.',
      );
      await expect(page.getByRole('region', { name: 'Submitted task' })).toContainText(
        'Draft a weekly summary',
      );
      const original = structuredClone(fixture.writes);
      expect(original).toHaveLength(1);
      const stored = await page.evaluate(() =>
        sessionStorage.getItem(
          `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`,
        ),
      );
      await capture(page, info, theme, '06-accepted');
      fixture.failStatus();
      await page.clock.runFor(1500);
      await expect(page.getByText('The latest setup status is unconfirmed')).toBeVisible();
      await expect(page.getByText(/Last confirmed: Preparing/)).toBeVisible();
      await expect(page.getByRole('button', { name: 'Start another task' })).toHaveCount(0);
      await capture(page, info, theme, '07-stuck');
      await page.getByRole('button', { name: 'Retry task status' }).click();
      await page.clock.runFor(500);
      expect(fixture.writes).toEqual(original);
      expect(
        await page.evaluate(() =>
          sessionStorage.getItem(
            `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`,
          ),
        ),
      ).toBe(stored);
      await expect(page).toHaveURL(/\/agent-manager$/);
      expect(fixture.boundary.errors).toEqual([]);
    });
    for (const width of [390, 720]) {
      test(`LOCAL redesign P1 ${theme} ${width === 390 ? 'narrow' : '720px viewport'} retains reachable actions`, async ({
        page,
        baseURL,
      }, info) => {
        await prepare(page, theme, width);
        const fixture = await redesignHomeFixture(page, baseURL!, 'full');
        await page.goto('/agent-manager');
        await page
          .getByLabel('Your task', { exact: true })
          .fill(
            'A long draft with a retained repository context and a descriptive task title that must remain readable on a narrow screen.',
          );
        const manage = page
          .getByRole('region', { name: 'Sandbox capacity' })
          .getByRole('button', { name: 'Manage environments' });
        await manage.focus();
        await expect(manage).toBeFocused();
        await capture(page, info, theme, width === 390 ? 'narrow-390' : 'viewport-720');
        await manage.press('Enter');
        await expect(page.getByRole('dialog', { name: 'Manage environments' })).toBeVisible();
        await page.keyboard.press('Escape');
        expect(fixture.writes).toEqual([]);
      });
    }
  }
  test('LOCAL redesign P1 navigation read failure cannot masquerade as first visit', async ({
    page,
    baseURL,
  }) => {
    const fixture = await redesignHomeFixture(page, baseURL!, 'navigation-error');
    await page.goto('/agent-manager');
    await expect(page.getByRole('main').getByRole('alert')).toBeVisible();
    await expect(page.getByText('Your first agent starts here')).toHaveCount(0);
    expect(fixture.writes).toEqual([]);
  });
}

export function registerRedesignTemplateCases() {
  for (const theme of ['dark', 'light']) {
    for (const supported of [true, false]) {
      test(`LOCAL redesign P1 ${theme} template ${supported ? 'empty and filled' : 'unavailable'} retains the compiled request`, async ({
        page,
        baseURL,
      }, info) => {
        await prepare(page, theme);
        const fixture = await redesignHomeFixture(page, baseURL!, 'crew', supported);
        await page.goto('/agent-manager');
        await page.getByLabel('Your task', { exact: true }).fill('Independent free-text draft');
        await page.getByRole('button', { name: 'Write a document', exact: true }).click();
        await expect(page.getByLabel('Document type', { exact: true })).toBeFocused();
        await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeDisabled();
        if (supported) await capture(page, info, theme, '13-template-empty');
        await page.getByLabel('Document type', { exact: true }).fill('brief');
        await page.getByLabel('Subject', { exact: true }).fill('sample project');
        await page.getByLabel('Audience', { exact: true }).fill('the product team');
        const preview = page.getByRole('region', { name: 'Task preview · read-only' });
        await expect(preview).toHaveText(
          'Write a brief about sample project for the product team.',
        );
        if (!supported) {
          await expect(
            page.getByText('Template tasks are not available on this service.'),
          ).toBeVisible();
          await expect(
            page.getByRole('button', { name: 'Start task', exact: true }),
          ).toBeDisabled();
        } else
          await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeEnabled();
        await capture(
          page,
          info,
          theme,
          supported ? '14-template-filled' : '15-template-unavailable',
        );
        if (supported) {
          const text = await preview.textContent();
          await page.getByRole('button', { name: 'Start task', exact: true }).click();
          await expect.poll(() => fixture.writes.length).toBe(1);
          expect(fixture.writes[0]!.body.initial_message).toBe(text);
          expect(fixture.writes[0]!.body.title_mode).toBe('automatic');
        } else expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
      });
    }
  }
}
