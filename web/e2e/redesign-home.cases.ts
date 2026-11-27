import { selectHomeTemplate } from './home-simplification.fixture';
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
  await expect
    .poll(async () => {
      await page.clock.runFor(50);
      const box = await page.locator('.task-home').boundingBox();
      return box ? box.x + box.width : Infinity;
    })
    .toBeLessThanOrEqual(page.viewportSize()!.width);
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
        if (state === 'empty') {
          await expect(page.getByRole('heading', { name: 'Give it a job.' })).toBeVisible();
          await expect(
            page.locator('.task-home__first-mascot .agent-mascot__poster'),
          ).toBeVisible();
        } else
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
          await expect(page.getByText('1 slot left', { exact: true })).toBeVisible();
        if (state === 'full') {
          await input.fill('Draft a weekly summary of the sample project.');
          await expect(page.getByText('Environment limit reached')).toBeVisible();
          await expect(page.getByRole('region', { name: 'Sandbox capacity' })).toHaveCount(1);
          await expect(page.getByText('5/5 Sandbox environments used')).toHaveCount(0);
          const manage = await page
            .getByRole('region', { name: 'Sandbox capacity' })
            .getByRole('button', { name: 'Environments' })
            .boundingBox();
          expect(manage!.y + manage!.height).toBeLessThan(900);
          const templates = await page
            .getByRole('button', { name: 'Templates', exact: true })
            .boundingBox();
          expect(templates!.y).toBeLessThan(900);
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
        'Getting its cloud computer ready',
      );
      await expect(page.getByRole('region', { name: 'Original request' })).toContainText(
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
      await expect(page.getByRole('heading', { name: 'Setup updates interrupted' })).toBeVisible();
      await expect(page.getByText('Request accepted', { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Start another task' })).toHaveCount(0);
      await capture(page, info, theme, '07-stuck');
      const recovery = page.getByRole('region', { name: 'Task preparation' });
      await expect(recovery.getByRole('alert')).toHaveCount(1);
      await recovery.getByRole('button', { name: 'Retry task status' }).click();
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
    for (const width of [1440, 320, 360, 390, 720]) {
      test(`LOCAL Home focus ${theme} at ${width}px has contrast and a forced-colors fallback`, async ({
        page,
        baseURL,
      }, info) => {
        await prepare(page, theme, width);
        const fixture = await redesignHomeFixture(page, baseURL!, 'empty');
        await page.goto('/agent-manager');
        const input = page.getByLabel('Your task', { exact: true });
        await expect(input).toHaveAttribute(
          'placeholder',
          'Describe the job. It gets its own cloud computer to do it.',
        );
        await page.getByRole('link', { name: 'Help starting a task' }).focus();
        await page.keyboard.press('Tab');
        await expect(input).toBeFocused();
        const focus = await page.locator('.task-home__composer').evaluate((node) => {
          const style = getComputedStyle(node);
          const luminance = (color: string) => {
            const channels = color
              .match(/[\d.]+/g)!
              .slice(0, 3)
              .map(Number)
              .map((value) => {
                const channel = value / 255;
                return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
              });
            return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722;
          };
          const ring = luminance(style.outlineColor);
          const surfaces = [style.backgroundColor, getComputedStyle(document.body).backgroundColor];
          return {
            width: style.outlineWidth,
            style: style.outlineStyle,
            contrasts: surfaces.map((surface) => {
              const background = luminance(surface);
              return (Math.max(ring, background) + 0.05) / (Math.min(ring, background) + 0.05);
            }),
          };
        });
        expect(focus.width).toBe('2px');
        expect(focus.style).toBe('solid');
        for (const ratio of focus.contrasts) expect(ratio).toBeGreaterThanOrEqual(3);
        await info.attach('focus-contrast', {
          body: JSON.stringify(focus),
          contentType: 'application/json',
        });
        await capture(page, info, theme, `focus-${width}`);
        await page.emulateMedia({ forcedColors: 'active' });
        await expect(page.locator('.task-home__composer')).toHaveCSS('outline-style', 'solid');
        await expect(page.locator('.task-home__composer')).toHaveCSS('outline-width', '2px');
        await capture(page, info, theme, `focus-forced-colors-${width}`);
        await page.emulateMedia({ forcedColors: 'none', reducedMotion: 'no-preference' });
        await expect(input).toBeFocused();
        expect(fixture.writes).toEqual([]);
      });
    }
    test(`LOCAL Home ${theme} receipt recovery retains exact body and key with a newer draft`, async ({
      page,
      baseURL,
    }, info) => {
      await prepare(page, theme);
      const fixture = await redesignHomeFixture(page, baseURL!, 'crew');
      const writes: { body: unknown; key: string | undefined }[] = [];
      await page.route('**/api/agent-manager/v1/agents', async (route) => {
        if (route.request().method() !== 'POST') return route.fallback();
        writes.push({
          body: route.request().postDataJSON(),
          key: route.request().headers()['idempotency-key'],
        });
        await route.fulfill({ status: 503, json: { code: 'unavailable' } });
      });
      await page.goto('/agent-manager');
      const input = page.getByLabel('Your task', { exact: true });
      await input.fill('Original retained task');
      await page.getByRole('button', { name: 'Start task', exact: true }).click();
      const recovery = page.getByRole('region', { name: 'Task preparation', exact: true });
      await expect(recovery.getByRole('alert')).toContainText('resend the same request');
      await expect(recovery.getByRole('alert')).toHaveCount(1);
      const followup = page.getByRole('textbox', { name: 'Follow-up draft', exact: true });
      await followup.fill('Separate newer draft');
      await capture(page, info, theme, 'receipt-unconfirmed');
      for (const width of [320, 360, 390, 720]) {
        await page.setViewportSize({ width, height: 900 });
        await page.clock.runFor(150);
        await recovery.getByRole('button', { name: 'Recover original task' }).focus();
        await capture(page, info, theme, `receipt-unconfirmed-${width}`);
      }
      await recovery.getByRole('button', { name: 'Recover original task' }).click();
      await expect.poll(() => writes.length).toBe(2);
      expect(writes[1]).toEqual(writes[0]);
      expect(writes[0]!.key).toBeTruthy();
      await expect(followup).toHaveValue('Separate newer draft');
      await expect(page).toHaveURL(/\/agent-manager$/);
      expect(fixture.writes).toEqual([]);
    });
    for (const width of [320, 360, 390, 720]) {
      test(`LOCAL redesign P1 ${theme} ${`${width}px viewport`} retains reachable actions`, async ({
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
          .getByRole('button', { name: 'Environments' });
        await manage.focus();
        await expect(manage).toBeFocused();
        await capture(page, info, theme, `viewport-${width}`);
        await manage.press('Enter');
        const sidebar = page.getByRole('complementary', { name: 'Application navigation' });
        await expect(
          sidebar.getByRole('button', { name: 'Environments', exact: true }),
        ).toHaveAttribute('aria-pressed', 'true');
        await sidebar.getByRole('button', { name: 'Tasks', exact: true }).click();
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
    await expect(page.locator('.task-home__first-mascot')).toHaveCount(0);
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
        await selectHomeTemplate(page, 'Write a document');
        await expect(page.getByLabel('Document type', { exact: true })).toBeFocused();
        await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeDisabled();
        if (supported) await capture(page, info, theme, '13-template-empty');
        await page.getByRole('button', { name: 'Write a task without a template' }).click();
        await expect(page.getByRole('dialog')).toHaveCount(0);
        await expect(page.getByLabel('Your task', { exact: true })).toHaveValue(
          'Independent free-text draft',
        );
        await page.getByRole('button', { name: 'Resume document template' }).click();
        await page.getByLabel('Document type', { exact: true }).fill('brief');
        await page.getByLabel('Subject', { exact: true }).fill('sample project');
        await page.getByLabel('Audience', { exact: true }).fill('the product team');
        const preview = page.getByRole('region', { name: 'Task preview · read-only' });
        await expect(preview).toHaveText(
          'Write a brief about sample project for the product team.',
        );
        if (!supported) {
          const notice = page.getByText('Template tasks are not available on this service.');
          await expect(notice).toBeVisible();
          const noticeBox = await notice.boundingBox();
          const fieldBox = await page.getByLabel('Document type', { exact: true }).boundingBox();
          expect(noticeBox!.y).toBeLessThan(fieldBox!.y);
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
        for (const width of [320, 360, 390, 720]) {
          await page.setViewportSize({ width, height: 900 });
          await page.clock.runFor(150);
          await page.getByLabel('Document type', { exact: true }).focus();
          await expect(page.getByLabel('Document type', { exact: true })).toBeFocused();
          await capture(
            page,
            info,
            theme,
            `template-${supported ? 'filled' : 'unavailable'}-${width}`,
          );
          await page.getByRole('button', { name: 'Write a task without a template' }).focus();
          await expect(
            page.getByRole('button', { name: 'Write a task without a template' }),
          ).toBeInViewport();
        }
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
