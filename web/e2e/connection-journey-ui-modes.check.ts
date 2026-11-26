import { test, expect } from '@playwright/test';
import { ConnectionJourney } from './connection-journey';
import { assertConnectionReplay, connectionStartPath } from './connection-journey-contract';
import { connectionWizardFixture } from './connection-journey-ui.fixture';
import { auditWizardProof } from './connection-journey-ui-audit.fixture';
import { providerInput } from './connection-journey-ui-provider.fixture';

for (const mode of ['api_key', 'device_code'] as const) {
  test(`LOCAL compiled ConnectionWizard binds the ${mode} service fixture`, async ({
    browser,
    baseURL,
  }, info) => {
    const fixture = await connectionWizardFixture(
      browser,
      baseURL!,
      `J4-${mode}`,
      info.config.metadata.connectionWizardBuild,
      mode,
    );
    const { h } = fixture;
    try {
      const page = await fixture.open();
      const journey = new ConnectionJourney(h, mode);
      const launch = await h.launch(page, 'missingService');
      await journey.begin(page, launch);
      const wizard = page.getByRole('region', { name: journey.wizardName, exact: true });
      await expect(wizard.getByText(`Fixture ${mode} account`, { exact: true })).toBeVisible();
      const popupResult = page.waitForEvent('popup');
      const startResult = journey.startResult(page);
      await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
      const [popup, original] = await Promise.all([popupResult, startResult]);
      expect(original.service_id).toBe(h.fixture.connectionJourneys.services[mode].serviceId);
      expect(fixture.admissions).toHaveLength(1);
      expect(fixture.admissions[0]!.intent).toEqual(original);
      expect([...fixture.links.values()]).toEqual(['pending']);
      expect(fixture.starts).toBe(0);
      const input = providerInput[mode];
      await expect(popup.getByLabel(input.label, { exact: true })).toBeVisible();
      await popup.getByLabel(input.label, { exact: true }).fill('invalid local value');
      await popup.getByRole('button', { name: input.action, exact: true }).click();
      await expect(popup.getByRole('alert')).toHaveText('Fixture input refused');
      expect(fixture.providerSubmissions).toEqual([{ mode, accepted: false }]);
      expect([...fixture.links.values()]).toEqual(['pending']);
      expect(fixture.starts).toBe(0);
      const verification = journey.verification(page);
      await popup.getByLabel(input.label, { exact: true }).fill(input.value);
      await popup.getByRole('button', { name: input.action, exact: true }).click();
      await verification;
      await expect(page).toHaveURL(new RegExp(`/agents/${fixture.boundary.agent.id}$`));
      await expect(page.locator('#agent-message')).toBeVisible();
      await expect.poll(() => popup.isClosed()).toBe(true);
      await page.waitForLoadState('networkidle');
      await h.flush();
      await journey.finish(page, 'valid');
      await h.flush();
      expect(fixture.creates).toBe(1);
      expect(fixture.starts).toBe(1);
      expect(fixture.startRequests).toHaveLength(1);
      expect(fixture.admissions).toHaveLength(1);
      expect([...fixture.links.values()]).toEqual(['completed']);
      expect(fixture.providerSubmissions).toEqual([
        { mode, accepted: false },
        { mode, accepted: true },
      ]);
      expect(h.violations).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      await auditWizardProof(h, info);
    } finally {
      await fixture.close();
    }
  });
}

test('LOCAL compiled ConnectionWizard retries a controlled upstream refusal explicitly', async ({
  browser,
  baseURL,
}, info) => {
  const fixture = await connectionWizardFixture(
    browser,
    baseURL!,
    'FAULT-nyxid',
    info.config.metadata.connectionWizardBuild,
  );
  const { h } = fixture;
  try {
    fixture.upstreamUnavailable = true;
    const page = await fixture.open();
    const journey = new ConnectionJourney(h, 'oauth');
    const launch = await h.launch(page, 'missingService');
    await journey.begin(page, launch);
    const wizard = page.getByRole('region', { name: journey.wizardName, exact: true });
    const popupResult = page.waitForEvent('popup');
    const refused = page.waitForResponse(
      (response) =>
        new URL(response.url()).origin === h.config.baseURL &&
        new URL(response.url()).pathname === connectionStartPath &&
        response.request().method() === 'POST',
    );
    await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
    const [popup, response] = await Promise.all([popupResult, refused]);
    expect(response.status()).toBe(503);
    await expect(wizard.getByRole('alert')).toBeVisible();
    expect(fixture.startRequests).toHaveLength(1);
    expect(fixture.admissions).toHaveLength(0);
    expect(fixture.links.size).toBe(0);
    expect(fixture.starts).toBe(0);
    const checks = fixture.checks.length;
    await expect.poll(() => fixture.checks.length).toBeGreaterThan(checks);
    expect(fixture.startRequests).toHaveLength(1);
    expect(fixture.links.size).toBe(0);
    expect(fixture.starts).toBe(0);
    const original = [...h.commands];
    fixture.upstreamUnavailable = false;
    const retry = journey.startResult(page);
    await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
    await retry;
    assertConnectionReplay(original, h.commands);
    expect(fixture.startRequests).toHaveLength(2);
    expect(fixture.startRequests[1]!.bytes).toBe(fixture.startRequests[0]!.bytes);
    expect(fixture.admissions).toHaveLength(1);
    expect(fixture.links.size).toBe(1);
    const verification = journey.verification(page);
    await popup.getByRole('link', { name: 'Complete connection', exact: true }).click();
    await verification;
    await expect(page).toHaveURL(new RegExp(`/agents/${fixture.boundary.agent.id}$`));
    await expect(page.locator('#agent-message')).toBeVisible();
    await expect.poll(() => popup.isClosed()).toBe(true);
    await page.waitForLoadState('networkidle');
    await h.flush();
    await journey.finish(page, 'upstream_outage');
    await h.flush();
    expect(fixture.creates).toBe(1);
    expect(fixture.starts).toBe(1);
    expect([...fixture.links.values()]).toEqual(['completed']);
    expect(fixture.providerSubmissions).toEqual([{ mode: 'oauth', accepted: true }]);
    expect(h.violations).toEqual([]);
    expect(fixture.unexpected).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    await auditWizardProof(h, info);
  } finally {
    fixture.upstreamUnavailable = false;
    await fixture.close();
  }
});
