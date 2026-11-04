import { expect, type Page } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function reconnectRunningTurn(
  page: Page,
  directory: string,
  agentId: string,
  input: string,
) {
  await expect
    .poll(async () =>
      readFile(join(directory, 'second-running.json'), 'utf8')
        .then(JSON.parse)
        .catch(() => null),
    )
    .not.toBeNull();
  const beforeResponse = await page.request.get(`/api/v1/agents/${agentId}/transcript`);
  expect(beforeResponse.status()).toBe(200);
  const before = await beforeResponse.json();
  expect(before.turns).toHaveLength(2);
  expect(before.turns[1].owner_message).toBe(input);
  expect(before.turns[1].outcome).toEqual({ type: 'active' });
  const stale = await page
    .context()
    .browser()!
    .newContext({ baseURL: new URL(page.url()).origin });
  let refusal;
  try {
    const source = before.turns[0];
    const answer = source.messages.at(-1);
    const response = await stale.request.post(`/api/v2/agents/${agentId}/responses`, {
      headers: { 'idempotency-key': crypto.randomUUID() },
      data: {
        input: 'Explain the flexibility trade-off in the comparison.',
        stream: true,
        answer_context: { turn_sequence: source.turn_sequence, answer_sequence: answer.sequence },
      },
    });
    refusal = { status: response.status(), body: await response.json() };
    expect(refusal.status).toBe(409);
    expect(refusal.body.error.code).toBe('agent_command_not_admissible');
  } finally {
    await stale.close();
  }
  await page.context().setOffline(true);
  await expect.poll(() => page.evaluate(() => navigator.onLine)).toBe(false);
  await page.context().setOffline(false);
  await expect.poll(() => page.evaluate(() => navigator.onLine)).toBe(true);
  await page.reload();
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible();
  const afterResponse = await page.request.get(`/api/v1/agents/${agentId}/transcript`);
  expect(afterResponse.status()).toBe(200);
  const after = await afterResponse.json();
  expect(after.turns).toHaveLength(2);
  expect(after.turns[1].turn_sequence).toBe(before.turns[1].turn_sequence);
  expect(after.turns[1].owner_message).toBe(input);
  expect(after.turns[1].outcome).toEqual({ type: 'active' });
  await writeFile(
    join(directory, 'reconnect-result.json'),
    JSON.stringify(
      { before, after, staleIndependentClient: refusal, offlineAndOnlineObserved: true },
      null,
      2,
    ),
  );
  await writeFile(join(directory, 'release-second.json'), JSON.stringify({ released: Date.now() }));
}
