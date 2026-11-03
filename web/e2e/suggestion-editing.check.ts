import { expect, test } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

for (const width of [1440, 390, 320]) {
  for (const theme of ['light', 'dark']) {
    test(`edit suggestion ${width}px ${theme}`, async ({ page, baseURL }, info) => {
      const fixture = new LocalAgentV2();
      page.on('pageerror', (error) => fixture.errors.push(String(error)));
      const suggestion = 'Add explicit comparison criteria and explain the trade-offs.';
      const edited = '保留预算上限\nCompare latency and cost.';
      fixture.append({ type: 'turn_started', turn_sequence: 1 });
      fixture.append({ type: 'message', role: 'user', content: 'Compare PostgreSQL and SQLite.' });
      fixture.append({
        type: 'message',
        role: 'assistant',
        content: 'The comparison is complete.',
      });
      fixture.append({ type: 'turn_completed', turn_sequence: 1 });
      try {
        await fixture.route(page, baseURL!);
        await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
          route.fulfill({
            json: {
              object: 'cma.agent.event_head',
              schema_version: 1,
              agent_id: 'agt_contract',
              incarnation: 'a'.repeat(64),
              through_cursor: `ag1.${'a'.repeat(64)}.4`,
              active_turn: null,
            },
          }),
        );
        const attachmentId = `att_${'a'.repeat(64)}`;
        await page.route('**/api/v1/agents/agt_contract/attachments?*', (route) =>
          route.fulfill({
            status: 201,
            json: {
              id: attachmentId,
              agent_id: 'agt_contract',
              name: 'budget.txt',
              size_bytes: 6,
              media_type: 'text/plain',
              sha256: 'b'.repeat(64),
              path: `/home/attachments/agt_contract/${attachmentId}/budget.txt`,
            },
          }),
        );
        await page.setViewportSize({ width, height: 900 });
        await page.goto('/agents/agt_contract');
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        const field = page.getByRole('textbox', { name: 'Message', exact: true });
        await expect(field).toBeEnabled();
        await field.fill('保留预算上限');
        await page.locator('input[type="file"]').setInputFiles({
          name: 'budget.txt',
          mimeType: 'text/plain',
          buffer: Buffer.from('budget'),
        });
        const attachments = page.getByRole('list', { name: 'Attachments' });
        await expect(attachments).toContainText('Ready');
        const edit = page.getByRole('button', { name: 'Edit suggestion 1 before sending' });
        if (process.env.CMA_SUGGESTION_EDIT_BASELINE === '1') {
          await expect(edit).toHaveCount(0);
          await page.screenshot({
            path: info.outputPath('before.png'),
            fullPage: true,
            animations: 'disabled',
          });
          return;
        }
        await expect(edit).toBeEnabled();
        await edit.scrollIntoViewIfNeeded();
        expect((await edit.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        if (width === 1440) {
          await edit.focus();
          expect(await edit.evaluate((button) => getComputedStyle(button).boxShadow)).not.toBe(
            'none',
          );
          await page.keyboard.press('Enter');
        } else await edit.tap();
        const dialog = page.getByRole('dialog', { name: 'Keep your draft?' });
        await expect(dialog).toBeVisible();
        await page.screenshot({
          path: info.outputPath('conflict.png'),
          fullPage: true,
          animations: 'disabled',
        });
        if (width === 1440) await page.keyboard.press('Escape');
        else await dialog.getByRole('button', { name: 'Cancel', exact: true }).tap();
        await expect(field).toHaveValue('保留预算上限');
        await expect(field).toBeFocused();
        await expect(attachments).toContainText('budget.txt');
        expect(fixture.writes).toHaveLength(0);
        await edit.click();
        await dialog.getByRole('button', { name: 'Append to draft' }).click();
        await expect(field).toHaveValue(`保留预算上限\n${suggestion}`);
        await expect(field).toBeFocused();
        await edit.click();
        await dialog.getByRole('button', { name: 'Replace text' }).click();
        await expect(field).toHaveValue(suggestion);
        await page.getByRole('button', { name: 'Undo replacement' }).click();
        await expect(field).toHaveValue(`保留预算上限\n${suggestion}`);
        await field.fill('');
        await edit.click();
        await expect(dialog).toHaveCount(0);
        await expect(field).toHaveValue(suggestion);
        await expect(field).toBeFocused();
        await field.fill(edited);
        await page.screenshot({
          path: info.outputPath('edited.png'),
          fullPage: true,
          animations: 'disabled',
        });
        let refusal = 0;
        await page.route('**/api/v2/agents/agt_contract/responses', async (route) => {
          refusal += 1;
          if (refusal > 2) return route.fallback();
          fixture.writes.push({
            path: '/api/v2/agents/agt_contract/responses',
            body: route.request().postDataJSON(),
            key: route.request().headers()['idempotency-key'],
          });
          await route.fulfill({
            status: refusal === 1 ? 422 : 503,
            json: {
              error: {
                type: refusal === 1 ? 'invalid_request_error' : 'server_error',
                code: refusal === 1 ? 'invalid_response_input' : 'upstream_response_lost',
                message: refusal === 1 ? 'Input refused.' : 'Receipt lost.',
                param: refusal === 1 ? 'input' : null,
              },
            },
          });
        });
        await page.getByRole('button', { name: 'Send message', exact: true }).click();
        await expect(page.getByText('Not sent', { exact: true })).toBeVisible();
        await expect(field).toHaveValue(edited);
        await expect(attachments).toContainText('budget.txt');
        await page.screenshot({
          path: info.outputPath('failed.png'),
          fullPage: true,
          animations: 'disabled',
        });
        await page.getByRole('button', { name: 'Send message', exact: true }).click();
        const recover = page.getByRole('button', { name: 'Recover pending request' });
        await expect(recover).toBeEnabled();
        await expect(field).toHaveValue(edited);
        await page.screenshot({
          path: info.outputPath('uncertain.png'),
          fullPage: true,
          animations: 'disabled',
        });
        await field.fill('Keep later input');
        await recover.click();
        await expect(page.getByRole('region', { name: 'Next you can ask' })).toHaveCount(0);
        await expect(field).toHaveValue('Keep later input');
        await expect(attachments).toContainText('budget.txt');
        expect(fixture.writes).toHaveLength(3);
        expect(fixture.writes[1]!.body.input).toContain(edited);
        expect(fixture.writes[1]!.body.answer_context).toEqual({
          turn_sequence: 1,
          answer_sequence: 3,
        });
        expect(fixture.writes[2]!.body).toEqual(fixture.writes[1]!.body);
        expect(fixture.writes[2]!.key).toBe(fixture.writes[1]!.key);
        const bounds = await field.boundingBox();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
        expect(fixture.errors).toEqual([]);
        await page.screenshot({
          path: info.outputPath('recovered.png'),
          fullPage: true,
          animations: 'disabled',
        });
      } finally {
        await fixture.close(page);
      }
    });
  }
}
