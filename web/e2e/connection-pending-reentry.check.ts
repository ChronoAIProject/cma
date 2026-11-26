import { test, expect } from '@playwright/test';
import { ConnectionJourney } from './connection-journey';
import { connectionCheckPath } from './connection-journey-contract';
import { auditWizardProof } from './connection-journey-ui-audit.fixture';
import { connectionWizardFixture } from './connection-journey-ui.fixture';

test('LOCAL compiled pending intent survives ready services, reload and re-entry', async ({
  browser,
  baseURL,
}, info) => {
  const fixture = await connectionWizardFixture(
    browser,
    baseURL!,
    'FAULT-popup-blocked',
    info.config.metadata.connectionWizardBuild,
  );
  const { h } = fixture;
  try {
    const page = await fixture.open(true);
    const journey = new ConnectionJourney(h, 'oauth');
    const launch = await h.launch(page, 'missingService');
    await journey.begin(page, launch);
    const wizard = page.getByRole('region', { name: journey.wizardName, exact: true });
    const started = journey.startResult(page);
    await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
    const original = await started;
    const fallback = wizard.getByRole('link', { name: 'Continue in NyxID', exact: true });
    await expect(fallback).toBeVisible();
    const hostedUrl = await fallback.getAttribute('href');
    expect(hostedUrl).toBe(`https://identity.example.test/connect/${original.intent_id}`);
    const originalBytes = fixture.admissions[0]!.bytes;

    // Another valid connection can satisfy the service while this handoff is pending.
    await page.route(`**${connectionCheckPath}`, async (route) => {
      const response = await route.fetch();
      const report = await response.json();
      if (report.active_state !== 'pending') return route.fulfill({ response });
      expect(route.request().postDataJSON()).toEqual({
        target: original.target,
        active: { service_id: original.service_id, intent_id: original.intent_id },
      });
      await route.fulfill({
        response,
        json: {
          ...report,
          ready: true,
          services: report.services.map((row: { state: string }) => ({
            ...row,
            state: 'satisfied',
          })),
        },
      });
    });

    for (const transition of ['initial', 'reload', 're-entry']) {
      const pending = page.waitForResponse(async (response) => {
        if (
          new URL(response.url()).pathname !== connectionCheckPath ||
          response.request().method() !== 'POST'
        )
          return false;
        const report = await response.json();
        return report.ready === true && report.active_state === 'pending';
      });
      if (transition === 'reload') await page.reload();
      if (transition === 're-entry') {
        await page.goto(h.config.baseURL);
        // Let the browser audit finish reading Home's API responses before the next navigation.
        await page.waitForLoadState('networkidle');
        await h.flush();
        await page.goto(`${h.config.baseURL}/launches/${launch.id}`);
      }
      const response = await pending;
      expect(response.request().postDataJSON()).toEqual({
        target: original.target,
        active: { service_id: original.service_id, intent_id: original.intent_id },
      });
      expect(await response.json()).toMatchObject({
        owner: h.fixture.accounts.visitor.subject,
        ready: true,
        active_state: 'pending',
        services: [{ service: original.service_id, state: 'satisfied' }],
      });
      await expect(
        wizard.getByRole('status').filter({ hasText: /connections verified$/ }),
      ).toHaveText('1 of 1 connections verified');
      await expect(fallback).toHaveAttribute('href', hostedUrl!);
      await expect(fallback).toBeVisible();
      await expect(wizard.getByRole('button', { name: 'Continue', exact: true })).toHaveCount(0);
      await expect(page).toHaveURL(`${h.config.baseURL}/launches/${launch.id}`);
      expect(fixture.creates).toBe(1);
      expect(fixture.starts).toBe(0);
      expect(fixture.startRequests).toHaveLength(1);
      expect(fixture.admissions).toEqual([{ bytes: originalBytes, intent: original }]);
      expect(fixture.cancellations).toEqual([]);
      expect([...fixture.links]).toEqual([[original.intent_id, 'pending']]);
      expect(fixture.providerSubmissions).toEqual([]);
      expect(page.context().pages()).toEqual([page]);
      await h.flush();
      await h.attachJourneyProof(info);
      expect(info.attachments).toEqual([]);
    }

    expect([null, '_self']).toContain(await fallback.getAttribute('target'));
    await fallback.click();
    await expect(page).toHaveURL(hostedUrl!);
    await page.getByRole('link', { name: 'Complete connection', exact: true }).click();
    await expect(page).toHaveURL(`${h.config.baseURL}/connection-return?status=completed`);
    expect(fixture.starts).toBe(0);
    await journey.returnFromSameTab(page);
    await expect(page).toHaveURL(`${h.config.baseURL}/agents/${fixture.boundary.agent.id}`);
    await expect(page.locator('#agent-message')).toBeVisible();
    await page.waitForLoadState('networkidle');
    await h.flush();
    await journey.finish(page, 'popup_blocked');
    await h.flush();
    expect(fixture.creates).toBe(1);
    expect(fixture.starts).toBe(1);
    expect(fixture.startRequests).toHaveLength(1);
    expect(fixture.admissions).toEqual([{ bytes: originalBytes, intent: original }]);
    expect(fixture.cancellations).toEqual([]);
    expect([...fixture.links]).toEqual([[original.intent_id, 'completed']]);
    expect(fixture.providerSubmissions).toEqual([{ mode: 'oauth', accepted: true }]);
    expect(page.context().pages()).toEqual([page]);
    expect(
      h.commands.filter((row) => row.path.endsWith(`/launches/${launch.id}/start`)),
    ).toHaveLength(1);
    expect(h.violations).toEqual([]);
    expect(fixture.unexpected).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    await auditWizardProof(h, info);
    expect(info.attachments).toHaveLength(1);
  } finally {
    await fixture.close();
  }
});
