import type { Agent, AgentTranscript } from '@chronoai/cma-sdk';
import { test, expect } from './owner-harness';
import { ownerCase } from './owner-environment';
import { identifier } from './environment';

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
  const readAgent = () => h.read<Agent>(page, `/api/v1/agents/${agentId}`);
  const initial = await readAgent();
  const worktreeId = identifier(initial.worktree.id, 'owner-created Worktree');
  const lineage = {
    id: agentId,
    sandbox: { id: sandboxId },
    workspace: { id: workspaceId },
    worktree: {
      id: worktreeId,
      path: initial.worktree.path,
      branch: initial.worktree.branch,
      base_commit: initial.worktree.base_commit,
    },
  };
  expect(initial).toMatchObject(lineage);
  const firstText = firstAnswer.turns[0]!.messages.filter((entry) => entry.role === 'assistant')
    .map((entry) => entry.content)
    .join('\n');
  expect(firstText).toContain(`OWNER-${h.run}`);
  expect(firstText).toContain(initial.worktree.path);
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
  expect(await readAgent()).toMatchObject(lineage);
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
  expect(await readAgent()).toMatchObject(lineage);
  expect(h.commands.map((command) => command.path)).toEqual([
    '/api/v1/sandboxes',
    `/api/v1/sandboxes/${sandboxId}/workspaces`,
    `/api/v1/workspaces/${workspaceId}/agents`,
    `/api/v1/sandboxes/${sandboxId}/pause`,
    `/api/v1/sandboxes/${sandboxId}/revive`,
  ]);
  await h.capture(page, 'resumed');
  const message =
    'Without running commands or modifying files, repeat the marker from my first message and the working directory you reported.';
  h.armContinuation(agentId, worktreeId, message);
  await page.locator('#agent-message').fill(message);
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect
    .poll(
      async () => {
        const value = await transcript();
        return (
          value.turns.length === 2 &&
          value.turns[1]?.outcome.type === 'completed' &&
          value.turns[1].messages.some(
            (entry) => entry.role === 'assistant' && entry.content.trim().length > 0,
          )
        );
      },
      { timeout: 180_000, intervals: [1500, 3000, 5000] },
    )
    .toBe(true);
  const secondAnswer = await transcript();
  expect(secondAnswer.turns[0]).toEqual(firstAnswer.turns[0]);
  expect(
    secondAnswer.turns[1]!.messages.filter((entry) => entry.role === 'user').map(
      (entry) => entry.content,
    ),
  ).toEqual([message]);
  const answer = secondAnswer.turns[1]!.messages.filter((entry) => entry.role === 'assistant')
    .map((entry) => entry.content)
    .join('\n');
  expect(answer).toContain(`OWNER-${h.run}`);
  expect(answer).toContain(initial.worktree.path);
  expect(await h.completedContinuation()).toBe(secondAnswer.turns[1]!.turn_sequence);
  expect(await readAgent()).toMatchObject(lineage);
  expect(h.commands.filter((command) => command.path === '/api/v1/agui/run')).toHaveLength(1);
  await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toHaveCount(0);
  await h.capture(page, 'second-answer');
  const completedCommands = [...h.commands];
  await h.close(page);
  page = await h.open();
  await page.goto(`${h.config.baseURL}/agents/${agentId}`);
  await expect(page.locator('#agent-message')).toBeEnabled({ timeout: 180_000 });
  expect((await transcript()).turns).toEqual(secondAnswer.turns);
  expect(await readAgent()).toMatchObject(lineage);
  expect(h.commands).toEqual(completedCommands);
  expect(h.journal.resources).toHaveLength(3);
  await h.capture(page, 'second-browser-return');
  h.journal.record({
    action: 'owner_baseline_observed',
    sandboxId,
    workspaceId,
    agentId,
    worktreeId,
    firstAnswerPreserved: true,
    secondAnswerCompleted: true,
    sameWorktreePreserved: true,
  });
});
