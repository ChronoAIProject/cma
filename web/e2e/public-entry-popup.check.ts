import { expect, test, type Page } from '@playwright/test';
import { placementPath, popupFixture } from './public-entry-popup.fixture';

const start = (page: Page) => page.getByRole('button', { name: 'Try it', exact: true });
const signIn = (page: Page) => page.getByRole('button', { name: 'Sign in to CMA', exact: true });
const checkAccount = (page: Page) =>
  page.getByRole('button', { name: 'Check signed-in account', exact: true });

async function openSignIn(page: Page, baseURL: string) {
  const opened = page.context().waitForEvent('page');
  await signIn(page).click();
  const popup = await opened;
  await expect(popup).toHaveURL(baseURL + placementPath);
  await expect(popup.getByRole('heading', { name: 'Placement entry', exact: true })).toBeVisible();
  expect(await popup.evaluate(() => window.opener === null)).toBe(true);
  return popup;
}

for (const scenario of [
  'original retry',
  'popup retry',
  'blocked popup',
  'different owner',
] as const) {
  test(`LOCAL credential popup preserves placement during ${scenario}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await popupFixture(page, baseURL!);
    try {
      await page.goto(placementPath, { referer: `${baseURL}/embedded` });
      await expect(start(page)).toBeEnabled();
      expect(flow.posts).toEqual([]);
      await start(page).click();
      await flow.waitCount(1);
      await expect(signIn(page)).toBeVisible();
      await expect(start(page)).toBeDisabled();
      const original = await flow.stored(page);
      expect(original).not.toBeNull();
      expect(JSON.parse(flow.posts[0]!.body!)).toEqual({
        placement_id: 'plc_a',
        referrer_origin: baseURL,
        start_if_ready: true,
      });

      let popup: Page;
      if (scenario === 'blocked popup') {
        await page.evaluate(() => {
          window.open = () => null;
        });
        await signIn(page).click();
        const fallback = page.getByRole('link', { name: 'Open CMA sign-in', exact: true });
        const href = await fallback.getAttribute('href');
        expect(new URL(href!, baseURL).searchParams.get('return_to')).toBe(placementPath);
        await expect(fallback).toHaveAttribute('rel', 'noopener noreferrer');
        const opened = page.context().waitForEvent('page');
        await fallback.click();
        popup = await opened;
        await expect(popup).toHaveURL(baseURL! + placementPath);
        await expect(start(popup)).toBeEnabled();
        expect(await popup.evaluate(() => window.opener === null)).toBe(true);
        // A noopener fallback has its own storage; the original tab owns the saved command.
        expect(await flow.stored(popup)).toBeNull();
      } else {
        popup = await openSignIn(page, baseURL!);
        expect(await flow.stored(popup)).toBe(original);
        await expect(start(popup)).toBeDisabled();
        await popup.reload();
        await expect(signIn(popup)).toBeVisible();
        await expect(start(popup)).toBeDisabled();
        expect(await flow.stored(popup)).toBe(original);
        await start(popup).dispatchEvent('click');
      }
      expect(flow.loginReturns).toEqual([placementPath]);
      expect(flow.posts).toHaveLength(1);
      expect(await flow.stored(page)).toBe(original);
      await popup.screenshot({ path: info.outputPath('popup-placement.png') });

      if (scenario === 'different owner') {
        flow.setSubject('different-owner');
        await checkAccount(page).click();
        await expect(
          page.getByText('Sign in with the same CMA account that made this request.'),
        ).toBeVisible();
        await expect(start(page)).toBeDisabled();
        expect(await flow.stored(page)).toBe(original);
        expect(flow.posts).toHaveLength(1);
      } else {
        const actor = scenario === 'popup retry' ? popup : page;
        if (actor === popup) {
          // A copied packet grants no automatic authority: this window must verify explicitly too.
          await expect(checkAccount(popup)).toHaveCount(0);
          await openSignIn(popup, baseURL!);
        }
        await checkAccount(actor).click();
        await expect(start(actor)).toBeEnabled();
        expect(flow.posts).toHaveLength(1);
        await start(actor).click();
        await flow.waitCount(2);
        await expect(actor).toHaveURL(`${baseURL}/launches/lch_placement_receipt`);
        expect(flow.posts[1]).toMatchObject({
          page: actor === popup ? 'popup' : 'original',
          body: flow.posts[0]!.body,
          status: 201,
        });
        expect(flow.posts[1]!.key).toBeTruthy();
        expect(flow.posts[1]!.key).not.toBe(flow.posts[0]!.key);
        expect(await flow.stored(actor)).toBeNull();
        if (actor === popup) {
          await expect(start(page)).toBeDisabled();
          expect(await flow.stored(page)).toBe(original);
          await start(page).dispatchEvent('click');
          expect(flow.posts).toHaveLength(2);
        }
      }
      await page.screenshot({ path: info.outputPath('original-placement.png') });
    } finally {
      await flow.finish(info);
    }
  });
}
