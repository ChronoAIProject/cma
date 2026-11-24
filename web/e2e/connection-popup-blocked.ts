import { expect } from '@playwright/test';
import type { Harness } from './harness';
import { ConnectionJourney } from './connection-journey';

export async function runPopupBlockedConnection(h: Harness): Promise<void> {
  const journey = new ConnectionJourney(h, 'oauth');
  const page = await h.open('visitor');
  await page.addInitScript(() => {
    window.open = () => null;
  });
  const launch = await h.launch(page, 'missingService');
  await journey.begin(page, launch);
  const result = journey.startResult(page);
  const wizard = page.getByRole('region', { name: journey.wizardName });
  await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
  await result;
  const fallback = wizard.getByRole('link', { name: 'Continue in NyxID', exact: true });
  await expect(fallback).toBeVisible();
  expect([null, '_self']).toContain(await fallback.getAttribute('target'));
  const tabs = page.context().pages().length;
  await h.capture(page, 'fault-popup-blocked');
  await h.identity(async () => {
    await fallback.click();
    await page.waitForURL((url) => url.origin !== h.config.baseURL);
    expect(page.context().pages()).toHaveLength(tabs);
    await h.operators.completeConnection(page, 'oauth');
  });
  await journey.returnFromSameTab(page);
  await page.waitForLoadState('networkidle');
  await h.flush();
  await journey.finish(page, 'popup_blocked');
  expect(page.context().pages()).toHaveLength(tabs);
}
