import { randomUUID } from 'node:crypto';
import type { TriggerStatistics } from '@cma/client';
import { test, expect } from './harness';
import { identifier, object, required } from './environment';
import { createContextualTask, createToRevive, openWorkspacesTab } from './lifecycle-creation';
import { publishCurrentTrigger } from './publisher-current';
import { publicApiPath } from './public-api-path';
import { assertPublishedLaunch, assertSingleLaunchDelta } from './public-journey-contracts';
import {
  commandProof,
  hashText,
  publicationProof,
  runtimeProof,
  workCommands,
} from './journey-proof';
import { firstAnswer } from './journey-answer';

test('J6 image and inline Workspace tasks preserve identity through retry, pause, rotation and deletion', async ({
  harness: h,
}) => {
  const page = await h.open('owner');
  const published = await publishCurrentTrigger(h, page, 'owner');
  const beforeStats = await h.get<TriggerStatistics>(
    page,
    `/api/v1/triggers/${published.document.id}/stats`,
  );
  const beforeCommands = h.commands.length;
  const admitted = await h.launchTarget(page, {
    publicId: published.document.public_id!,
    placementId: published.placement.id,
  });
  const launch = await h.ready(page, admitted.id);
  await firstAnswer(h, page, launch, h.fixture.accounts.owner.subject);
  const sandboxId = identifier(launch.resolution.sandbox.id, 'J6 managed Sandbox');
  const sandbox = await h.get(page, `/api/v1/sandboxes/${sandboxId}`);
  assertPublishedLaunch(launch, sandbox, {
    triggerId: published.document.id,
    publicId: published.document.public_id!,
    placementId: published.placement.id,
    digest: published.revision.digest,
    subject: h.fixture.accounts.owner.subject,
  });
  let afterStats = beforeStats;
  await expect(async () => {
    afterStats = await h.get<TriggerStatistics>(
      page,
      `/api/v1/triggers/${published.document.id}/stats`,
    );
    assertSingleLaunchDelta(beforeStats, afterStats, published.placement.id);
  }).toPass({ timeout: 20_000 });
  const bootstrap = publicationProof(
    published,
    launch,
    sandbox,
    workCommands(h.commands.slice(beforeCommands)),
    beforeStats,
    afterStats,
  );
  await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
  await openWorkspacesTab(page);
  await page.getByRole('button', { name: 'New workspace', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'New workspace', exact: true });
  await dialog.getByRole('button', { name: 'Workspace source', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Clone a GitHub repository', exact: true }).click();
  const repository = h.fixture.publicJourneys.repository;
  const name = `journey-${randomUUID().slice(0, 8)}`;
  await dialog.getByLabel('Workspace name', { exact: true }).fill(name);
  await dialog.getByLabel('GitHub repository URL', { exact: true }).fill(repository.url);
  await dialog.getByLabel('Branch or ref (optional)', { exact: true }).fill(repository.reference);
  const admission = page.waitForResponse(
    (response) =>
      publicApiPath(new URL(response.url()).pathname) ===
        `/api/v1/sandboxes/${sandboxId}/workspaces/inline` &&
      response.request().method() === 'POST',
  );
  await dialog.getByRole('button', { name: 'Create workspace', exact: true }).click();
  const response = await admission;
  expect(response.status()).toBe(201);
  expect(response.request().postDataJSON()).toEqual({
    name,
    source: { type: 'git_repository', url: repository.url, reference: repository.reference },
  });
  const created = object(await response.json());
  expect(created.sandbox_id).toBe(sandboxId);
  const workspace = {
    id: identifier(created.id, 'J6 inline Workspace'),
    path: required(created.path, 'J6 inline Workspace path'),
  };
  await expect(dialog).toHaveCount(0);
  const agents = [];
  for (let index = 0; index < 2; index++) {
    await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
    const result = await createContextualTask(
      page,
      h,
      workspace,
      `Task ${index + 1}: reply with your working directory and Agent home. Do not modify files.`,
    );
    agents.push({
      id: result.agent.id,
      workspaceId: result.agent.workspace.id,
      sandboxId: result.agent.sandbox.id,
      worktreeId: result.agent.worktree.id,
      command: commandProof(result.commands[0]!),
    });
  }
  const agentIds = agents.map((agent) => agent.id);
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
  expect(before.imageReference).toBe(h.fixture.provenance.runtimeImage);
  expect(before.homes.map((home) => home.agentId).sort()).toEqual([...agentIds].sort());
  expect(new Set(before.homes.map((home) => home.homeDigest)).size).toBe(2);
  for (const home of before.homes) {
    expect(home.homeDigest).toMatch(/^[a-f0-9]{64}$/);
    expect(home.fileHash).toMatch(/^[a-f0-9]{64}$/);
  }
  await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled({
    timeout: 180_000,
  });
  const retry = await createToRevive(page, h, sandboxId, workspace);
  const workspaceState = await h.get<{ agents: { id: string }[] }>(
    page,
    `/api/v1/workspaces/${workspace.id}`,
  );
  const workspaceAgentIds = workspaceState.agents.map((agent) => agent.id);
  expect([...workspaceAgentIds].sort()).toEqual([...agentIds, retry.agent.id].sort());
  await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled({
    timeout: 540_000,
  });
  const afterSnapshots = [];
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
      expect([...after.homes].sort((a, b) => a.agentId.localeCompare(b.agentId))).toEqual(
        [...before.homes].sort((a, b) => a.agentId.localeCompare(b.agentId)),
      );
      expect(after.imageReference).toMatch(/@sha256:[a-f0-9]{64}$/);
      if (fault === 'rotate_image') {
        expect(after.runtimeVersion).not.toBe(before.runtimeVersion);
        expect(after.imageReference).not.toBe(before.imageReference);
      }
      afterSnapshots.push({ fault, snapshot: runtimeProof(after) });
      for (const id of agentIds) {
        const original = agents.find((agent) => agent.id === id)!;
        const agent = await h.get(page, `/api/v1/agents/${id}`);
        expect(agent).toMatchObject({
          id,
          sandbox: { id: sandboxId },
          workspace: { id: workspace.id },
          worktree: { id: original.worktreeId },
        });
      }
    } finally {
      await h.operators.controlFault({ ...control, phase: 'release' });
    }
  }
  await page.goto(`${h.config.baseURL}/agents/${agentIds[0]}`);
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
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
  await expect
    .poll(async () => {
      const deleted = await page.request.get(`${h.config.baseURL}/api/v1/agents/${agentIds[0]}`);
      if (deleted.status() === 404) return true;
      if (!deleted.ok()) return false;
      const retained = object(await deleted.json());
      return retained.id === agentIds[0] && retained.status === 'deleted';
    })
    .toBe(true);
  const survivingAgentId = (await h.get<{ id: string }>(page, `/api/v1/agents/${agentIds[1]}`)).id;
  expect(survivingAgentId).toBe(agentIds[1]);
  h.proveJourney({
    owner: h.fixture.accounts.owner.subject,
    bootstrap,
    sandboxId,
    workspaceId: workspace.id,
    agents,
    workspaceAgentIds,
    expectedManagedImageHash: hashText(h.fixture.provenance.runtimeImage),
    before: runtimeProof(before),
    after: afterSnapshots,
    retry: {
      operationId: retry.operationId,
      agentId: retry.agent.id,
      commands: retry.commands.map(commandProof),
    },
    deletedAgentId: agentIds[0],
    survivingAgentId,
  });
});
