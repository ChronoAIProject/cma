import { verifiedEntryHeaders } from './public-entry-binding.fixture';
import { test, expect, type Page } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import { buildSandboxHarness, sandboxHarnessPath } from './credential-sandbox.fixture';

// The refusal is announced by a live alert. A replaced alert can still be
// leaving the page, hidden and inert, with the same text; role queries skip it.
const accountMismatch = (page: Page) =>
  page
    .getByRole('alert')
    .filter({ hasText: 'Sign in with the same CMA account that made this request.' });

let sandboxHarness: Awaited<ReturnType<typeof buildSandboxHarness>>;
test.beforeAll(async () => {
  sandboxHarness = await buildSandboxHarness();
});

for (const surface of ['sandbox', 'landing', 'review'] as const) {
  test(`LOCAL ${surface} credential recovery preserves choices and separates refusal from transport retry`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    if (surface === 'sandbox') await sandboxHarness(page, baseURL!);
    let launch: Launch = {
      ...recorded<Launch>('launches.json', 'launch-ready-create'),
      public_id: 'pub_credential',
      review: { needs: [{ type: 'owner_requires_review' }], can_start_now: false },
    };
    const commandPath =
      surface === 'sandbox'
        ? '/api/v1/sandboxes'
        : surface === 'landing'
          ? '/api/v2/triggers/public/pub_credential/launches'
          : `/api/v1/launches/${launch.id}/start`;
    const commands: { key?: string; body: unknown; bytes: string | null }[] = [];
    const loginReturns: string[] = [];
    const entryProbes: string[] = [];
    let sessionSubject = 'local-owner';
    await page.context().route('**/auth/login**', async (route) => {
      const url = new URL(route.request().url());
      expect(url.origin).toBe(baseURL);
      loginReturns.push(url.searchParams.get('return_to') ?? '');
      await route.fulfill({
        contentType: 'text/html',
        body: '<!doctype html><title>Local sign-in boundary</title><p>Local identity fixture only.</p>',
      });
    });
    await page.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== baseURL) return route.fallback();
      const proof = verifiedEntryHeaders(request, sessionSubject);
      const reply = (body: unknown, status = 200) =>
        route.fulfill({
          status,
          headers: proof,
          contentType: 'application/json',
          body: JSON.stringify(body),
        });
      if (url.pathname === '/auth/session' && request.method() === 'GET')
        return reply({
          authenticated: true,
          subject: sessionSubject,
          display_name: 'Local owner',
          visitor: false,
          capabilities: [
            'use_own_sandboxes',
            'create_sandbox',
            'delete_own_sandbox',
            'launch_triggers',
          ],
        });
      if (url.pathname === commandPath && request.method() === 'POST') {
        commands.push({
          key: request.headers()['idempotency-key'],
          body: request.postDataJSON(),
          bytes: request.postData(),
        });
        if (surface === 'landing')
          expect(commands.at(-1)?.body).toMatchObject({ start_if_ready: true });
        if (commands.length === 1)
          return reply(
            {
              type: 'https://spec.chrono-ai.fun/cma/1.0/problems/sandbox-credential-setup-required',
              title: 'sandbox credential setup required',
              status: 409,
              detail: 'Sign in to CMA with the same account.',
              code: 'sandbox_credential_setup_required',
            },
            409,
          );
        if (commands.length === 2) return route.abort('connectionfailed');
        if (surface !== 'sandbox') {
          const started = recorded<Launch>('launches.json', 'launch-ready-start');
          launch = {
            ...started,
            public_id: launch.public_id,
            confirmations: started.confirmations.map((confirmation) => ({
              ...confirmation,
              surface: surface === 'landing' ? 'landing' : 'review',
            })),
          };
        }
        return reply(
          surface === 'sandbox' ? boundary.sandbox : launch,
          surface === 'review' ? 202 : 201,
        );
      }
      if (
        url.pathname === '/api/v1/triggers/public/pub_credential/entry-intent' &&
        request.method() === 'POST'
      ) {
        entryProbes.push(url.pathname);
        // Direct navigation has no publisher navigation cookie to consume.
        return reply({ accepted: false });
      }
      if (request.method() !== 'GET') return route.fallback();
      if (url.pathname === '/api/v1/sandbox-profiles') return reply({ items: [] });
      if (url.pathname === '/api/v1/sandbox-profiles/sbp_route/revisions/1')
        return reply({
          ...recorded<object>('profiles.json', 'sandbox-profile-exact-revision'),
          profile_id: 'sbp_route',
        });
      if (url.pathname === '/api/v1/triggers/public/pub_credential')
        return reply({
          ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
          always_review: surface === 'review',
        });
      if (url.pathname === `/api/v1/launches/${launch.id}`) return reply(launch);
      if (url.pathname === `/api/v1/launches/${launch.id}/requirements`)
        return reply(launch.requirements);
      if (url.pathname === `/api/v1/agui/launches/${launch.id}/events`)
        return route.fulfill({
          contentType: 'text/event-stream',
          body: ': local progress only\n\n',
        });
      return route.fallback();
    });
    if (surface === 'review')
      await page.addInitScript(() => {
        window.open = () => null;
      });
    await page.goto(
      surface === 'sandbox'
        ? sandboxHarnessPath
        : surface === 'landing'
          ? '/t/pub_credential'
          : `/launches/${launch.id}`,
    );
    if (surface === 'sandbox') {
      const dialog = page.getByRole('dialog', { name: 'New sandbox', exact: true });
      await dialog.getByRole('radio', { name: 'Default', exact: true }).check();
      await dialog
        .getByRole('button', { name: 'Select Deployment default Version 1', exact: true })
        .click();
      await dialog.getByLabel('Name (optional)', { exact: true }).fill('Preserved setup draft');
    }
    const command = page.getByRole('button', {
      name:
        surface === 'sandbox' ? 'Create sandbox' : surface === 'landing' ? 'Try it' : 'Continue',
      exact: true,
    });
    await expect(command).toBeEnabled();
    expect(commands).toEqual([]);
    if (surface === 'landing') await expect.poll(() => entryProbes.length).toBe(1);
    await command.click();
    await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
    await expect(command).toBeDisabled();
    for (const theme of ['light', 'dark']) {
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        const recovery = page.getByRole('region', { name: 'CMA sign-in required' });
        await recovery.scrollIntoViewIfNeeded();
        await expect(recovery).toBeInViewport();
        await expect
          .poll(() =>
            page.evaluate(() => {
              const element = document.querySelector<HTMLElement>('.sandbox-credential-setup')!;
              return (
                document.documentElement.scrollWidth <= innerWidth + 1 &&
                element.scrollWidth <= element.clientWidth + 1 &&
                element.getBoundingClientRect().right <= innerWidth + 1
              );
            }),
          )
          .toBe(true);
        await page.screenshot({
          path: info.outputPath(`credential-refusal-${theme}-${width}.png`),
          fullPage: true,
        });
      }
    }
    await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
    if (surface === 'review') {
      await expect(page.getByText('The sign-in window was blocked.')).toBeVisible();
      await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
    }
    await expect.poll(() => loginReturns.length).toBe(1);
    expect(loginReturns).toEqual([surface === 'sandbox' ? '/' : '/t/pub_credential']);
    sessionSubject = 'different-owner';
    await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
    await expect(accountMismatch(page)).toBeVisible();
    await expect(command).toBeDisabled();
    expect(commands).toHaveLength(1);
    sessionSubject = 'local-owner';
    await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
    await expect(command).toBeEnabled();
    expect(commands).toHaveLength(1);
    if (surface === 'sandbox')
      await expect(page.getByLabel('Name (optional)', { exact: true })).toHaveValue(
        'Preserved setup draft',
      );
    sessionSubject = 'different-owner';
    await command.click();
    await expect(accountMismatch(page)).toBeVisible();
    expect(commands).toHaveLength(1);
    const checkedRetry = page.getByRole('button', {
      name:
        surface === 'sandbox' ? 'Retry creation' : surface === 'landing' ? 'Try it' : 'Try again',
      exact: true,
    });
    const recover = async () => {
      sessionSubject = 'local-owner';
      await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
      if (surface === 'review')
        await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
      await expect(checkedRetry).toBeEnabled();
    };
    await recover();
    expect(commands).toHaveLength(1);
    await checkedRetry.click();
    const ambiguousRetry = page.getByRole('button', {
      name: surface === 'sandbox' ? 'Retry creation' : 'Try again',
      exact: true,
    });
    await expect(ambiguousRetry).toBeEnabled();
    expect(commands).toHaveLength(2);
    sessionSubject = 'different-owner';
    await ambiguousRetry.click();
    await expect(accountMismatch(page)).toBeVisible();
    expect(commands).toHaveLength(2);
    if (surface !== 'sandbox') {
      await page.reload();
      await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
      await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
      if (surface === 'review')
        await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
      await expect(accountMismatch(page)).toBeVisible();
      expect(commands).toHaveLength(2);
    }
    await recover();
    expect(commands).toHaveLength(2);
    await checkedRetry.click();
    if (surface === 'sandbox') await page.waitForURL('/sandboxes/sbx_route');
    else if (surface === 'landing') await page.waitForURL(`/launches/${launch.id}`);
    else await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(commands).toHaveLength(3);
    expect(commands.every(({ key }) => typeof key === 'string' && key.length > 0)).toBe(true);
    expect(commands[1]?.key).not.toBe(commands[0]?.key);
    expect(commands[1]?.body).toEqual(commands[0]?.body);
    expect(commands[1]?.bytes).toBe(commands[0]?.bytes);
    expect(commands[2]).toEqual(commands[1]);
    await page.reload();
    await expect(
      page.locator(surface === 'sandbox' ? '.sandbox-page' : '.public-progress'),
    ).toBeVisible();
    expect(commands).toHaveLength(3);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
    expect(entryProbes).toHaveLength(surface === 'landing' ? 1 : 0);
    await info.attach('local-credential-boundary', {
      body: JSON.stringify({
        mocked: true,
        boundary:
          surface === 'sandbox'
            ? 'built_retained_component_public_http'
            : 'built_application_public_http',
        surface,
        providerLoginExecuted: false,
        renewableCredentialsProven: false,
        commands: 2,
        writes: 3,
        blockedDispatches: 2,
        reloadedUnsettledCommand: surface !== 'sandbox',
        captures: 6,
        entryProbes: entryProbes.length,
      }),
      contentType: 'application/json',
    });
  });
}
