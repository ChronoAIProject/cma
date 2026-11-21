import { expect, type Page } from '@playwright/test';
import { readFile, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

async function policy(page: Page, directory: string, mode: 'enabled' | 'disabled') {
  await writeFile(join(directory, 'policy-mode.tmp'), JSON.stringify({ mode }));
  await rename(join(directory, 'policy-mode.tmp'), join(directory, 'policy-mode.json'));
  const response = await page.request.get('/api/v2/ui/task-experience');
  expect(response.status()).toBe(200);
  expect(response.headers()['cache-control']).toBe('private, no-store');
  const value = await response.json();
  expect(value.account_id).toBe('owner');
  expect(Object.values(value.capabilities)).toEqual(Array(5).fill(mode === 'enabled'));
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  return value;
}

export async function disableDuringControl(page: Page, directory: string, agentId: string) {
  const reading = await policy(page, directory, 'disabled');
  await expect(page.getByLabel('Text to type in browser')).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Return to Agent', exact: true })).toBeEnabled();
  await expect(page.getByRole('application', { name: 'Agent browser input' })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  const creation = JSON.parse(await readFile(join(directory, 'creation.json'), 'utf8'));
  const original = await page.request.post('/api/v1/agents', {
    headers: { 'idempotency-key': creation.key },
    data: creation.body,
  });
  expect(original.status()).toBe(202);
  const receipt = await original.json();
  expect(receipt.id).toBe(creation.receipt.id);
  const fresh = await page.request.post('/api/v1/agents', {
    headers: { 'idempotency-key': crypto.randomUUID() },
    data: creation.body,
  });
  expect(fresh.status()).toBe(403);
  const history = await page.request.get(`/api/v1/agents/${agentId}/transcript`);
  expect(history.status()).toBe(200);
  const transcript = await history.json();
  expect(transcript.turns).toHaveLength(1);
  expect(transcript.turns[0].owner_message).toBe(creation.body.initial_message);
  const origin = new URL(page.url()).origin;
  const revoked = await page.context().browser()!.newContext({ baseURL: origin });
  let denial;
  try {
    await revoked.addCookies([{ name: 'integrated-revoked', value: 'true', url: origin }]);
    const response = await revoked.request.post('/api/v1/agents', {
      headers: { 'idempotency-key': creation.key },
      data: creation.body,
    });
    denial = { status: response.status(), body: await response.json() };
    expect(denial.status).toBe(403);
  } finally {
    await revoked.close();
  }
  await writeFile(
    join(directory, 'policy-disabled-result.json'),
    JSON.stringify(
      {
        reading,
        originalReceipt: receipt,
        deniedFresh: { status: fresh.status(), body: await fresh.json() },
        deniedRevokedOriginal: denial,
        transcript,
        returnVisible: true,
        inputDisabled: true,
        boundary:
          'Real enabled and default-disabled public compositions share retained Mongo/runtime; synthetic external authority supplies current request capabilities.',
      },
      null,
      2,
    ),
  );
}

export async function restoreAfterReturn(page: Page, directory: string) {
  const reading = await policy(page, directory, 'enabled');
  await expect(page.getByRole('button', { name: 'Take control', exact: true })).toBeEnabled();
  await writeFile(
    join(directory, 'policy-restored-result.json'),
    JSON.stringify({ reading, returned: true }, null, 2),
  );
}
