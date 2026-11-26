import { expect, test } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';
import { desktopBoundary } from './desktop-rfb.fixture';
import { SkillCatalogFixture, skill, problem } from './skill-catalog.fixture';
import {
  appearance,
  capture,
  liveHead,
  noClipping,
  openSkillsModal,
  originalMessage,
  startingAgent,
  stoppedClock,
  workingAgent,
} from './redesign-agent.fixture';

export function registerAgentRedesignCases() {
  for (const theme of ['dark', 'light'] as const) {
    test(`LOCAL redesign Agent preparing and stale ${theme} never infer ready from elapsed time`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      startingAgent(fixture);
      let failed = false;
      let settled = 0;
      try {
        await appearance(page, theme);
        await stoppedClock(page);
        await fixture.route(page, baseURL!);
        await page.route(`**/api/v1/sandboxes/${fixture.boundary.sandbox.id}`, async (route) => {
          if (!failed) return route.fallback();
          await route.fulfill({
            status: 503,
            json: { code: 'unavailable', detail: 'Status unavailable' },
          });
          settled++;
        });
        await page.goto('/agents/agt_contract');
        const preparation = page.getByRole('region', { name: 'Agent preparation', exact: true });
        await expect(
          preparation.getByRole('heading', { name: 'Getting its cloud computer ready' }),
        ).toBeVisible();
        await expect(preparation.locator('time')).toBeVisible();
        await capture(page, info, '06-accepted', theme);
        failed = true;
        for (let index = 0; index < 2; index++) {
          await page.clock.fastForward(5_000);
          await expect.poll(() => settled).toBe(index + 1);
        }
        await expect(preparation).toContainText('Last confirmed: Preparing');
        await capture(page, info, '07-stuck', theme);
        await page.clock.fastForward(60_000);
        await expect(preparation).toContainText('Last confirmed: Preparing');
        expect(fixture.writes).toEqual([]);
        failed = false;
        fixture.boundary.agent = {
          ...fixture.boundary.agent,
          status: 'ready',
          ready_at: '2026-01-02T03:05:05Z',
          worktree: { ...fixture.boundary.agent.worktree, status: 'ready' },
        };
        fixture.append({ type: 'status_changed', from: 'starting', to: 'ready' });
        await page.clock.fastForward(5_000);
        await expect(preparation).toHaveCount(0);
        expect(fixture.writes).toEqual([]);
        expect(fixture.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });

    test(`LOCAL redesign Agent working ${theme} uses real conversation without synthetic steps`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      workingAgent(fixture);
      try {
        await appearance(page, theme);
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        const activity = page.getByRole('region', { name: 'Agent activity', exact: true });
        await expect(activity.getByRole('heading', { name: 'Working on it' })).toBeVisible();
        await expect(
          page.getByText('I’m reviewing the project files.', { exact: true }),
        ).toBeVisible();
        await expect(activity.locator('ol, progress')).toHaveCount(0);
        await capture(page, info, '08-running', theme);
        expect(fixture.writes).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });

    test(`LOCAL redesign Agent unknown ${theme} retains the original request through recovery and copying`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      fixture.loseInput = true;
      fixture.revealInputReceipt = false;
      try {
        await appearance(page, theme);
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        await page.getByRole('textbox', { name: 'Message', exact: true }).fill(originalMessage);
        await page.getByRole('button', { name: 'Send message', exact: true }).click();
        const recover = page.getByRole('button', { name: 'Recover pending request', exact: true });
        await expect(recover).toBeEnabled();
        expect(fixture.writes).toHaveLength(1);
        const original = structuredClone(fixture.writes[0]);
        await expect(
          page.getByRole('heading', { name: 'We can’t confirm this message arrived' }),
        ).toBeVisible();
        await capture(page, info, '09-unknown', theme);
        await page.setViewportSize({ width: 390, height: 844 });
        await recover.focus();
        await expect(recover).toBeFocused();
        await noClipping(page);
        await capture(page, info, '09-unknown-narrow-focus', theme);
        await page.setViewportSize({ width: 1440, height: 900 });
        await page.evaluate(() => {
          document.documentElement.style.zoom = '2';
        });
        await recover.focus();
        await noClipping(page);
        await capture(page, info, '09-unknown-zoom-200', theme);
        await page.evaluate(() => {
          document.documentElement.style.zoom = '';
          Object.defineProperty(navigator, 'clipboard', {
            configurable: true,
            value: { writeText: () => Promise.reject(new Error('Synthetic clipboard denial')) },
          });
        });
        await page.getByRole('button', { name: 'Copy message', exact: true }).click();
        await expect(page.getByRole('textbox', { name: 'Message to copy' })).toHaveValue(
          originalMessage,
        );
        await page.reload();
        await expect(recover).toBeEnabled();
        expect(fixture.writes).toEqual([original]);
        fixture.revealInputReceipt = true;
        await recover.click();
        await expect(recover).toHaveCount(0);
        expect(fixture.writes).toEqual([original]);
        expect(fixture.receiptReads.length).toBeGreaterThan(0);
        for (const lookup of fixture.receiptReads)
          expect(lookup).toEqual({ ...original, path: `${original!.path}/receipt` });
      } finally {
        await fixture.close(page);
      }
    });
  }
}

