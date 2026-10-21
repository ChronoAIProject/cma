import { test, expect } from './harness';
import { identifier, object, required } from './environment';
import { createToRevive, openWorkspacesTab, workspaceAgentButton } from './lifecycle-creation';

test('J6 G-S1 G-P1 trusted Sandbox, two Agents, stop/delete, pause/revive and rotation preserve work', async ({
  harness: h,
}) => {
  const page = await h.open('owner');
  const profile = h.fixture.profiles.sandbox;
  await page.goto(`${h.config.baseURL}/marketplace/sandbox/${profile.id}`);
  await page.getByRole('button', { name: 'Use', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Create sandbox', exact: true })).toBeDisabled();
  await page.getByLabel('I trust this profile', { exact: true }).check();
  await h.capture(page, 'j6-trust');
  await page.getByRole('button', { name: 'Create sandbox', exact: true }).click();
  await page.waitForURL(/\/sandboxes\/[^/]+$/);
  await h.flush();
  const sandboxId = h.journal.resources.find((row) => row.kind === 'sandboxes')!.id;
  await expect
    .poll(
      async () => (await h.get<{ status: string }>(page, `/api/v1/sandboxes/${sandboxId}`)).status,
      { timeout: 540_000 },
    )
    .toBe('active');
  await page.reload();
  await page.getByRole('button', { name: 'More actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'New workspace', exact: true }).click();
  await page.getByRole('radio', { name: 'Public', exact: true }).check();
  await page
    .getByRole('button', {
      name: `${h.fixture.profiles.workspace.name} Version ${h.fixture.profiles.workspace.revision}`,
      exact: true,
    })
    .click();
  const trust = page.getByLabel('I trust this profile', { exact: true });
  if (await trust.isVisible()) await trust.check();
  const workspaceAdmission = page.waitForResponse(
    (response) =>
      response.url() === `${h.config.baseURL}/api/v1/sandboxes/${sandboxId}/workspaces` &&
      response.request().method() === 'POST',
  );
  await page.getByRole('button', { name: 'Create workspace', exact: true }).click();
  const workspaceResponse = await workspaceAdmission;
  expect(workspaceResponse.status()).toBe(201);
  const createdWorkspace = object(await workspaceResponse.json());
  expect(createdWorkspace.sandbox_id).toBe(sandboxId);
  const workspace = {
    id: identifier(createdWorkspace.id, 'J6 created Workspace ID'),
    path: required(createdWorkspace.path, 'J6 created Workspace path'),
  };
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const agentIds: string[] = [];
  for (let index = 0; index < 2; index++) {
    await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
    await openWorkspacesTab(page);
    await workspaceAgentButton(page, workspace).click();
    const dialog = page.getByRole('dialog', { name: 'New agent', exact: true });
    await dialog.getByRole('button', { name: 'Continue', exact: true }).click();
    await dialog.getByRole('radio', { name: 'Public', exact: true }).check();
    await dialog
      .getByRole('button', {
        name: `${h.fixture.profiles.agent.name} Version ${h.fixture.profiles.agent.revision}`,
        exact: true,
      })
      .click();
    await dialog.getByRole('button', { name: 'Continue', exact: true }).click();
    await dialog.getByLabel('Title (optional)', { exact: true }).fill(`Staging Agent ${index + 1}`);
    await dialog
      .getByLabel('First message (optional)', { exact: true })
      .fill('Reply with your working directory and agent home. Do not modify files.');
    await dialog.getByRole('button', { name: 'Create agent', exact: true }).click();
    await page.waitForURL(/\/agents\/[^/]+$/);
    await expect(page.locator('#agent-message')).toBeEnabled({ timeout: 540_000 });
    const id = new URL(page.url()).pathname.split('/').at(-1)!;
    agentIds.push(id);
  }
  expect(new Set(agentIds).size).toBe(2);
  await h.capture(page, 'j6-two-agents');
  const before = await h.operators.verifyRuntime({ phase: 'before_pause', sandboxId, agentIds });
  expect(before).toMatchObject({
    isolatedHomes: true,
    installBeforeCredentials: true,
    softwarePresent: true,
    dirtyWorktreePresent: true,
    transcriptsPreserved: true,
  });
  await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled({
    timeout: 180_000,
  });
  await createToRevive(page, h, sandboxId, workspace);
  await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled({
    timeout: 540_000,
  });
  for (const fault of ['restart_pausing', 'restart_reviving', 'rotate_image'] as const) {
    const control = { name: fault, fixtureOwner: h.fixture.fixtureOwner, sandboxId };
    await h.operators.controlFault({ ...control, phase: 'arm' });
    try {
      if (fault !== 'rotate_image') {
        await h.command(page, 'POST', `/api/v1/sandboxes/${sandboxId}/pause`);
        await expect
          .poll(
            async () =>
              (await h.get<{ status: string }>(page, `/api/v1/sandboxes/${sandboxId}`)).status,
            { timeout: 180_000 },
          )
          .toBe('paused');
        await h.command(page, 'POST', `/api/v1/sandboxes/${sandboxId}/revive`);
      }
      await h.operators.controlFault({ ...control, phase: 'release' });
      await expect
        .poll(
          async () =>
            (await h.get<{ status: string }>(page, `/api/v1/sandboxes/${sandboxId}`)).status,
          { timeout: 540_000 },
        )
        .toBe('active');
      const after = await h.operators.verifyRuntime({
        phase: fault === 'rotate_image' ? 'after_rotation' : 'after_revive',
        sandboxId,
        agentIds,
      });
      expect(after).toMatchObject({
        isolatedHomes: true,
        softwarePresent: true,
        dirtyWorktreePresent: true,
        transcriptsPreserved: true,
      });
      if (fault === 'rotate_image') expect(after.runtimeVersion).not.toBe(before.runtimeVersion);
      for (const id of agentIds) {
        const agent = await h.get<{ id: string; sandbox: { id: string } }>(
          page,
          `/api/v1/agents/${id}`,
        );
        expect(agent.id).toBe(id);
        expect(agent.sandbox.id).toBe(sandboxId);
      }
    } finally {
      await h.operators.controlFault({ ...control, phase: 'release' });
    }
  }
  await page.goto(`${h.config.baseURL}/agents/${agentIds[0]}`);
  await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect
    .poll(
      async () => (await h.get<{ status: string }>(page, `/api/v1/agents/${agentIds[0]}`)).status,
    )
    .toBe('stopped');
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await page
    .getByLabel('I understand this Agent and Worktree will be deleted.', { exact: true })
    .check();
  await page.getByRole('button', { name: 'Delete Agent', exact: true }).click();
  expect((await h.get<{ id: string }>(page, `/api/v1/agents/${agentIds[1]}`)).id).toBe(agentIds[1]);
});
