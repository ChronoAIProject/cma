import { expect, test } from '@playwright/test';
import { captureFlow, flowFixture } from './topographic-flow.fixture';
import { flowProbe, installFlowProbe } from './topographic-flow.probes';
import { expectReadableText } from './themeContrast';

export function registerTopographicFlowCases() {
  test.describe('LOCAL topographic enabled playback', () => {
    for (const target of ['landing', 'first-visit'] as const)
      for (const theme of ['dark', 'light']) {
        test(`${target} ${theme} has bounded real draws and preserves interaction`, async ({
          browser,
          baseURL,
        }, info) => {
          const context = await browser.newContext({
            baseURL,
            serviceWorkers: 'block',
            recordVideo: { dir: info.outputPath('video'), size: { width: 1440, height: 900 } },
          });
          const page = await context.newPage();
          try {
            await page.setViewportSize({ width: 1440, height: 900 });
            await page.emulateMedia({
              reducedMotion: 'no-preference',
              colorScheme: theme as 'light' | 'dark',
            });
            await page.addInitScript(
              (value) => localStorage.setItem('cma:appearance', value),
              theme,
            );
            await installFlowProbe(page);
            const fixture = await flowFixture(page, baseURL!, target);
            await page.goto('/');
            const action = page.getByRole('button', {
              name: target === 'landing' ? 'Get Started' : 'Start task',
              exact: true,
            });
            await expect(action).toBeVisible();
            await expect(page.locator('.topographic-backdrop')).toHaveCount(1);
            await expect.poll(async () => (await flowProbe(page)).draws.length).toBeGreaterThan(1);
            await expectReadableText(
              page.locator(target === 'landing' ? '.sign-in-landing__copy' : '.task-home'),
            );
            await captureFlow(page, info, `${target}-${theme}-moving`);
            await page.waitForTimeout(4300);
            const stopped = await flowProbe(page);
            expect(stopped.contexts).toBe(1);
            expect(stopped.contentReady).toEqual([true]);
            expect(stopped.draws.at(-1)!.at - stopped.contextTimes[0]!).toBeLessThan(4000);
            expect(stopped.draws.length).toBeLessThanOrEqual(118);
            for (let index = 1; index < stopped.draws.length; index++)
              expect(
                stopped.draws[index]!.at - stopped.draws[index - 1]!.at,
              ).toBeGreaterThanOrEqual(33.5);
            for (const draw of stopped.draws)
              expect(draw.width * draw.height).toBeLessThanOrEqual(300000);
            const box = await page.locator('.topographic-backdrop').boundingBox();
            expect(box!.width).toBeLessThanOrEqual(560);
            expect(box!.height).toBeLessThanOrEqual(220);
            expect(stopped.releases).toBe(stopped.allocations);
            expect(stopped.contextReleases).toBe(1);
            expect(stopped.pendingFrames).toEqual([]);
            expect(stopped.errors).toEqual([]);
            expect(stopped.fatal).toEqual([]);
            expect(stopped.canvas2d).toBe(0);
            if (target === 'landing') expect(stopped.mascotDraws).toBeGreaterThan(0);
            await page.waitForTimeout(300);
            expect((await flowProbe(page)).draws).toEqual(stopped.draws);
            await captureFlow(page, info, `${target}-${theme}-static`);
            expect(fixture.boundary.writes).toEqual([]);
            expect(fixture.boundary.errors).toEqual([]);
            if (target === 'landing') {
              await action.click();
              await expect.poll(() => fixture.signIns.length).toBe(1);
            } else {
              await page
                .getByRole('textbox', { name: 'Your task', exact: true })
                .fill('Keep the original task composer usable.');
              await expect(action).toBeEnabled();
            }
          } finally {
            await context.close();
          }
        });
      }
  });

  for (const policy of [
    'reduce',
    'mobile',
    '720px',
    'coarse',
    'save-data',
    'no-observer',
    'no-webgl',
  ] as const)
    for (const target of ['landing', 'first-visit'] as const) {
      test(`LOCAL topographic ${target} ${policy} stays static without vendor download`, async ({
        page,
        baseURL,
      }, info) => {
        await page.setViewportSize({
          width: policy === 'mobile' ? 390 : policy === '720px' ? 720 : 1440,
          height: 900,
        });
        await page.emulateMedia({
          reducedMotion: policy === 'reduce' ? 'reduce' : 'no-preference',
        });
        if (policy === 'coarse')
          await page.addInitScript(() => {
            const match = matchMedia;
            window.matchMedia = (query) =>
              query === '(pointer: coarse)'
                ? Object.assign(match(query), { matches: true })
                : match(query);
          });
        if (policy === 'save-data')
          await page.addInitScript(() =>
            Object.defineProperty(navigator, 'connection', { value: { saveData: true } }),
          );
        if (policy === 'no-observer')
          await page.addInitScript(() => {
            Reflect.deleteProperty(window, 'IntersectionObserver');
          });
        await installFlowProbe(page, { noGL: policy === 'no-webgl' });
        const fixture = await flowFixture(page, baseURL!, target);
        await page.goto('/');
        await expect(page.locator('.topographic-backdrop__static')).toBeVisible();
        await page.waitForTimeout(300);
        const probe = await flowProbe(page);
        expect(probe.draws).toEqual([]);
        expect(probe.contexts).toBe(0);
        expect(probe.canvas2d).toBe(0);
        expect(fixture.chunks).toEqual([]);
        expect(probe.fatal).toEqual([]);
        expect(probe.errors).toEqual([]);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await captureFlow(page, info, `${target}-${policy}`);
      });
    }

  for (const fault of ['shader', 'allocation', 'draw', 'context', 'chunk'] as const) {
    test(`LOCAL topographic landing ${fault} failure leaves sign-in usable`, async ({
      page,
      baseURL,
    }) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await installFlowProbe(page, {
        shaderFailure: fault === 'shader',
        allocationFailure: fault === 'allocation',
        drawFailure: fault === 'draw',
      });
      const fixture = await flowFixture(page, baseURL!, 'landing');
      if (fault === 'chunk')
        await page.route('**/src/decoration/vendor/**/topographic-surface*', (route) =>
          route.abort('failed'),
        );
      await page.goto('/');
      if (fault === 'context') {
        await expect.poll(async () => (await flowProbe(page)).draws.length).toBeGreaterThan(1);
        await page.evaluate(() => window.__flowLose());
      }
      await expect.poll(async () => (await flowProbe(page)).contextReleases).toBe(1);
      const stopped = await flowProbe(page);
      expect(stopped.releases).toBe(stopped.allocations);
      expect(stopped.pendingFrames).toEqual([]);
      expect(stopped.errors).toEqual([]);
      expect(stopped.fatal).toEqual([]);
      if (fault === 'context') await page.evaluate(() => window.__flowRestore());
      await page.waitForTimeout(300);
      expect((await flowProbe(page)).draws).toEqual(stopped.draws);
      if (fault === 'chunk') expect(fixture.chunks).toHaveLength(1);
      await page.getByRole('button', { name: 'Open Event Gateway', exact: true }).click();
      await expect.poll(() => fixture.signIns).toEqual(['/event-gateway']);
    });
  }

  test('LOCAL topographic landing focus stops playback before sign-in', async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installFlowProbe(page);
    const fixture = await flowFixture(page, baseURL!, 'landing');
    await page.goto('/');
    await expect.poll(async () => (await flowProbe(page)).draws.length).toBeGreaterThan(1);
    const action = page.getByRole('button', { name: 'Get Started', exact: true });
    await action.focus();
    const stopped = await flowProbe(page);
    await page.waitForTimeout(300);
    expect((await flowProbe(page)).draws).toEqual(stopped.draws);
    expect(stopped.releases).toBe(stopped.allocations);
    expect(stopped.pendingFrames).toEqual([]);
    await action.press('Enter');
    await expect.poll(() => fixture.signIns.length).toBe(1);
  });

  test('LOCAL topographic landing reduced-motion transition permanently stops real draws', async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installFlowProbe(page);
    await flowFixture(page, baseURL!, 'landing');
    await page.goto('/');
    await expect.poll(async () => (await flowProbe(page)).draws.length).toBeGreaterThan(1);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect.poll(async () => (await flowProbe(page)).contextReleases).toBe(1);
    const stopped = await flowProbe(page);
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.waitForTimeout(300);
    expect((await flowProbe(page)).draws).toEqual(stopped.draws);
    expect(stopped.pendingFrames).toEqual([]);
    expect(stopped.fatal).toEqual([]);
  });

  test('LOCAL topographic landing visibility changes pause submissions and preserve the deadline', async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installFlowProbe(page);
    await flowFixture(page, baseURL!, 'landing');
    await page.goto('/');
    await expect.poll(async () => (await flowProbe(page)).draws.length).toBeGreaterThan(1);
    await page.evaluate(() => {
      Object.defineProperty(document, 'hidden', { configurable: true, value: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    const background = await flowProbe(page);
    await page.waitForTimeout(200);
    expect((await flowProbe(page)).draws).toEqual(background.draws);
    expect(background.pendingFrames).toEqual([]);
    await page.evaluate(() => {
      Reflect.deleteProperty(document, 'hidden');
      document.dispatchEvent(new Event('visibilitychange'));
    });
    const host = page.locator('.sign-in-landing__brand');
    await host.evaluate((node: HTMLElement) => {
      node.hidden = true;
    });
    await expect.poll(async () => (await flowProbe(page)).pendingFrames.length).toBe(0);
    const hidden = await flowProbe(page);
    await page.waitForTimeout(300);
    expect((await flowProbe(page)).draws).toEqual(hidden.draws);
    await host.evaluate((node: HTMLElement) => {
      node.hidden = false;
    });
    await expect
      .poll(async () => (await flowProbe(page)).draws.length)
      .toBeGreaterThan(hidden.draws.length);
    await host.evaluate((node: HTMLElement) => {
      node.style.transform = 'translateY(10000px)';
    });
    await expect.poll(async () => (await flowProbe(page)).pendingFrames.length).toBe(0);
    const offscreen = await flowProbe(page);
    await page.waitForTimeout(4300);
    expect((await flowProbe(page)).draws).toEqual(offscreen.draws);
    await host.evaluate((node: HTMLElement) => {
      node.style.transform = '';
    });
    await page.waitForTimeout(300);
    expect((await flowProbe(page)).draws).toEqual(offscreen.draws);
    expect((await flowProbe(page)).contextReleases).toBe(1);
  });

  test('LOCAL topographic ordinary Home and retained work never request the renderer', async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installFlowProbe(page);
    const fixture = await flowFixture(page, baseURL!, 'ordinary');
    for (const path of ['/', '/agents/agt_contract']) {
      await page.goto(path);
      await expect(page.locator('.frame__center')).toBeVisible();
      await page.waitForTimeout(200);
      expect(fixture.chunks).toEqual([]);
      await expect(page.locator('.topographic-backdrop')).toHaveCount(0);
    }
    expect(fixture.boundary.writes).toEqual([]);
  });
  test('LOCAL topographic landing theme and resize storms share the actual submission budget', async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installFlowProbe(page);
    await flowFixture(page, baseURL!, 'landing');
    await page.goto('/');
    await expect.poll(async () => (await flowProbe(page)).draws.length).toBeGreaterThan(1);
    await page.evaluate(async () => {
      const host = document.querySelector<HTMLElement>('.sign-in-landing__brand')!;
      for (let i = 0; i < 40; i++) {
        document.body.toggleAttribute('data-ds-dark-theme');
        host.style.width = `${400 + (i % 2) * 80}px`;
        await new Promise((resolve) => setTimeout(resolve, 8));
      }
    });
    await page.waitForTimeout(4300);
    const state = await flowProbe(page);
    for (let index = 1; index < state.draws.length; index++)
      expect(state.draws[index]!.at - state.draws[index - 1]!.at).toBeGreaterThanOrEqual(33.5);
    expect(state.draws.at(-1)!.at - state.contextTimes[0]!).toBeLessThan(4000);
    expect(state.releases).toBe(state.allocations);
    expect(state.pendingFrames).toEqual([]);
    expect(state.fatal).toEqual([]);
    expect(state.errors).toEqual([]);
  });
}
