import { appendFileSync, renameSync, writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';

function startup(event: string) {
  const path = process.env.CMA_WEB_WATCHDOG_STARTUP;
  if (path)
    appendFileSync(
      path,
      `${JSON.stringify({ event, pid: process.pid, at: new Date().toISOString() })}\n`,
    );
}

startup(
  process.env.TEST_WORKER_INDEX === undefined ? 'runner-loaded-fixture' : 'worker-loaded-fixture',
);

test('worker exit cannot leave a detached browser behind', async ({ page }) => {
  const ready = process.env.CMA_WEB_WATCHDOG_READY;
  if (!ready) throw new Error('Run with the isolated web-image-watchdog-tests.py supervisor.');
  startup('page-fixture-ready');
  if (process.env.CMA_WEB_WATCHDOG_MODE === 'pre-ready-stall') {
    startup('injected-pre-ready-stall');
    await page.waitForTimeout(60_000);
  }
  if (process.env.CMA_WEB_WATCHDOG_MODE === 'slow-start') {
    startup('injected-slow-start');
    await page.waitForTimeout(27_000);
  }
  startup('set-content-begin');
  await page.setContent('<button>Watchdog fixture ready</button>');
  startup('visible-assertion-begin');
  await expect(page.getByRole('button', { name: 'Watchdog fixture ready' })).toBeVisible();
  startup('fixture-ready');
  writeFileSync(`${ready}.tmp`, JSON.stringify({ worker: process.pid }));
  renameSync(`${ready}.tmp`, ready);
  if (process.env.CMA_WEB_WATCHDOG_MODE === 'phase-pass') return;
  if (process.env.CMA_WEB_WATCHDOG_MODE === 'phase-fail')
    throw new Error('Injected trace probe failure');
  // The observer injects worker failure only after the supervisor acknowledges
  // readiness and arms its execution deadline. Chrome stays in its detached group.
  await page.waitForTimeout(60_000);
});
