import type { Page, Response } from '@playwright/test';
import type { Agent } from '@cma/client';
import { expect, type Harness } from './harness';
import { assertCredentialFree } from './security';
import { identifier, object } from './environment';
import { assertContextualTask } from './public-journey-contracts';
import { publicApiPath } from './public-api-path';
import { agentFirstAnswer } from './journey-answer';
import { workCommands } from './journey-proof';

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

async function taskForm(page: Page, workspace: LifecycleWorkspace, message: string) {
  await openWorkspacesTab(page);
  await workspaceAgentButton(page, workspace).click();
  const dialog = page.getByRole('dialog', { name: 'New task', exact: true });
  await dialog.getByRole('textbox', { name: 'Your task', exact: true }).fill(message);
  await expect(dialog.getByRole('button', { name: 'Start task', exact: true })).toBeEnabled();
  return dialog;
}

function taskResponse(page: Page) {
  return page.waitForResponse(
    (response) =>
      publicApiPath(new URL(response.url()).pathname) === '/api/v1/agents' &&
      response.request().method() === 'POST',
  );
}

async function admittedTask(
  page: Page,
  h: Harness,
  response: Response,
  workspace: LifecycleWorkspace,
  message: string,
) {
  expect(response.status()).toBe(202);
  assertContextualTask(response.request().postDataJSON(), workspace.id, message);
  const operationId = identifier(object(await response.json()).id, 'current task operation');
  await page.waitForURL(/\/agents\/[^/]+$/);
  const id = new URL(page.url()).pathname.split('/').at(-1)!;
  await expect(page.locator('#agent-message')).toBeEnabled({ timeout: 540_000 });
  const agent = await h.get<Agent>(page, `/api/v1/agents/${id}`);
  expect(agent.workspace.id).toBe(workspace.id);
  const operation = await h.get<{ id: string; agent_id?: string }>(
    page,
    `/api/v1/agent-lifecycle/${operationId}`,
  );
  expect(operation).toMatchObject({ id: operationId, agent_id: agent.id });
  h.journal.observe(
    `/api/v1/agent-lifecycle/${operationId}`,
    'GET',
    200,
    operation,
    h.fixture.accounts.owner.subject,
  );
  await agentFirstAnswer(h, page, agent.id);
  await h.flush();
  return { agent, operationId };
}

export async function createContextualTask(
  page: Page,
  h: Harness,
  workspace: LifecycleWorkspace,
  message: string,
) {
  const before = h.commands.length;
  const dialog = await taskForm(page, workspace, message);
  const response = taskResponse(page);
  await dialog.getByRole('button', { name: 'Start task', exact: true }).click();
  const result = await admittedTask(page, h, await response, workspace, message);
  const commands = workCommands(h.commands.slice(before));
  expect(commands).toHaveLength(1);
  expect(commands[0]?.path).toBe('/api/v1/agents');
  return { ...result, commands };
}

export async function createToRevive(
  page: Page,
  h: Harness,
  sandboxId: string,
  workspace: LifecycleWorkspace,
) {
  const before = h.commands.length;
  const createURL = `${h.config.baseURL}/api/agent-manager/v1/agents`;
  const message = 'Reply once with your working directory. Do not modify files.';
  let lost = false;
  let originalOperation: string | undefined;
  await page.route(createURL, async (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    assertContextualTask(route.request().postDataJSON(), workspace.id, message);
    if (lost) return route.fallback();
    // Retain the real accepted operation before losing only its browser response.
    const response = await route.fetch();
    expect(response.status()).toBe(202);
    const text = await response.text();
    assertCredentialFree(text, h.forbidden);
    const receipt: unknown = JSON.parse(text);
    originalOperation = identifier(object(receipt).id, 'lost task operation');
    h.journal.observe(
      new URL(route.request().url()).pathname,
      'POST',
      202,
      receipt,
      h.fixture.accounts.owner.subject,
    );
    lost = true;
    await route.abort('connectionfailed');
  });
  try {
    await page.reload();
    const dialog = await taskForm(page, workspace, message);
    await dialog.getByRole('button', { name: 'Start task', exact: true }).click();
    const recover = dialog.getByRole('button', { name: 'Recover original task', exact: true });
    await expect(recover).toBeEnabled();
    const response = taskResponse(page);
    await recover.click();
    const result = await admittedTask(page, h, await response, workspace, message);
    expect(lost).toBe(true);
    expect(result.operationId).toBe(originalOperation);
    const commands = workCommands(h.commands.slice(before));
    expect(commands).toHaveLength(2);
    expect(commands[0]?.path).toBe('/api/v1/agents');
    expect(commands[0]?.key).toBeTruthy();
    expect(commands[1]).toEqual(commands[0]);
    expect((await h.get<{ status: string }>(page, `/api/v1/sandboxes/${sandboxId}`)).status).toBe(
      'active',
    );
    await expect
      .poll(
        async () => {
          const events = await h.get<{ items: { kind: { type: string } }[] }>(
            page,
            `/api/v1/agents/${result.agent.id}/events`,
          );
          return events.items.filter((entry) => entry.kind.type === 'owner_message').length;
        },
        { timeout: 180_000 },
      )
      .toBe(1);
    await h.capture(page, 'j6-creation-revival');
    return { ...result, commands };
  } finally {
    await page.unroute(createURL);
  }
}
