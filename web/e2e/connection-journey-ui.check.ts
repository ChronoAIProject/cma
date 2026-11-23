import assert from 'node:assert/strict';
import { test, expect } from '@playwright/test';
import { ConnectionJourney } from './connection-journey';
import { assertConnectionReplay, connectionStartPath } from './connection-journey-contract';
import { connectionWizardFixture } from './connection-journey-ui.fixture';
import { auditWizardProof } from './connection-journey-ui-audit.fixture';

for (const [caseId, variant] of [
  ['J4-oauth', 'valid'],
  ['FAULT-popup-blocked', 'popup_blocked'],
  ['FAULT-handoff-response', 'start_response_loss'],
] as const) {
  test(`LOCAL compiled ConnectionWizard produces audited ${variant} evidence`, async ({
    browser,
    baseURL,
  }, info) => {
    const fixture = await connectionWizardFixture(
      browser,
      baseURL!,
      caseId,
      info.config.metadata.connectionWizardBuild,
    );
    const { h } = fixture;
    try {
      const page = await fixture.open(variant === 'popup_blocked');
      const journey = new ConnectionJourney(h, 'oauth');
      const launch = await h.launch(page, 'missingService');
      await journey.begin(page, launch);
      expect(h.journal.resources).toEqual([
        { kind: 'launches', id: launch.id, subject: h.fixture.accounts.visitor.subject },
      ]);
      const wizard = page.getByRole('region', { name: journey.wizardName, exact: true });
      const tabCount = page.context().pages().length;
      let admittedBeforeLoss = false;
      if (variant === 'start_response_loss') {
        await page.route(`**${connectionStartPath}`, async (route) => {
          if (admittedBeforeLoss) return route.fallback();
          const response = await route.fetch();
          expect(response.ok()).toBe(true);
          expect(fixture.links.size).toBe(1);
          journey.readStart(route.request().postDataJSON(), await response.json());
          admittedBeforeLoss = true;
          await route.abort('connectionfailed');
        });
      }
      const popupPromise = variant === 'popup_blocked' ? undefined : page.waitForEvent('popup');
      const firstResult = variant === 'start_response_loss' ? undefined : journey.startResult(page);
      await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
      await firstResult;
      const popup = await popupPromise;
      if (variant === 'start_response_loss') {
        await expect(wizard.getByRole('alert')).toBeVisible();
        expect(admittedBeforeLoss).toBe(true);
        const before = [...h.commands];
        expect(fixture.admissions).toHaveLength(1);
        expect(fixture.starts).toBe(0);
        const retry = journey.startResult(page);
        await wizard.getByRole('button', { name: 'Connect all', exact: true }).click();
        await retry;
        assertConnectionReplay(before, h.commands);
        expect(fixture.admissions).toHaveLength(2);
        expect(fixture.admissions[1]!.bytes).toBe(fixture.admissions[0]!.bytes);
        expect(fixture.links.size).toBe(1);
      }
      if (variant === 'popup_blocked') {
        const fallback = wizard.getByRole('link', { name: 'Continue in NyxID', exact: true });
        await expect(fallback).toBeVisible();
        expect([null, '_self']).toContain(await fallback.getAttribute('target'));
        await fallback.click();
        await expect(page).toHaveURL(/https:\/\/identity\.example\.test\/connect\//);
        expect(page.context().pages()).toHaveLength(tabCount);
        await page.getByRole('link', { name: 'Complete connection', exact: true }).click();
        await expect(page).toHaveURL(/\/connection-return\?status=completed$/);
        expect(fixture.starts).toBe(0);
        const returnLink = page.getByRole('link', { name: 'Return to CMA', exact: true });
        await expect(returnLink).toHaveAttribute('href', `/launches/${launch.id}`);
        expect([null, '_self']).toContain(await returnLink.getAttribute('target'));
        await journey.returnFromSameTab(page);
      } else {
        const verification = journey.verification(page);
        await popup!.getByRole('link', { name: 'Complete connection', exact: true }).click();
        await verification;
      }
      await expect(page).toHaveURL(new RegExp(`/agents/${fixture.boundary.agent.id}$`));
      await expect(page.locator('#agent-message')).toBeVisible();
      await page.waitForLoadState('networkidle');
      await h.flush();
      await journey.finish(page, variant);
      await page.waitForLoadState('networkidle');
      await h.flush();
      expect(fixture.creates).toBe(1);
      expect(fixture.starts).toBe(1);
      expect(fixture.links.size).toBe(1);
      expect([...fixture.links.values()]).toEqual(['completed']);
      expect(
        fixture.admissions.every(
          ({ intent }) => JSON.stringify(intent.target) === JSON.stringify(fixture.target),
        ),
      ).toBe(true);
      if (variant === 'popup_blocked') {
        expect(page.context().pages()).toHaveLength(tabCount);
        expect(fixture.admissions).toHaveLength(1);
      } else await expect.poll(() => popup!.isClosed()).toBe(true);
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

for (const changed of ['owner', 'intent', 'service'] as const) {
  test(`LOCAL compiled ConnectionWizard and helper reject wrong ${changed}`, async ({
    browser,
    baseURL,
  }, info) => {
    const fixture = await connectionWizardFixture(
      browser,
      baseURL!,
      'J4-oauth',
      info.config.metadata.connectionWizardBuild,
    );
    try {
      fixture.invalidReply = changed;
      const page = await fixture.open();
      const journey = new ConnectionJourney(fixture.h, 'oauth');
      await journey.begin(page, await fixture.h.launch(page, 'missingService'));
      const rejected = assert.rejects(
        journey.startResult(page),
        new RegExp(`Connection ${changed} changed`),
      );
      await page.getByRole('button', { name: 'Connect all', exact: true }).click();
      await rejected;
      await expect(page.getByRole('alert')).toHaveText(
        'Account changed. Reopen this connection step to continue.',
      );
      expect(fixture.starts).toBe(0);
      expect([...fixture.links.values()]).toEqual(['pending']);
      await fixture.h.attachJourneyProof(info);
      expect(info.attachments).toEqual([]);
      expect(fixture.h.violations).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
    } finally {
      await fixture.close();
    }
  });
}
