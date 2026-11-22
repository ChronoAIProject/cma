import { test, expect } from '@playwright/test';
import { publisherFixture } from './public-journeys.fixture';
import { publishCurrentTrigger } from './publisher-current';

for (const role of ['owner', 'secondOwner'] as const) {
  test(`LOCAL current publisher driver publishes pinned inline configuration for ${role}`, async ({
    page,
    baseURL,
  }) => {
    const { harness, boundary } = await publisherFixture(page, baseURL!, role);
    const result = await publishCurrentTrigger(harness, page, role);
    expect(result.revision.content.target.inline).toEqual(result.target);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
test('LOCAL lifecycle bootstrap publisher retains the managed default image', async ({
  page,
  baseURL,
}) => {
  const { harness, boundary } = await publisherFixture(page, baseURL!, 'owner');
  const result = await publishCurrentTrigger(harness, page, 'owner', true);
  expect(result.revision.content.target.inline?.image).toBeUndefined();
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

import { taskFixture } from './public-task.fixture';
import {
  createContextualTask,
  openWorkspacesTab,
  workspaceAgentButton,
} from './lifecycle-creation';
import { assertContextualTask } from './public-journey-contracts';

test('LOCAL current contextual task driver resolves the operation and verifies the first answer', async ({
  page,
  baseURL,
}) => {
  const { harness, boundary, workspace, message } = await taskFixture(page, baseURL!);
  await page.goto(`/sandboxes/${boundary.sandbox.id}`);
  const created = await createContextualTask(page, harness, workspace, message);
  expect(created.agent.id).toBe(boundary.agent.id);
  expect(created.operationId).toBe('inline-operation');
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

test('LOCAL contextual task recovery keeps the exact request and key after response loss', async ({
  page,
  baseURL,
}) => {
  const { harness, boundary, workspace, message, bodies } = await taskFixture(page, baseURL!, true);
  await page.goto(`/sandboxes/${boundary.sandbox.id}`);
  await openWorkspacesTab(page);
  await workspaceAgentButton(page, workspace).click();
  const dialog = page.getByRole('dialog', { name: 'New task', exact: true });
  await dialog.getByRole('textbox', { name: 'Your task', exact: true }).fill(message);
  await dialog.getByRole('button', { name: 'Start task', exact: true }).click();
  await dialog.getByRole('button', { name: 'Recover original task', exact: true }).click();
  await page.waitForURL(`/agents/${boundary.agent.id}`);
  expect(bodies).toHaveLength(2);
  assertContextualTask(bodies[0], workspace.id, message);
  expect(bodies[1]).toEqual(bodies[0]);
  expect(harness.commands[0]?.key).toBeTruthy();
  expect(harness.commands[1]).toEqual(harness.commands[0]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
