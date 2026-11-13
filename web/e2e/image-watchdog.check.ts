import { writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';

test('worker exit cannot leave a detached browser behind', async ({ page }) => {
  const ready = process.env.CMA_WEB_WATCHDOG_READY;
  if (!ready) throw new Error('Run with the isolated web-image-watchdog-tests.py supervisor.');
  await page.setContent('<button>Watchdog fixture ready</button>');
  await expect(page.getByRole('button', { name: 'Watchdog fixture ready' })).toBeVisible();
  writeFileSync(ready, JSON.stringify({ worker: process.pid }));
  // Stop only the worker; Chrome remains live in the separate group Playwright creates.
  if (process.env.CMA_WEB_WATCHDOG_MODE === 'stopped') process.kill(process.pid, 'SIGSTOP');
  await page.waitForTimeout(60_000);
});
