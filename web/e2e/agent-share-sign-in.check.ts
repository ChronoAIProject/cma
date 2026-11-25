import { expect, test } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

const shareId = 'share-fixture';
const sharePath = `/agent-manager/share/${shareId}`;
const storageKey = `cma:agent-share:${shareId}`;
const token = 'ab'.repeat(32);

for (const outcome of ['accepted', 'revoked', 'invalid'] as const) {
  test(`LOCAL Agent share sign-in handles the ${outcome} result`, async ({
    page,
    context,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    let signedIn = false;
    let redemptions = 0;

    await page.route('**/auth/session', (route) =>
      signedIn
        ? route.fulfill({
            json: { authenticated: true, subject: 'local-reader', capabilities: [] },
          })
        : route.fulfill({ status: 401, json: { authenticated: false, silent_sign_in: true } }),
    );
    await page.route('**/auth/login?*', (route) => {
      const url = new URL(route.request().url());
      expect([...url.searchParams.entries()]).toEqual([['return_to', sharePath]]);
      expect(url.href).not.toContain(token);
      expect(route.request().headers()['referer'] ?? '').not.toContain(token);
      return route.fulfill({
        contentType: 'text/html',
        body: '<a href="/auth/callback?code=fixture">Complete fixture sign-in</a>',
      });
    });
    await page.route('**/auth/callback?code=fixture', (route) => {
      signedIn = true;
      return route.fulfill({ status: 303, headers: { location: sharePath } });
    });
    await page.route(`**/api/v1/agent-share-links/${shareId}/redeem`, (route) => {
      redemptions += 1;
      expect(signedIn).toBe(true);
      expect(route.request().method()).toBe('POST');
      expect(new URL(route.request().url()).search).toBe('');
      expect(route.request().postDataJSON()).toEqual({ token });
      if (outcome !== 'accepted') {
        return route.fulfill({
          status: outcome === 'revoked' ? 410 : 404,
          json: { title: 'Share unavailable' },
        });
      }
      return route.fulfill({
        json: {
          id: shareId,
          agent_id: boundary.agent.id,
          audience: { kind: 'public' },
          mode: 'view',
          shared_by: { subject: 'local-owner', display_name: 'Local owner', avatar_url: null },
          created_at: localTime,
          revoked_at: null,
          revision: 1,
          accepted_count: 1,
        },
      });
    });

    const signIn = () => page.getByRole('button', { name: 'Sign in', exact: true }).click();
    const complete = page.getByRole('link', { name: 'Complete fixture sign-in' });
    const retained = () => page.evaluate((key) => sessionStorage.getItem(key), storageKey);
    await page.goto(`${sharePath}#${token}`);
    await signIn();
    await expect(complete).toBeVisible();
    expect(redemptions).toBe(0);
    expect(JSON.parse((await retained())!).token).toBe(token);
    expect(await page.evaluate(() => Object.values(localStorage).join(''))).not.toContain(token);

    if (outcome === 'accepted') {
      const otherTab = await context.newPage();
      await otherTab.route(baseURL! + sharePath, (route) =>
        route.fulfill({ contentType: 'text/html', body: '<title>Independent tab</title>' }),
      );
      await otherTab.goto(baseURL! + sharePath);
      expect(await otherTab.evaluate((key) => sessionStorage.getItem(key), storageKey)).toBeNull();
      await otherTab.close();
      await page.goBack();
      await expect(page).toHaveURL(baseURL! + sharePath);
      await page.reload();
      expect(JSON.parse((await retained())!).token).toBe(token);
      await signIn();
    }

    await complete.click();
    if (outcome === 'accepted') {
      await expect(page).toHaveURL(`${baseURL}/agents/${boundary.agent.id}`);
    } else {
      await expect(page.getByRole('alert')).toContainText('This link is unavailable');
      await expect(page).toHaveURL(baseURL! + sharePath);
    }
    expect(redemptions).toBe(1);
    expect(await retained()).toBeNull();
  });
}
