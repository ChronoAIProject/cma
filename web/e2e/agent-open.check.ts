import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';
import { PerformanceObserver } from './ui-performance-observer';
import {
  observeAgentOpening,
  agentOpeningTimings,
  agentOpeningResources,
} from './agent-open-measurements';
import { writeFile } from 'node:fs/promises';
import { bindOpeningBuild } from './agent-open-binding';

for (const mode of [
  'cold',
  'list',
  'hover',
  'retained',
  'slow-history',
  'slow-sandbox',
  'slow-connections',
] as const) {
  test(`LOCAL Agent opening ${mode}`, async ({ page, baseURL }, info) => {
    const fixture = new LocalAgentV2();
    const observed = new PerformanceObserver(page);
    const recordBinding = await bindOpeningBuild(page);
    try {
      await observeAgentOpening(page);
      fixture.append({ type: 'message', role: 'assistant', content: 'Previous recorded answer' });
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/ui/task-experience', (route) =>
        route.fulfill({ status: 404, json: { code: 'not_found' } }),
      );
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(1),
            active_turn: null,
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_contract',
            through_seq: 1,
            turns: [
              {
                turn_sequence: 1,
                started_at: fixture.started,
                owner_message: '',
                agent_messages: ['Previous recorded answer'],
                messages: [
                  {
                    sequence: 1,
                    role: 'assistant',
                    content: 'Previous recorded answer',
                    appended_at: fixture.started,
                  },
                ],
                outcome: { type: 'completed', at: fixture.started },
              },
            ],
          },
        }),
      );
      await page.route(/\/(api|auth)\//, async (route) => {
        const path = new URL(route.request().url()).pathname;
        const slow =
          (mode === 'slow-history' && /transcript|events\/snapshot/.test(path)) ||
          (mode === 'slow-sandbox' && path === '/api/v1/sandboxes/sbx_route') ||
          (mode === 'slow-connections' && path === '/auth/connections/check');
        await new Promise((resolve) => setTimeout(resolve, slow ? 2000 : 100));
        await route.fallback();
      });
      if (mode === 'list' || mode === 'hover' || mode === 'retained') {
        if (mode === 'retained') {
          await page.goto('/agents/agt_contract', { waitUntil: 'domcontentloaded' });
          await expect(page.getByText('Previous recorded answer', { exact: true })).toBeVisible();
          await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
          await page.getByRole('link', { name: 'CMA home', exact: true }).click();
        } else await page.goto('/', { waitUntil: 'domcontentloaded' });
        const link = page.locator('.agent-navigation a[href="/agents/agt_contract"]');
        await expect(link).toBeVisible();
        if (mode === 'hover') {
          observed.begin('intent');
          await link.hover();
          await page.waitForTimeout(500);
        }
        observed.begin('open');
        await link.click();
      } else {
        observed.begin('open');
        await page.goto('/agents/agt_contract', { waitUntil: 'domcontentloaded' });
      }
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      // Observe publication each frame instead of adding assertion retry backoff
      // to the elapsed clock. Keep the same visibility assertion and deadline.
      await page.waitForFunction(
        () => !!document.querySelector('.agent-surface textarea')?.getClientRects().length,
        undefined,
        { timeout: 5000 },
      );
      await expect(input).toBeVisible();
      // Two frames include the browser's rendering opportunity after DOM publication.
      await page.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
          ),
      );
      observed.mark('meaningfulPaint');
      await input.fill('Unsent opening measurement');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      observed.mark('inputReady');
      await expect(page.getByText('Previous recorded answer', { exact: true })).toBeVisible();
      observed.mark('content');
      await page.waitForFunction(
        () => document.querySelector('.agent-surface__history-notice') === null,
        undefined,
        { timeout: 5000 },
      );
      await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
      await page.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
          ),
      );
      // The completion boundary observes all content and control predicates in
      // one browser task after rendering, rather than retaining earlier flags.
      expect(
        await page.evaluate(() => {
          const surface = document.querySelector('.agent-surface');
          const editor = surface?.querySelector('textarea');
          const send = surface?.querySelector<HTMLButtonElement>(
            'button[aria-label="Send message"]',
          );
          return {
            visible: !!editor?.getClientRects().length,
            enabled: !!editor && !editor.disabled && !!send && !send.disabled,
            draft: editor?.value,
            content: surface?.textContent?.includes('Previous recorded answer'),
            historyComplete: surface?.querySelector('.agent-surface__history-notice') === null,
          };
        }),
      ).toEqual({
        visible: true,
        enabled: true,
        draft: 'Unsent opening measurement',
        content: true,
        historyComplete: true,
      });
      observed.mark('openingComplete');
      await expect.poll(() => fixture.streams.size).toBe(1);
      observed.mark('liveReady');
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(observed.errors).toEqual([]);
      const timings = await agentOpeningTimings(page);
      if (mode === 'slow-connections') {
        // Connection verification fences Send; it must not hold completed content.
        expect(timings.content).toBeLessThan(timings.inputReady!);
        expect(timings.historyComplete).toBeLessThan(timings.inputReady!);
      }
      const openingPath = info.outputPath('opening.json');
      await writeFile(
        openingPath,
        JSON.stringify(
          {
            mode,
            source: process.env.CMA_OPEN_SOURCE,
            browser: await page.context().browser()?.version(),
            delayMs: 100,
            slowDelayMs: mode.startsWith('slow') ? 2000 : undefined,
            browserTimings: timings,
            resources: await agentOpeningResources(page),
            assertionTimings: Object.fromEntries(
              Object.entries(observed.marks).map(([name, value]) => [
                name,
                value - observed.marks.openStart!,
              ]),
            ),
          },
          null,
          2,
        ),
      );
      await info.attach('opening', { path: openingPath, contentType: 'application/json' });
      // Injected two-second dependencies exercise behavior, not the warm fixture
      // budget. Preserve their full timings without redefining completion.
      if (!mode.startsWith('slow-'))
        expect(observed.marks.openingComplete! - observed.marks.openStart!).toBeLessThanOrEqual(
          1000,
        );
    } finally {
      try {
        await recordBinding(info);
        await observed.save(info);
      } finally {
        await fixture.close(page);
      }
    }
  });
}
