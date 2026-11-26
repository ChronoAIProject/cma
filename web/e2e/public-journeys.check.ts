import { auditSyntheticPublications } from './public-journey-audit.fixture';
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
    expect(result.revision.content.target.agent_profile.id).toBe(`generated-agent-${role}`);
    expect(result.document.draft.target).toEqual(result.revision.content.target);
    expect(result.target.workspace?.repository?.ref).toBe('b'.repeat(40));
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
import { assertContextualTask, assertInlineRequest } from './public-journey-contracts';
import { currentTriggerTarget } from './current-trigger-target';
import { publicApiPath } from './public-api-path';

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

test('LOCAL compiled publications produce evidence accepted by the final auditor', async ({
  browser,
  baseURL,
}) => {
  const publications = [];
  for (const role of ['owner', 'secondOwner'] as const) {
    const page = await browser.newPage();
    try {
      const { harness, boundary } = await publisherFixture(page, baseURL!, role);
      publications.push(await publishCurrentTrigger(harness, page, role));
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    } finally {
      await page.close();
    }
  }
  auditSyntheticPublications(publications);
});

test('LOCAL model input keeps focus when image resolution completes before typing', async ({
  page,
  baseURL,
}) => {
  const { harness, boundary } = await publisherFixture(page, baseURL!, 'secondOwner');
  let releaseImage!: () => void;
  const imageGate = new Promise<void>((resolve) => {
    releaseImage = resolve;
  });
  await page.route('**/images/image-secondOwner/versions', async (route) => {
    await imageGate;
    await route.fallback();
  });
  try {
    await page.goto('/triggers/manage');
    const image = page.getByRole('button', { name: 'Image', exact: true });
    await image.click();
    await page
      .getByRole('menuitemradio', { name: 'Image secondOwner (Public)', exact: true })
      .click();
    const model = page.getByLabel('Model', { exact: true });
    await expect(model).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Next', exact: true })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeDisabled();
    await model.focus();
    await expect(model).toBeFocused();
    releaseImage();
    await expect(image).toBeEnabled();
    await expect(model).toBeFocused();
    await page.keyboard.insertText('fixture-model');
    await expect(model).toHaveValue('fixture-model');
    await expect(model).toBeFocused();
    await page
      .getByLabel('Agent key scope services (comma separated)', { exact: true })
      .fill('model');
    await page
      .getByLabel('GitHub repository (optional)', { exact: true })
      .fill(harness.fixture.publicJourneys.repository.url);
    await page
      .getByLabel('GitHub repository ref', { exact: true })
      .fill(harness.fixture.publicJourneys.repository.reference);
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await page.locator('#trigger-title').fill('Preserved model selection');
    const creation = page.waitForResponse(
      (response) =>
        response.request().method() === 'POST' &&
        publicApiPath(new URL(response.url()).pathname) === '/api/v1/triggers',
    );
    await page.getByRole('button', { name: 'Save draft', exact: true }).click();
    const response = await creation;
    expect(response.status()).toBe(201);
    assertInlineRequest(
      response.request().postDataJSON().content.target,
      currentTriggerTarget(harness.fixture, 'secondOwner'),
    );
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  } finally {
    releaseImage();
  }
});
