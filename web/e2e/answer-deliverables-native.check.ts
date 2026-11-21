import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { expect, test } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

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
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: { owner: 'owner-a', ready: true, checked_at: localTime, services: [] },
    }),
  );
  await page.route(`**${root}/answers/**`, async (route) => {
    const url = new URL(route.request().url());
    const prefer = route.request().headers().prefer;
    expect(prefer).toBe('cma-native-attachment-media');
    const response = await request.get(origin + url.pathname + url.search, {
      headers: { Prefer: prefer ?? '' },
    });
    await route.fulfill({ response });
  });
  await page.goto(`/agents/${agentId}`);
  const answer = page
    .getByRole('article')
    .filter({ hasText: 'Delivered the native files and persistent document.' });
  await expect(answer.getByRole('list', { name: 'Answer deliverables' })).toBeVisible();
  await expect(page.getByText('Your CMA account changed.', { exact: false })).toHaveCount(0);
  const listed = await request.get(origin + root + '/answers/event:5/deliverables');
  expect(listed.status()).toBe(200);
  const { items } = (await listed.json()) as {
    items: { reference: { name: string; size_bytes: number; sha256: string } }[];
  };
  expect(items.map(({ reference }) => reference.name).sort()).toEqual(
    [
      'Native document.md',
      'readme.md',
      'report.csv',
      'report.docx',
      'report.xlsx',
      'report.zip',
      'report.json',
    ].sort(),
  );
  await expect(answer.getByRole('button', { name: /^Download / })).toHaveCount(items.length);
  await page.reload();
  await expect(answer.getByRole('button', { name: /^Download / })).toHaveCount(items.length);
  for (const { reference } of items) {
    const { name, sha256, size_bytes } = reference;
    const downloaded = page.waitForEvent('download');
    await answer.getByRole('button', { name: `Download ${name}` }).click();
    const file = await downloaded;
    expect(file.suggestedFilename()).toBe(name);
    const bytes = await readFile((await file.path())!);
    expect(bytes.length).toBe(size_bytes);
    expect(createHash('sha256').update(bytes).digest('hex')).toBe(sha256);
  }
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.writes).toEqual([]);
});
