import { expect, test } from '@playwright/test';
import { randomUUID, createHash } from 'node:crypto';
import { readFile, rename, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import type { BrowserControl, BrowserControlChange } from '../../sdk/typescript/src/index';

test('real Web input exclusively controls the native Agent browser through public authorization and receipts', async ({
  page,
  context,
  request,
  baseURL,
}, info) => {
  const directory = process.env.CMA_BROWSER_RELAY_DIRECTORY!;
  const origins = JSON.parse(await readFile(join(directory, 'public-ready.json'), 'utf8'));
  const root = '/api/v1/agents/agt_shared_read';
  const environment = await request.get(origins.owner + root + '/environment');
  expect(environment.status()).toBe(200);
  const initial = await environment.json();
  expect(initial.browser.state).toBe('not_enabled');
  const incarnation = initial.runtime_incarnation;
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, id: 'agt_shared_read' };
  boundary.sandbox = { ...boundary.sandbox, owner: 'owner-a' };
  boundary.workspace = {
    ...boundary.workspace,
    agents: boundary.workspace.agents.map((agent) => ({ ...agent, id: 'agt_shared_read' })),
  };
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'owner-a',
        display_name: 'Control proof owner',
        capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
      },
    }),
  );
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: {
        owner: 'owner-a',
        ready: true,
        checked_at: localTime,
        services: [],
      },
    }),
  );
  let access = 'owner';
  let drop: string | undefined;
  const mutations: BrowserControlChange[] = [];
  const receipts: BrowserControl[] = [];
  let privateKey = '';
  let taskInputs = 0;
  page.on('request', (outgoing) => {
    if (
      outgoing.method() === 'POST' &&
      /\/(responses|messages|inputs)(\?|$)/.test(new URL(outgoing.url()).pathname)
    )
      taskInputs += 1;
  });
  await page.route(
    /\/api\/v1\/agents\/agt_shared_read\/(browser(?:\/control)?|environment)(\?|$)/,
    async (route) => {
      const outgoing = route.request();
      const url = new URL(outgoing.url());
      const control = url.pathname.endsWith('/control');
      const command =
        control && outgoing.method() === 'POST'
          ? (outgoing.postDataJSON() as BrowserControlChange)
          : undefined;
      if (command) {
        mutations.push(command);
        privateKey = outgoing.headers()['x-cma-browser-client']!;
      }
      const response = await route.fetch({ url: origins[access] + url.pathname + url.search });
      if (command) console.log('Control mutation', command.command, response.status());
      if (control && response.ok()) receipts.push(await response.json());
      if (command && command.command === drop) {
        drop = undefined;
        await route.abort();
        return;
      }
      await route.fulfill({ response });
    },
  );
  const rpc = async (value: Record<string, unknown>) => {
    const identifier = randomUUID();
    await writeFile(join(directory, identifier + '.tmp'), JSON.stringify(value));
    await rename(
      join(directory, identifier + '.tmp'),
      join(directory, identifier + '.request.json'),
    );
    const path = join(directory, identifier + '.response.json');
    for (let attempt = 0; attempt < 1000; attempt += 1) {
      const bytes = await readFile(path).catch(() => undefined);
      if (bytes) {
        await unlink(path);
        return JSON.parse(bytes.toString());
      }
      await delay(40);
    }
    throw new Error('Actual native/runtime proof RPC deadline');
  };
  const native = (text: string) =>
    rpc({ t: 'native_control', command: 'input', input: { kind: 'text', text } });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_shared_read');
  await page.getByRole('textbox', { name: 'Message' }).fill('Keep this conversation draft');
  const open = page.getByRole('button', { name: 'Show Agent computer', exact: true });
  if (await open.isVisible()) await open.click();
  await page.getByRole('tab', { name: 'Browser', exact: true }).click();
  await page.getByRole('button', { name: 'Enable browser', exact: true }).click();
  await expect(page.getByText('Live', { exact: true })).toBeVisible({ timeout: 15000 });
  await rpc({ t: 'native_control', command: 'navigate' });
  const image = page.getByAltText('Agent browser page');
  await expect(image).toHaveJSProperty('naturalWidth', 1280);
  await expect(page.getByRole('button', { name: 'Take control' })).toBeEnabled({ timeout: 10000 });
  drop = 'acquire';
  await page.getByRole('button', { name: 'Take control' }).click();
  await expect.poll(() => mutations.filter((value) => value.command === 'acquire').length).toBe(1);
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  expect((await native('-LOST-ACQUIRE')).refused).toBe(true);
  await page.getByRole('button', { name: 'Check original operation' }).click();
  await expect(page.getByText('You are controlling', { exact: true })).toBeVisible();
  const acquisition = mutations.find((value) => value.command === 'acquire')!;
  const acquired = receipts.find((value) => value.operation_id === acquisition.operation_id)!;
  const browserContext = acquired.context_id;
  const bounds = (await image.boundingBox())!;
  await image.click({
    position: { x: (bounds.width * 100) / 1280, y: (bounds.height * 90) / 720 },
  });
  await expect(page.getByRole('button', { name: 'Return to Agent' })).toBeEnabled();
  await page.getByLabel('Text to type in browser').fill('USER-17');
  const [, blocked] = await Promise.all([
    page.getByRole('button', { name: 'Type text' }).click(),
    native('-FORBIDDEN'),
  ]);
  expect(blocked.refused).toBe(true);
  await expect.poll(async () => (await rpc({ t: 'form_state' })).values.at(-1)).toBe('USER-17');
  await expect(page.getByRole('button', { name: 'Return to Agent' })).toBeEnabled();
  await page.getByRole('application').press('Backspace');
  await expect.poll(async () => (await rpc({ t: 'form_state' })).values.at(-1)).toBe('USER-1');
  await page.getByRole('application').press('7');
  await expect.poll(async () => (await rpc({ t: 'form_state' })).values.at(-1)).toBe('USER-17');
  const controlPath = root + '/browser/control';
  const scope = {
    runtime_incarnation: incarnation,
    context_id: browserContext,
    client_id: acquisition.client_id,
  };
  const auditPath = controlPath + '?' + new URLSearchParams(scope);
  const auditResponse = await request.get(origins.viewer + auditPath);
  expect(auditResponse.status()).toBe(200);
  const audit = await auditResponse.json();
  expect(audit.lease_token).toBeNull();
  expect(JSON.stringify(audit)).not.toContain('USER-17');
  expect(JSON.stringify(audit)).not.toContain(privateKey);
  expect(audit.audit.some((entry: { kind: string }) => entry.kind === 'input')).toBe(true);
  const competitor = {
    ...scope,
    client_id: randomUUID(),
    command: 'acquire',
    operation_id: randomUUID(),
    expected_revision: audit.revision,
  };
  const otherHeaders = { 'x-cma-browser-client': randomUUID() };
  expect(
    (
      await request.post(origins.owner + controlPath, {
        headers: otherHeaders,
        data: {
          ...competitor,
          command: 'input',
          operation_id: randomUUID(),
          lease_token: acquired.lease_token,
          input: { kind: 'text', text: '-COMPETING' },
        },
      })
    ).status(),
  ).toBe(409);
  expect((await rpc({ t: 'form_state' })).values.at(-1)).toBe('USER-17');
  expect(
    (
      await request.post(origins.owner + controlPath, { data: competitor, headers: otherHeaders })
    ).status(),
  ).toBe(409);
  expect(
    (
      await request.get(origins.owner + auditPath + '&operation_id=' + acquisition.operation_id, {
        headers: otherHeaders,
      })
    ).status(),
  ).toBe(409);
  expect(
    (
      await request.post(origins.viewer + controlPath, { data: competitor, headers: otherHeaders })
    ).status(),
  ).toBe(403);
  expect((await request.get(origins.foreign + auditPath)).status()).toBe(404);
  expect(
    (
      await request.post(origins.owner + controlPath, {
        data: { ...competitor, runtime_incarnation: 'old' },
        headers: otherHeaders,
      })
    ).status(),
  ).toBe(409);
  drop = 'input';
  await page.getByLabel('Text to type in browser').fill('-ONCE');
  await page.getByRole('button', { name: 'Type text' }).click();
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Check original operation' }).click();
  await expect(page.getByText('You are controlling', { exact: true })).toBeVisible();
  expect((await rpc({ t: 'form_state' })).values.at(-1)).toBe('USER-17-ONCE');
  const inputObservedAt = Date.now();
  await expect
    .poll(async () =>
      Date.parse((await page.locator('.agent-browser figure time').getAttribute('datetime'))!),
    )
    .toBeGreaterThan(inputObservedAt);
  await expect(image).toHaveJSProperty('naturalWidth', 1280);
  await page.getByText('Control log', { exact: true }).click();
  await page.screenshot({ path: info.outputPath('user-control-audit.png'), fullPage: true });
  const imageData = (await image.getAttribute('src'))!.split(',')[1]!;
  const jpeg = Buffer.from(imageData, 'base64');
  await writeFile(join(directory, 'public-user-input.jpg'), jpeg);
  drop = 'return';
  await page.getByRole('button', { name: 'Return to Agent' }).click();
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  const resumed = await native('-AGENT');
  expect(resumed.refused).toBe(false);
  expect(resumed.values.at(-1)).toBe('USER-17-ONCE-AGENT');
  expect(resumed.pages).toBe(1);
  await page.getByRole('button', { name: 'Check original operation' }).click();
  await expect(page.getByText('Agent is controlling', { exact: true })).toBeVisible();
  const returned = await (await request.get(origins.owner + auditPath)).json();
  expect(returned.context_id).toBe(browserContext);
  expect(returned.audit.some((entry: { kind: string }) => entry.kind === 'returned')).toBe(true);
  expect(
    (
      await request.post(origins.owner + controlPath, {
        headers: { 'x-cma-browser-client': privateKey },
        data: {
          ...scope,
          command: 'input',
          operation_id: randomUUID(),
          expected_revision: returned.revision,
          lease_token: acquired.lease_token,
          input: { kind: 'text', text: '-OLD-TOKEN' },
        },
      })
    ).status(),
  ).toBe(409);
  await page.getByRole('button', { name: 'Take control' }).click();
  await expect(page.getByText('You are controlling', { exact: true })).toBeVisible();
  await context.setOffline(true);
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  await delay(15500);
  const expired = await native('-EXPIRED');
  expect(expired.refused).toBe(false);
  expect(expired.values.at(-1)).toBe('USER-17-ONCE-AGENT-EXPIRED');
  const expiryAudit = await (await request.get(origins.viewer + auditPath)).json();
  expect(expiryAudit.state).toBe('agent');
  expect(expiryAudit.lease_token).toBeNull();
  expect(expiryAudit.audit.some((entry: { kind: string }) => entry.kind === 'expired')).toBe(true);
  await context.setOffline(false);
  await expect(page.getByText('Agent is controlling', { exact: true })).toBeVisible({
    timeout: 10000,
  });
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    await expect(page.getByRole('button', { name: 'Take control' })).toBeVisible();
    await page.screenshot({ path: info.outputPath(`control-mobile-${width}.png`), fullPage: true });
  }
  await page.getByRole('button', { name: 'Take control' }).click();
  await expect(page.getByText('You are controlling', { exact: true })).toBeVisible();
  access = 'viewer';
  await expect(page.getByText('Browser control is not permitted.', { exact: true })).toBeVisible({
    timeout: 10000,
  });
  await expect(page.getByRole('application')).toHaveAttribute('aria-disabled', 'true');
  const before = (await rpc({ t: 'form_state' })).values;
  await page.getByRole('application').dispatchEvent('keydown', { key: 'X' });
  expect((await rpc({ t: 'form_state' })).values).toEqual(before);
  access = 'owner';
  expect(
    (
      await request.put(origins.owner + root + '/browser', {
        data: { runtime_incarnation: incarnation, enabled: false },
      })
    ).status(),
  ).toBe(200);
  expect(
    (
      await request.put(origins.owner + root + '/browser', {
        data: { runtime_incarnation: incarnation, enabled: true },
      })
    ).status(),
  ).toBe(200);
  expect(
    (
      await request.post(origins.owner + controlPath, { data: competitor, headers: otherHeaders })
    ).status(),
  ).toBe(409);
  await page.getByRole('button', { name: 'Back to conversation' }).click();
  await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue(
    'Keep this conversation draft',
  );
  expect(taskInputs).toBe(0);
  await page.unrouteAll({ behavior: 'wait' });
  await page.close();
  await writeFile(
    join(directory, 'web-chain-result.json'),
    JSON.stringify(
      {
        passed: true,
        context_id: browserContext,
        incarnation,
        samePageLoadsBeforeExplicitReplacement: resumed.pages,
        inputValues: expired.values,
        publicAuditKinds: expiryAudit.audit.map((entry: { kind: string }) => entry.kind),
        lostReceipts: ['acquire', 'input', 'return'],
        offlineExpiry: true,
        competingClient: true,
        unpublishedClientKey: true,
        staleToken: true,
        staleIncarnation: true,
        accountPermission: true,
        contextReplacement: true,
        taskInputs,
        frameSha256: createHash('sha256').update(jpeg).digest('hex'),
        boundaries: [
          'fixture IdP and resource store',
          'private file RPC transport',
          'actual Core/public HTTP/SDK/Web/native Agent/runtime/Chromium',
        ],
      },
      null,
      2,
    ),
  );
  await writeFile(join(directory, 'public-stop.json'), '{}');
  await writeFile(join(directory, 'stop.json'), '{}');
});
