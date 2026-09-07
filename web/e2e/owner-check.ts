import type { AgentTranscript } from '@chronoai/cma-sdk';
import { test, expect } from './owner-harness';
import { ownerCase } from './owner-environment';

test(ownerCase, async ({ owner: h }) => {
  let page = await h.open();
  const sandboxId = await h.create(page, 'sandbox');
  await expect
    .poll(
      async () => (await h.read<{ status: string }>(page, `/api/v1/sandboxes/${sandboxId}`)).status,
      { timeout: 540_000, intervals: [1500, 3000, 5000] },
    )
    .toBe('active');
  const workspaceId = await h.create(page, 'workspace', sandboxId);
  await expect
    .poll(
      async () =>
        (await h.read<{ status: string }>(page, `/api/v1/workspaces/${workspaceId}`)).status,
      { timeout: 180_000, intervals: [1500, 3000, 5000] },
    )
    .toBe('ready');
  const agentId = await h.create(page, 'agent', workspaceId);
  const transcript = () => h.read<AgentTranscript>(page, `/api/v1/agents/${agentId}/transcript`);
  await expect
    .poll(
      async () => {
        const value = await transcript();
        return (
          value.turns.length === 1 &&
          value.turns[0]?.outcome.type === 'completed' &&
          value.turns[0].messages.some(
            (message) => message.role === 'assistant' && message.content.trim().length > 0,
          )
        );
      },
      { timeout: 180_000, intervals: [1500, 3000, 5000] },
    )
    .toBe(true);
  const firstAnswer = await transcript();
  await page.goto(`${h.config.baseURL}/agents/${agentId}`);
  await expect
    .poll(async () =>
      (await page.locator('.message--assistant .agent-content').allTextContents()).some(
        (text) => text.trim().length > 0,
      ),
    )
    .toBe(true);
  await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toHaveCount(0);
  await h.capture(page, 'first-answer');
  const creates = [...h.commands];
  await h.close(page);
  page = await h.open();
  await page.goto(`${h.config.baseURL}/agents/${agentId}`);
  await expect(page.locator('#agent-message')).toBeEnabled({ timeout: 180_000 });
  expect((await transcript()).turns).toEqual(firstAnswer.turns);
  expect(h.commands).toEqual(creates);
  await h.capture(page, 'browser-return');
  await page.goto(`${h.config.baseURL}/sandboxes/${sandboxId}`);
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled({
    timeout: 180_000,
  });
  await h.capture(page, 'paused');
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled({
    timeout: 540_000,
  });
  await page.goto(`${h.config.baseURL}/agents/${agentId}`);
  await expect(page.locator('#agent-message')).toBeEnabled({ timeout: 180_000 });
  expect((await transcript()).turns).toEqual(firstAnswer.turns);
  expect(h.commands.map((command) => command.path)).toEqual([
    '/api/v1/sandboxes',
    `/api/v1/sandboxes/${sandboxId}/workspaces`,
    `/api/v1/workspaces/${workspaceId}/agents`,
    `/api/v1/sandboxes/${sandboxId}/pause`,
    `/api/v1/sandboxes/${sandboxId}/revive`,
  ]);
  await h.capture(page, 'resumed');
  h.journal.record({
    action: 'owner_baseline_observed',
    sandboxId,
    workspaceId,
    agentId,
    firstAnswerPreserved: true,
  });
});
