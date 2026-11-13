import type { AgentTranscript } from '@chronoai/cma-sdk';
import type { Page } from '@playwright/test';
import { expect, type Harness, type Launch } from './harness';
import { assertVisitorSandbox, completedFirstAnswer } from './journey-oracles';

/** Read the actual admitted resource and completed answer, not only Launch.ready. */
export async function firstAnswer(h: Harness, page: Page, launch: Launch, subject: string) {
  const sandboxId = launch.resolution.sandbox.id;
  const agentId = launch.resolution.agent.id;
  expect(sandboxId).toBeTruthy();
  expect(agentId).toBeTruthy();
  const sandbox = await h.get<unknown>(page, `/api/v1/sandboxes/${sandboxId}`);
  assertVisitorSandbox(sandbox, sandboxId!, subject);
  let transcript: AgentTranscript | undefined;
  await expect
    .poll(
      async () => {
        transcript = await h.get<AgentTranscript>(page, `/api/v1/agents/${agentId}/transcript`);
        return completedFirstAnswer(transcript, agentId!);
      },
      { timeout: 180_000, intervals: [1500, 3000, 5000] },
    )
    .toBe(true);
  await expect(page.locator('.message--assistant .agent-content').first()).toContainText(/\S/);
  await expect(page.getByRole('button', { name: 'Stop response', exact: true })).toHaveCount(0);
  return transcript!;
}