export function registerComputerSkillsRedesignCases() {
  for (const theme of ['dark', 'light'] as const) {
    test(`LOCAL redesign Computer exhausted ${theme} stops automatic attempts and reconnects view-only`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
      let available = false;
      let settled = 0;
      let sandboxReads = 0;
      try {
        await appearance(page, theme);
        await stoppedClock(page);
        await fixture.route(page, baseURL!);
        const stream = await desktopBoundary(page);
        await liveHead(page, fixture);
        await page.route(`**/api/v1/sandboxes/${fixture.boundary.sandbox.id}`, async (route) => {
          await route.fulfill({ json: fixture.boundary.sandbox });
          sandboxReads++;
        });
        await page.route('**/api/v1/agents/agt_contract/desktop', async (route) => {
          if (available)
            await route.fulfill({
              json: { websocket_url: '/api/v1/agents/agt_contract/desktop/ws' },
            });
          else
            await route.fulfill({
              status: 503,
              json: { code: 'desktop_runtime_unavailable', detail: 'Synthetic unavailable' },
            });
          settled++;
        });
        await page.goto('/agents/agt_contract');
        await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
        const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
        await expect(desktop.getByRole('status')).toContainText('Reconnecting');
        const delays = [1000, 2000, 4000, 8000, 16000, ...Array<number>(8).fill(30_000)];
        for (const [index, delay] of delays.entries()) {
          await page.clock.fastForward(delay);
          await expect.poll(() => settled).toBe(index + 2);
          await expect(desktop.getByRole('status')).toContainText('Reconnecting');
        }
        await page.clock.fastForward(28_999);
        await expect(desktop.getByRole('status')).toContainText('Reconnecting');
        await page.clock.runFor(1);
        await expect(
          desktop.getByRole('heading', { name: 'Automatic reconnect attempts have stopped' }),
        ).toBeVisible();
        await capture(page, info, '10-computer-view-lost', theme);
        for (let index = 0; index < 12; index++) {
          const before = sandboxReads;
          await page.clock.fastForward(5_000);
          await expect.poll(() => sandboxReads).toBeGreaterThan(before);
          await expect(
            desktop.getByRole('heading', { name: 'Automatic reconnect attempts have stopped' }),
          ).toBeVisible();
          expect(settled).toBe(14);
        }
        expect(settled).toBe(14);
        expect(stream.connections).toHaveLength(0);
        available = true;
        await desktop.getByRole('button', { name: 'Reconnect view', exact: true }).click();
        await expect.poll(() => settled).toBe(15);
        await expect
          .poll(async () => {
            await page.clock.runFor(50);
            return desktop.getAttribute('data-controller');
          })
          .toBe('view');
        expect(stream.connections).toHaveLength(1);
        await desktop.locator('canvas').click();
        await page.keyboard.press('a');
        expect(stream.inputs).toEqual([]);
        await page.getByRole('button', { name: 'Close computer', exact: true }).click();
        await expect(desktop).toHaveCount(0);
        await page.clock.fastForward(60_000);
        expect(settled).toBe(15);
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });

    test(`LOCAL redesign Skills ${theme} preserves search and version across failure, retry and forbidden catalog`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new SkillCatalogFixture();
      let failure: number | undefined;
      fixture.catalog = (route) =>
        failure
          ? problem(route, failure, failure === 403 ? 'forbidden' : 'profile_skills_unavailable')
          : route.fulfill({
              json: {
                items: [skill],
                next: new URL(route.request().url()).searchParams.has('cursor')
                  ? undefined
                  : 'page-two',
              },
            });
      await appearance(page, theme);
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const modal = await openSkillsModal(page);
      const search = modal.getByRole('textbox', { name: 'Search available Ornn skills' });
      await expect(search).toBeEnabled();
      await search.fill('review');
      await expect.poll(() => fixture.catalogReads.at(-1)?.query).toBe('review');
      const version = modal.getByRole('button', { name: 'Version for Review', exact: true });
      await expect(version).toBeEnabled();
      await version.click();
      await page.getByRole('menuitemradio', { name: '1.1', exact: true }).click();
      failure = 503;
      await modal.getByRole('button', { name: 'More Ornn skills' }).click();
      const retry = modal.getByRole('button', { name: 'Retry skill page' });
      await expect(retry).toBeEnabled();
      await expect(modal.getByText('No matching Ornn skills.')).toHaveCount(0);
      await expect(version).toContainText('1.1');
      await modal.locator('.agent-skills__catalog-failure').scrollIntoViewIfNeeded();
      await capture(page, info, '20-skills', theme);
      failure = undefined;
      await retry.click();
      await expect(version).toBeEnabled();
      await expect(version).toContainText('1.1');
      await expect(search).toHaveValue('review');
      expect(fixture.installs).toEqual([]);
      failure = 403;
      await search.fill('forbidden');
      await expect(modal.getByText(/do not have permission/)).toBeVisible();
      await expect(search).toBeDisabled();
      await expect(modal.getByRole('button', { name: /Retry/ })).toHaveCount(0);
      expect(fixture.installs).toEqual([]);
      fixture.assertReadBoundary();
    });
  }
}

