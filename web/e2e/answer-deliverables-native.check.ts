import { mkdir, readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('NATIVE receipts reach answer cards and authenticated public downloads without invented associations', async ({
  page,
  request,
  baseURL,
}) => {
  const origin = process.env.CMA_NATIVE_DELIVERY_ORIGIN!;
  const agentId = `agt_${'a'.repeat(32)}`;
  const root = `/api/v1/agents/${agentId}`;
  const agent = await request.get(origin + root);
  expect(agent.status()).toBe(200);
  const events = await request.get(origin + root + '/events?after=0&limit=100');
  expect(events.status()).toBe(200);
  const boundary = new LocalStartup();
  boundary.agent = await agent.json();
  boundary.workspace = {
    ...boundary.workspace,
    agents: boundary.workspace.agents.map((entry) => ({ ...entry, id: agentId })),
  };
  boundary.sandbox = { ...boundary.sandbox, owner: 'owner-a' };
  boundary.events = (await events.json()).items;
  expect(
    boundary.events.find((event) => event.kind.type === 'agent_message')?.kind.content,
  ).toContain('<!--cma-deliverable:v1:');
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'owner-a',
        display_name: 'Native proof owner',
        capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
      },
    }),
  );
  await page.route(`**${root}/answers/**`, async (route) => {
    const url = new URL(route.request().url());
    const response = await request.get(origin + url.pathname + url.search);
    await route.fulfill({ response });
  });
  await page.goto(`/agents/${agentId}`);
  const answer = page
    .getByRole('article')
    .filter({ hasText: 'Delivered the file and persistent document.' });
  await expect(answer.getByRole('list', { name: 'Answer deliverables' })).toBeVisible();
  for (const [name, digest] of [
    ['report.txt', 'f6c042409c945c141af65759a07055f9589a486941a285d38d361db0bf3f8400'],
    ['Native document.md', '6f09a58c411f877c6926b5f4d711df6fb6c8229335bd77d3de54b95021130e92'],
  ]) {
    const downloaded = page.waitForEvent('download');
    await answer.getByRole('button', { name: `Download ${name}` }).click();
    const file = await downloaded;
    expect(file.suggestedFilename()).toBe(name);
    const bytes = await readFile((await file.path())!);
    expect(createHash('sha256').update(bytes).digest('hex')).toBe(digest);
  }
  await page.reload();
  await expect(answer.getByRole('button', { name: 'Download report.txt' })).toBeEnabled();
  await expect(answer.getByRole('button', { name: 'Download Native document.md' })).toBeEnabled();
  await mkdir('/tmp/cma-ticket07-evidence', { recursive: true });
  await page.screenshot({
    path: '/tmp/cma-ticket07-evidence/native-api-cards.png',
    fullPage: true,
  });
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.writes).toEqual([]);
});
