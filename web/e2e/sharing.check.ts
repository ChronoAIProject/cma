import { test, expect } from '@playwright/test';
import type { ResourceShare } from '../../sdk/typescript/src/index';
import { LocalStartup, localTime } from './agent-startup-fixtures';

for (const origin of ['manual', 'trigger'] as const) {
  test(`LOCAL ${origin} owner sharing preserves creation and personal draft`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    boundary.sandbox = { ...boundary.sandbox, owner: 'local-owner' };
    boundary.agent = { ...boundary.agent, title: 'Retained investigation' };
    boundary.workspace = {
      ...boundary.workspace,
      agents: boundary.workspace.agents.map((agent) => ({
        ...agent,
        title: boundary.agent.title,
      })),
    };
    const retained = structuredClone(boundary.agent);
    await boundary.route(page, baseURL!);
    await page.route('**/auth/connections/check', (route) =>
      route.fulfill({
        json: { owner: 'local-owner', ready: true, checked_at: localTime, services: [] },
      }),
    );
    let policy = 1;
    let allowed = true;
    let shares: ResourceShare[] = [];
    const commands: { method: string; path: string; body: unknown; key: string | undefined }[] = [];
    await page.route('**/api/v1/resource-access/**', (route) =>
      route.fulfill({
        json: {
          capabilities: allowed ? ['view_details', 'manage_sharing'] : ['view_details'],
          revalidate_at: null,
          policy_revision: policy,
        },
      }),
    );
    await page.route('**/api/v1/organizations', (route) =>
      route.fulfill({
        json: { items: [{ id: 'org_research', role: 'admin', display_name: 'Research' }] },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/provenance', (route) =>
      route.fulfill({
        json: {
          agent_id: boundary.agent.id,
          created_at: boundary.agent.created_at,
          creation_origin: origin,
        },
      }),
    );
    await page.route('**/api/v1/resource-shares/**', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const kind = url.pathname.split('/')[4];
      const selector =
        kind === 'agent'
          ? { kind: 'agent' as const, agent_id: boundary.agent.id }
          : { kind: 'sandbox' as const, sandbox_id: boundary.sandbox.id };
      if (request.method() === 'GET')
        return route.fulfill({
          json: {
            items: shares.filter(
              (share) => share.selector.kind === kind && share.revoked_at === null,
            ),
          },
        });
      const body = request.postDataJSON();
      commands.push({
        method: request.method(),
        path: url.pathname,
        body,
        key: request.headers()['idempotency-key'],
      });
      if (kind === 'agent' && body?.mode === 'org_roles')
        return route.fulfill({
          status: 409,
          json: { code: 'unsafe_interactive_share', detail: 'Isolation is unproved' },
        });
      let share: ResourceShare;
      if (request.method() === 'POST') {
        share = {
          id: `share_${policy}`,
          owner: { kind: 'person', subject: 'local-owner' },
          selector,
          target_org_id: body.target_org_id,
          mode: body.mode,
          issuer_actor: { kind: 'human', subject: 'local-owner' },
          created_at: localTime,
          revoked_at: null,
          revision: 1,
        };
        shares.push(share);
      } else {
        const found = shares.find((share) => url.pathname.endsWith('/' + share.id))!;
        expect(Number(url.searchParams.get('expected_revision'))).toBe(found.revision);
        share = { ...found, revoked_at: localTime, revision: found.revision + 1 };
        shares = shares.map((old) => (old === found ? share : old));
      }
      policy += 1;
      return route.fulfill({
        json: {
          operation_id: `operation_${policy}`,
          share,
          actor: share.issuer_actor,
          at: localTime,
          policy_revision: policy,
        },
      });
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/agents/agt_contract');
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Keep my unsent personal work');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    await composer.evaluate((element) =>
      element.setAttribute('data-continuity-fixture', 'original'),
    );
    const creationLabel = origin === 'manual' ? 'Created manually' : 'Created by Trigger';
    await page.locator('.agent-header__strip').hover();
    await page.getByRole('button', { name: 'More Agent actions' }).click();
    await expect(page.getByRole('menu', { name: 'More Agent actions' })).toContainText(
      creationLabel,
    );
    await page.keyboard.press('Escape');
    await page.locator('.agent-header__strip').hover();
    await page.getByRole('button', { name: 'More Agent actions' }).click();
    await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
    await page.getByRole('button', { name: 'Sharing', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Resource sharing' });
    await expect(dialog.getByText('No direct organization grants.')).toBeVisible();
    await dialog.getByRole('button', { name: 'Share with organization', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'Research', exact: true }).click();
    for (const theme of ['light', 'dark']) {
      await page.evaluate((value) => {
        document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
        document.documentElement.style.colorScheme = value;
      }, theme);
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 1000 });
        await expect
          .poll(() =>
            dialog.evaluate((element) => {
              const box = element.getBoundingClientRect();
              return (
                box.left >= 0 &&
                box.right <= innerWidth &&
                box.top >= 0 &&
                box.bottom <= innerHeight &&
                element.scrollWidth <= element.clientWidth + 1
              );
            }),
          )
          .toBe(true);
        await page.screenshot({
          path: info.outputPath(`sharing-${origin}-${theme}-${width}.png`),
          fullPage: true,
          animations: 'disabled',
        });
      }
    }
    await dialog.getByRole('button', { name: 'Create share', exact: true }).click();
    await expect(dialog.getByText('Current access: View only')).toBeVisible();
    await dialog.getByRole('button', { name: 'Access for Research', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'Use organization roles', exact: true }).click();
    await dialog.getByRole('button', { name: 'Save access' }).click();
    await expect(dialog.getByRole('alert')).toContainText(
      'does not have proven execution isolation',
    );
    expect(commands.every((command) => command.path.includes('/agent/'))).toBe(true);
    await dialog.getByRole('button', { name: 'Share scope', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'Whole Sandbox', exact: true }).click();
    await expect(dialog.getByText('No direct organization grants.')).toBeVisible();
    await dialog.getByRole('button', { name: 'Share with organization', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'Research', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Create share', exact: true })).toBeDisabled();
    await dialog.getByRole('checkbox').check();
    await dialog.getByRole('button', { name: 'Create share', exact: true }).click();
    await expect(dialog.getByText('Current access: View only')).toBeVisible();
    await dialog.getByRole('button', { name: 'Revoke', exact: true }).click();
    await dialog.getByRole('button', { name: 'Revoke grant', exact: true }).click();
    await expect(dialog.getByText('No direct organization grants.')).toBeVisible();
    allowed = false;
    await dialog.getByRole('button', { name: 'Refresh sharing' }).click();
    await expect(dialog.getByRole('alert')).toContainText('Only the resource owner');
    await expect(dialog.getByRole('button', { name: 'Create share', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await page.setViewportSize({ width: 1440, height: 1000 });
    await expect(composer).toHaveValue('Keep my unsent personal work');
    await expect(composer).toHaveAttribute('data-continuity-fixture', 'original');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    await page.locator('.agent-header__strip').hover();
    await page.getByRole('button', { name: 'More Agent actions' }).click();
    await expect(page.getByRole('menu', { name: 'More Agent actions' })).toContainText(
      creationLabel,
    );
    await page.keyboard.press('Escape');
    expect(boundary.agent).toEqual(retained);
    expect(commands).toHaveLength(4);
    expect(commands.every((command) => command.key)).toBe(true);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