export function registerDesktopMotionCases() {
  for (const reducedMotion of ['no-preference', 'reduce'] as const) {
    test(`LOCAL desktop loading and timed handoffs with motion ${reducedMotion}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
      try {
        await page.setViewportSize({ width: 1600, height: 1000 });
        await page.emulateMedia({ reducedMotion });
        await fixture.route(page, baseURL!);
        const desktopStream = await desktopBoundary(page, false);
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
        await page.goto('/agents/agt_contract');
        await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
        const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
        const control = desktop.getByRole('button', { name: 'Take control', exact: true });
        await expect(desktop.getByRole('status')).toContainText('Loading the desktop screen');
        await expect(control).toBeDisabled();
        await expect(desktop.locator('.agent-desktop__loading')).toBeVisible();
        const shimmer = await desktop
          .locator('.skeleton')
          .first()
          .evaluate((e) => getComputedStyle(e, '::after').animationName);
        expect(shimmer).toBe(reducedMotion === 'reduce' ? 'none' : 'skeleton-shimmer');
        await page.screenshot({ path: info.outputPath(`loading-${reducedMotion}.png`) });
        desktopStream.paint();
        await expect(control).toBeEnabled();
        await expect(desktop.locator('.agent-desktop__loading')).toHaveCount(0);
        await page.clock.install();
        // Leave ample room for real screenshot/protocol work; component tests
        // cover the exact 4,999/5,000ms boundary with a fully stopped clock.
        await control.click();
        await expect(desktop.locator('.agent-desktop__notice')).toContainText('You have control');
        expect(
          await desktop
            .locator('.agent-desktop__notice')
            .evaluate((e) => getComputedStyle(e).pointerEvents),
        ).toBe('none');
        await page.clock.fastForward(3_000);
        await expect(desktop.locator('.agent-desktop__notice')).toBeVisible();
        await desktop.locator('canvas').click();
        await page.keyboard.down('Control');
        await page.mouse.down();
        await expect
          .poll(() =>
            desktopStream.inputs.some((x) => x.kind === 'key' && x.down && x.keysym === 0xffe3),
          )
          .toBe(true);
        fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
        fixture.live('text_delta', 'Using the shared desktop.');
        await expect(desktop.getByRole('status')).toContainText(
          'Agent is in control · Working in Desktop',
        );
        await expect(control).toBeEnabled();
        await expect(desktop.locator('.agent-desktop__notice')).toContainText(
          'The Agent has control',
        );
        await expect
          .poll(() =>
            desktopStream.inputs.some((x) => x.kind === 'key' && !x.down && x.keysym === 0xffe3),
          )
          .toBe(true);
        expect(desktopStream.inputs.filter((x) => x.kind === 'pointer').at(-1)?.buttons).toBe(0);
        await page.mouse.up();
        await page.keyboard.up('Control');
        await page.screenshot({ path: info.outputPath(`handoff-${reducedMotion}.png`) });
        await page.clock.fastForward(3_000);
        await expect(desktop.locator('.agent-desktop__notice')).toBeVisible();
        await page.clock.fastForward(2_400);
        await expect(desktop.locator('.agent-desktop__notice')).toHaveCount(0);
        await expect(desktop.getByRole('status')).toContainText(
          'Agent is in control · Working in Desktop',
        );
        expect(fixture.writes).toEqual([]);
        expect(desktopStream.connections).toHaveLength(1);
      } finally {
        await fixture.close(page);
      }
    });
  }
}
