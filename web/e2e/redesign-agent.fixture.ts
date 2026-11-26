import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, type Page, type TestInfo } from '@playwright/test';
import { LocalAgentV2, binding, cursor } from './agent-v2.fixture';

export const originalMessage =
  'Review the project and explain the next change. Preserve the original request, including this second line.\n请保留这条消息。';
export type Theme = 'dark' | 'light';
export async function appearance(page: Page, theme: Theme) {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
}
export async function stoppedClock(page: Page) {
  const anchor = new Date('2026-01-02T03:04:05Z').getTime();
  await page.clock.install({ time: anchor - 60_000 });
  await page.clock.pauseAt(anchor);
  await page.addInitScript(() => {
    Math.random = () => 0;
  });
}
export async function liveHead(page: Page, fixture: LocalAgentV2) {
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
}
export function startingAgent(fixture: LocalAgentV2) {
  fixture.boundary.agent = {
    ...fixture.boundary.agent,
    status: 'starting',
    ready_at: undefined,
    owner_title: 'Review the project',
    worktree: { ...fixture.boundary.agent.worktree, status: 'creating' },
  };
  fixture.append({ type: 'message', role: 'user', content: originalMessage });
}
export function workingAgent(fixture: LocalAgentV2) {
  fixture.boundary.agent = {
    ...fixture.boundary.agent,
    owner_title: 'Review the project',
  };
  fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
  fixture.append({ type: 'status_changed', from: 'ready', to: 'working' });
  fixture.append({ type: 'message', role: 'user', content: originalMessage });
  fixture.append({
    type: 'message',
    role: 'assistant',
    content: 'I’m reviewing the project files.',
  });
}
export async function capture(page: Page, info: TestInfo, name: string, theme: Theme) {
  await page.evaluate(() => document.fonts.ready);
  const root = process.env.CMA_REDESIGN_SCREENSHOTS;
  const directory = root ? join(root, theme) : info.outputPath(theme);
  mkdirSync(directory, { recursive: true });
  const file = join(directory, `${name}.png`);
  await page.screenshot({ path: file, animations: 'disabled' });
  writeFileSync(
    `${file}.json`,
    JSON.stringify(
      {
        commit: process.env.CMA_REDESIGN_COMMIT ?? 'working-tree',
        route: page.url(),
        fixture: name,
        theme,
        viewport: page.viewportSize(),
        zoom: await page.evaluate(() => document.documentElement.style.zoom || '100%'),
        reducedMotion: 'reduce',
      },
      null,
      2,
    ),
  );
}
export async function noClipping(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  for (const panel of await page
    .locator(
      '.agent-status-panel, .agent-pending-panel, .computer-recovery-panel, .agent-skills__catalog-failure',
    )
    .all()) {
    if (await panel.isVisible())
      expect(
        await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
      ).toBe(true);
  }
}
export async function openSkillsModal(page: Page) {
  await page.getByRole('button', { name: 'More Agent actions', exact: true }).click();
  await page.getByRole('menuitem', { name: /^Update skills/ }).click();
  return page.getByRole('dialog', { name: 'Choose skills', exact: true });
}
