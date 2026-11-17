import { test, expect } from '@playwright/test';
import { openingHistory } from './agent-open-history.fixture';
import { bindOpeningBuild } from './agent-open-binding';
import { PerformanceObserver } from './ui-performance-observer';

for (const byteBound of [false, true]) {
  test(`LOCAL complete opening ${byteBound ? '4MiB-trimmed' : '205-row'} history`, async ({
    page,
    baseURL,
  }, info) => {
    const recordBinding = await bindOpeningBuild(page);
    const history = await openingHistory(page, baseURL!, byteBound);
    const observed = new PerformanceObserver(page);
    try {
      await observed.delay();
      observed.begin('open');
      await page.goto('/agents/agt_contract', { waitUntil: 'domcontentloaded' });
      await history.waiting;
      await expect(page.getByText('Previous recorded answer', { exact: true })).toBeVisible();
      await expect(page.locator('.agent-surface__history-notice')).toBeVisible();
      expect(history.pages).toHaveLength(1);
      expect(history.pages[0]!.count).toBe(byteBound ? 7 : 200);
      observed.mark('previewWhileHistoryPending');
      history.release();
      await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
      await expect(page.getByText('Recorded history request', { exact: true })).toBeVisible();
      await expect(page.getByText('Previous recorded answer', { exact: true })).toBeVisible();
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await input.fill('Unsent complete-history check');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      await page.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
          ),
      );
      await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
      await expect(page.getByText('Previous recorded answer', { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      observed.mark('openingComplete');
      expect(history.pages).toHaveLength(2);
      expect(history.pages.reduce((sum, item) => sum + item.count, 0)).toBe(history.through);
      expect(history.fixture.writes).toEqual([]);
      expect(history.fixture.errors).toEqual([]);
      expect(history.fixture.boundary.unexpected).toEqual([]);
      expect(observed.errors).toEqual([]);
      await info.attach('history-completion', {
        contentType: 'application/json',
        body: JSON.stringify(
          {
            through: history.through,
            pages: history.pages,
            completeMs: observed.marks.openingComplete! - observed.marks.openStart!,
            scope:
              'Compiled local behavior; continuation deliberately held; no latency acceptance.',
          },
          null,
          2,
        ),
      });
    } finally {
      history.release();
      try {
        await recordBinding(info);
        await observed.save(info);
      } finally {
        await history.fixture.close(page);
      }
    }
  });
}
