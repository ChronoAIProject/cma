import type { Page } from '@playwright/test';
import { expect, type Harness } from './harness';
import { assertCredentialFree } from './security';

export interface LifecycleWorkspace {
  id: string;
  path: string;
}

/** The Workspaces tab row whose path is exactly this Workspace's, and its New agent here. */
export function workspaceAgentButton(page: Page, workspace: LifecycleWorkspace) {
  return page
    .locator('.sandbox-table tbody tr')
    .filter({ has: page.getByText(workspace.path, { exact: true }) })
    .getByRole('button', { name: /^New agent in / });
}

/** Workspace rows live in the Sandbox page's Workspaces tab. */
export async function openWorkspacesTab(page: Page) {
  await page.getByRole('tab', { name: 'Workspaces', exact: true }).click();
}

export async function createToRevive(
  page: Page,
  h: Harness,
  sandboxId: string,
  workspace: LifecycleWorkspace,
) {
  const before = h.commands.length;
  const createPath = `/api/v1/workspaces/${workspace.id}/agents`;
  const createURL = `${h.config.baseURL}${createPath}`;
  let lost = false;
  await page.route(createURL, async (route) => {
    if (lost || route.request().method() !== 'POST') return route.fallback();
    // Only browser transport is faulted; the real server admission is retained for cleanup.
    const response = await route.fetch();
    expect(response.status()).toBe(201);
    const text = await response.text();
    assertCredentialFree(text, h.forbidden);
    const created: unknown = JSON.parse(text);
    h.journal.observe(
      new URL(route.request().url()).pathname,
      'POST',
      201,
      created,
      h.fixture.accounts.owner.subject,
    );
    lost = true;
    await route.abort('connectionfailed');
  });
  try {
    await page.reload();
    await openWorkspacesTab(page);
    await workspaceAgentButton(page, workspace).click();
    const dialog = page.getByRole('dialog', { name: 'New agent', exact: true });
    await dialog.getByRole('button', { name: 'Continue', exact: true }).click();
    await dialog.getByRole('radio', { name: 'Public', exact: true }).check();
    const profile = h.fixture.profiles.agent;
    await dialog
      .getByRole('button', { name: `${profile.name} Version ${profile.revision}`, exact: true })
      .click();
    await dialog.getByRole('button', { name: 'Continue', exact: true }).click();
    await dialog.getByLabel('Title (optional)', { exact: true }).fill('Creation revival evidence');
    await dialog
      .getByLabel('First message (optional)', { exact: true })
      .fill('Reply once with your working directory. Do not modify files.');
    await dialog.getByRole('button', { name: 'Create agent', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Retry creation', exact: true })).toBeEnabled();
    await dialog.getByRole('button', { name: 'Retry creation', exact: true }).click();
    await page.waitForURL(/\/agents\/[^/]+$/);
    const id = new URL(page.url()).pathname.split('/').at(-1)!;
    await page.reload();
    await expect(page.locator('#agent-message')).toBeEnabled({ timeout: 540_000 });
    const commands = h.commands.slice(before);
    const creates = commands.filter((row) =>
      /^\/api\/v1\/workspaces\/[^/]+\/agents$/.test(row.path),
    );
    expect(lost).toBe(true);
    expect(creates).toHaveLength(2);
    expect(creates[0]?.path).toBe(createPath);
    expect(creates[1]).toEqual(creates[0]);
    expect(commands.some((row) => /\/(revive|input|agui\/run|responses)$/.test(row.path))).toBe(
      false,
    );
    expect((await h.get<{ status: string }>(page, `/api/v1/sandboxes/${sandboxId}`)).status).toBe(
      'active',
    );
    await expect
      .poll(
        async () => {
          const events = await h.get<{ items: { kind: { type: string } }[] }>(
            page,
            `/api/v1/agents/${id}/events`,
          );
          return events.items.filter((entry) => entry.kind.type === 'owner_message').length;
        },
        { timeout: 180_000 },
      )
      .toBe(1);
    await h.capture(page, 'j6-creation-revival');
  } finally {
    await page.unroute(createURL);
  }
}
