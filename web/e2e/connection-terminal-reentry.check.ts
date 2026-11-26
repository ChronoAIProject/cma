import { test, expect } from '@playwright/test';
import { ConnectionJourney } from './connection-journey';
import { connectionCheckPath } from './connection-journey-contract';
import { connectionWizardFixture } from './connection-journey-ui.fixture';

for (const state of ['expired', 'failed', 'cancelled'] as const) {
  test(`LOCAL compiled ${state} connection requires explicit continuation despite ready services`, async ({
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
      const started = journey.startResult(page);
      await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
      const original = await started;
      await expect(
        wizard.getByRole('link', { name: 'Continue in NyxID', exact: true }),
      ).toBeVisible();

      const terminal = page.waitForResponse(async (response) => {
        if (
          new URL(response.url()).pathname !== connectionCheckPath ||
          response.request().method() !== 'POST'
        )
          return false;
        const report = await response.json();
        return report.ready === true && report.active_state === state;
      });
      fixture.terminalState = state;
      fixture.independentlyConnected = true;
      const response = await terminal;
      expect(response.request().postDataJSON()).toEqual({
        target: original.target,
        active: { service_id: original.service_id, intent_id: original.intent_id },
      });
      expect(await response.json()).toMatchObject({
        owner: h.fixture.accounts.visitor.subject,
        ready: true,
        active_state: state,
        services: [{ service: original.service_id, state: 'satisfied' }],
      });
      await expect(wizard.getByRole('alert')).toHaveText(
        'This sign-in did not finish. Connect again to retry.',
      );
      expect(fixture.starts).toBe(0);
      await expect(
        wizard.getByRole('link', { name: 'Continue in NyxID', exact: true }),
      ).toHaveCount(0);

      const resume = wizard.getByRole('button', { name: 'Continue', exact: true });
      await wizard.getByRole('button', { name: 'Verify again', exact: true }).click();
      for (const reload of [false, true]) {
        if (reload) await page.reload();
        await expect(resume).toBeEnabled();
        await expect(wizard).toContainText(
          'Your original action is waiting for your confirmation.',
        );
        const verified = await page.waitForResponse(
          (response) =>
            new URL(response.url()).pathname === connectionCheckPath &&
            response.request().method() === 'POST',
        );
        expect(verified.request().postDataJSON()).toEqual({ target: original.target });
        expect(await verified.json()).toMatchObject({
          ready: true,
          owner: h.fixture.accounts.visitor.subject,
        });
        await expect(resume).toBeVisible();
        await expect(page).toHaveURL(`${h.config.baseURL}/launches/${launch.id}`);
        expect(fixture.starts).toBe(0);
        expect(fixture.creates).toBe(1);
        expect(fixture.startRequests).toHaveLength(1);
        expect(fixture.admissions).toHaveLength(1);
        expect(fixture.cancellations).toEqual([]);
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
      expect(fixture.cancellations).toEqual([]);
      expect([...fixture.links.values()]).toEqual(['pending']);
      expect(fixture.providerSubmissions).toEqual([]);
      expect(page.context().pages()).toEqual([page]);
      expect(
        h.commands.filter((row) => row.path.endsWith(`/launches/${launch.id}/start`)),
      ).toHaveLength(1);
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
