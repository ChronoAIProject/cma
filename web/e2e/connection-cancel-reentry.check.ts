import { test, expect } from '@playwright/test';
import { ConnectionJourney } from './connection-journey';
import { connectionCheckPath } from './connection-journey-contract';
import { connectionWizardFixture } from './connection-journey-ui.fixture';

for (const reply of ['acknowledged', 'lost'] as const) {
  test(`LOCAL compiled popup-blocked cancellation survives re-entry and reload with ${reply} reply`, async ({
    browser,
    baseURL,
  }, info) => {
    const fixture = await connectionWizardFixture(
      browser,
      baseURL!,
      'J4-oauth',
      info.config.metadata.connectionWizardBuild,
    );
    const { h } = fixture;
    try {
      const page = await fixture.open(true);
      const journey = new ConnectionJourney(h, 'oauth');
      const launch = await h.launch(page, 'missingService');
      await journey.begin(page, launch);
      const wizard = page.getByRole('region', { name: journey.wizardName, exact: true });
      const startResult = journey.startResult(page);
      await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
      const original = await startResult;
      await expect(
        wizard.getByRole('link', { name: 'Continue in NyxID', exact: true }),
      ).toBeVisible();
      expect(page.context().pages()).toEqual([page]);
      expect(fixture.starts).toBe(0);
      const cancelPath = '/auth/connections/cancel';
      if (reply === 'lost') {
        await page.route(`**${cancelPath}`, async (route) => {
          const response = await route.fetch();
          expect(response.ok()).toBe(true);
          expect(fixture.cancellations).toHaveLength(1);
          await route.abort('connectionfailed');
        });
      }
      const cancellation =
        reply === 'lost'
          ? page.waitForEvent(
              'requestfailed',
              (request) => new URL(request.url()).pathname === cancelPath,
            )
          : page.waitForResponse((response) => new URL(response.url()).pathname === cancelPath);
      await wizard.getByRole('button', { name: 'Cancel', exact: true }).click();
      await cancellation;
      await expect(page).toHaveURL(`${h.config.baseURL}/t/public-fixture/placement-fixture`);
      expect(page.context().pages()).toEqual([page]);
      expect(fixture.cancellations).toEqual([
        {
          target: original.target,
          active: { service_id: original.service_id, intent_id: original.intent_id },
        },
      ]);

      // A different valid connection can satisfy readiness without completing this intent.
      fixture.independentlyConnected = true;
      await page.goto(`${h.config.baseURL}/launches/${launch.id}`);
      const resume = wizard.getByRole('button', { name: 'Continue', exact: true });
      for (const reload of [false, true]) {
        if (reload) await page.reload();
        await expect(resume).toBeVisible();
        await expect(wizard).toContainText(
          'Your original action is waiting for your confirmation.',
        );
        const fresh = await page.waitForResponse(
          (response) =>
            new URL(response.url()).pathname === connectionCheckPath &&
            response.request().method() === 'POST',
        );
        expect(fresh.request().postDataJSON()).toEqual({ target: original.target });
        expect(await fresh.json()).toMatchObject({
          ready: true,
          owner: h.fixture.accounts.visitor.subject,
        });
        await expect(resume).toBeEnabled();
        await expect(page).toHaveURL(`${h.config.baseURL}/launches/${launch.id}`);
        expect(fixture.starts).toBe(0);
        expect(fixture.startRequests).toHaveLength(1);
        expect(fixture.admissions).toHaveLength(1);
        expect(fixture.cancellations).toHaveLength(1);
        expect([...fixture.links.values()]).toEqual(['pending']);
        await h.flush();
        await h.attachJourneyProof(info);
        expect(info.attachments).toEqual([]);
      }
      await resume.click();
      await expect(page).toHaveURL(`${h.config.baseURL}/agents/${fixture.boundary.agent.id}`);
      await expect(page.locator('#agent-message')).toBeVisible();
      await page.waitForLoadState('networkidle');
      await h.flush();
      expect(fixture.creates).toBe(1);
      expect(fixture.starts).toBe(1);
      expect(fixture.startRequests).toHaveLength(1);
      expect(fixture.admissions).toHaveLength(1);
      expect(fixture.cancellations).toHaveLength(1);
      expect([...fixture.links.values()]).toEqual(['pending']);
      expect(fixture.providerSubmissions).toEqual([]);
      expect(page.context().pages()).toEqual([page]);
      expect(
        h.commands.filter((row) => row.path.endsWith(`/launches/${launch.id}/start`)),
      ).toHaveLength(1);
      expect(h.commands.filter((row) => row.path === cancelPath)).toHaveLength(1);
      await h.attachJourneyProof(info);
      expect(info.attachments).toEqual([]);
      expect(h.violations).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
    } finally {
      await fixture.close();
    }
  });
}
