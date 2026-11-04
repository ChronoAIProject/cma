import { expect, type Page, type TestInfo } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { disableDuringControl, restoreAfterReturn } from './integrated-policy';

export async function composedControl(
  page: Page,
  info: TestInfo,
  directory: string,
  agentId: string,
) {
  const root = '/api/v1/agents/' + agentId;
  const origin = new URL(page.url()).origin;
  const transcriptResponse = await page.request.get(origin + root + '/transcript');
  expect(transcriptResponse.status()).toBe(200);
  const transcript = await transcriptResponse.json();
  const turn = transcript.turns[0].turn_sequence;
  const commands: { command: string; operation: string; context: string; status: number }[] = [];
  let drop: string | undefined = 'acquire';
  let scope: Record<string, string> = {};
  await page.route('**' + root + '/browser/control', async (route) => {
    const request = route.request();
    if (request.method() !== 'POST') return route.continue();
    const body = request.postDataJSON();
    const response = await route.fetch();
    const receipt = await response.json();
    if (body.command === 'acquire')
      scope = {
        runtime_incarnation: body.runtime_incarnation,
        context_id: receipt.context_id,
        client_id: body.client_id,
      };
    commands.push({
      command: body.command,
      operation: body.operation_id,
      context: receipt.context_id,
      status: response.status(),
    });
    expect(response.status()).toBe(200);
    if (body.command === drop) {
      drop = undefined;
      await route.abort('failed');
    } else await route.fulfill({ response });
  });
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await composer.fill('Keep the comparison draft through control and replay.');
  const show = page.getByRole('button', { name: 'Show Agent computer', exact: true });
  if (await show.isVisible()) await show.click();
  await page.getByRole('tab', { name: 'Browser', exact: true }).click();
  const image = page.getByAltText('Agent browser page');
  await expect(image).toHaveJSProperty('naturalWidth', 1280);
  const recovery = page.getByRole('button', { name: 'Check original operation', exact: true });
  if (await recovery.isVisible()) await recovery.click();
  await page.getByRole('button', { name: 'Take control', exact: true }).click();
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Check original operation', exact: true }).click();
  await expect(page.getByText('You are controlling', { exact: true })).toBeVisible();
  const bounds = (await image.boundingBox())!;
  await image.click({
    position: { x: (bounds.width * 100) / 1280, y: (bounds.height * 90) / 720 },
  });
  await expect(page.getByRole('button', { name: 'Return to Agent', exact: true })).toBeEnabled();
  drop = 'input';
  await page.getByLabel('Text to type in browser').fill('TICKET19-ONCE');
  await page.getByRole('button', { name: 'Type text', exact: true }).click();
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Check original operation', exact: true }).click();
  await expect(page.getByText('You are controlling', { exact: true })).toBeVisible();
  await expect
    .poll(
      async () =>
        JSON.parse(await readFile(join(directory, 'native-form.json'), 'utf8').catch(() => '{}'))
          .values,
    )
    .toEqual(['TICKET19-ONCE']);
  await page.screenshot({ path: info.outputPath('same-turn-browser-control.png') });
  if (process.env.CMA_INTEGRATED_POLICY === '1')
    await disableDuringControl(page, directory, agentId);
  drop = 'return';
  await page.getByRole('button', { name: 'Return to Agent', exact: true }).click();
  await expect(page.getByText('Control state unknown', { exact: true })).toBeVisible();
  const recoveredReturn = page.waitForResponse(
    (response) =>
      response.request().method() === 'GET' &&
      new URL(response.url()).pathname === root + '/browser/control',
  );
  await page.getByRole('button', { name: 'Check original operation', exact: true }).click();
  await expect(page.getByText('Agent is controlling', { exact: true })).toBeVisible();
  const auditResponse = await recoveredReturn;
  expect(auditResponse.status()).toBe(200);
  const audit = await auditResponse.json();
  expect(audit.state).toBe('agent');
  expect(audit.context_id).toBe(commands[0]!.context);
  expect(commands.every((command) => command.context === audit.context_id)).toBe(true);
  expect(audit.audit.some((entry: { kind: string }) => entry.kind === 'input')).toBe(true);
  expect(audit.audit.some((entry: { kind: string }) => entry.kind === 'returned')).toBe(true);
  if (process.env.CMA_INTEGRATED_POLICY === '1') await restoreAfterReturn(page, directory);
  const answer = page.getByRole('article').filter({ hasText: 'The comparison is complete.' });
  await answer.getByRole('button', { name: 'View plan and replay', exact: true }).click();
  const replay = page.getByRole('region', { name: 'Operation record replay' });
  await expect(replay.getByText('History · Read only', { exact: true })).toBeVisible();
  await expect(replay.getByText('Turn ' + turn, { exact: true })).toBeVisible();
  await expect(replay.getByRole('article')).toContainText('report.txt');
  await page.screenshot({ path: info.outputPath('same-turn-operation-replay.png') });
  await replay.getByRole('button', { name: 'Return Live', exact: true }).click();
  await expect(replay.getByText('Live · Recorded operations', { exact: true })).toBeVisible();
  await expect(composer).toHaveValue('Keep the comparison draft through control and replay.');
  await composer.fill('');
  await page.getByRole('tab', { name: 'Browser', exact: true }).click();
  await writeFile(
    join(directory, 'control-result.json'),
    JSON.stringify(
      {
        passed: true,
        agentId,
        turn,
        context: audit.context_id,
        scope,
        commands,
        audit: audit.audit,
      },
      null,
      2,
    ),
  );
}
