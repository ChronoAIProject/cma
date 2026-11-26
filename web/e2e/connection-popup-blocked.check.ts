import { test, expect } from '@playwright/test';
import { connectionWizardFixture } from './connection-journey-ui.fixture';
import { auditWizardProof } from './connection-journey-ui-audit.fixture';
import { runPopupBlockedConnection } from './connection-popup-blocked';

for (const scenario of ['valid', 'missing-link'] as const) {
  test(`LOCAL compiled staging popup-blocked driver: ${scenario}`, async ({
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
    let callbackObserved = false;
    let removed = false;
    let returnClicks = 0;
    try {
      const page = await fixture.open();
      const tabs = page.context().pages().length;
      await page.exposeFunction('recordCallbackReturnClick', () => {
        returnClicks += 1;
      });
      await page.addInitScript(() => {
        document.addEventListener('click', (event) => {
          if (window.location.pathname !== '/connection-return') return;
          const target = event.target as Element | null;
          if (target?.closest('a')?.textContent === 'Return to CMA') {
            void (
              window as unknown as { recordCallbackReturnClick(): Promise<void> }
            ).recordCallbackReturnClick();
          }
        });
      });
      h.open = async (role) => {
        expect(role).toBe('visitor');
        return page;
      };
      h.operators.completeConnection = async (providerPage, mode) => {
        expect(providerPage).toBe(page);
        expect(mode).toBe('oauth');
        await providerPage.getByRole('link', { name: 'Complete connection', exact: true }).click();
        await expect(providerPage).toHaveURL(
          `${h.config.baseURL}/connection-return?status=completed`,
        );
        callbackObserved = true;
        expect(fixture.starts).toBe(0);
        const link = providerPage.getByRole('link', { name: 'Return to CMA', exact: true });
        await expect(link).toBeVisible();
        if (scenario === 'missing-link') {
          await link.evaluate((element) => element.remove());
          await expect(link).toHaveCount(0);
          removed = true;
        }
      };

      if (scenario === 'missing-link') {
        await expect(runPopupBlockedConnection(h)).rejects.toThrow(/Return to CMA|toBeVisible/);
        expect(callbackObserved).toBe(true);
        expect(removed).toBe(true);
        expect(fixture.starts).toBe(0);
        expect(fixture.creates).toBe(1);
        expect(fixture.admissions).toHaveLength(1);
        expect([...fixture.links.values()]).toEqual(['completed']);
        await h.attachJourneyProof(info);
        expect(info.attachments).toEqual([]);
      } else {
        await runPopupBlockedConnection(h);
        expect(callbackObserved).toBe(true);
        expect(removed).toBe(false);
        expect(fixture.creates).toBe(1);
        expect(fixture.starts).toBe(1);
        expect(fixture.admissions).toHaveLength(1);
        expect([...fixture.links.values()]).toEqual(['completed']);
        await h.flush();
        await auditWizardProof(h, info);
        expect(info.attachments).toHaveLength(1);
      }
      expect(returnClicks).toBe(scenario === 'valid' ? 1 : 0);
      expect(page.context().pages()).toHaveLength(tabs);
      expect(h.violations).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
    } finally {
      await fixture.close();
    }
  });
}
