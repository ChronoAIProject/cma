import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { commandStatus } from './agent-v2-command.fixture';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: preserve a conflicting queued edit through reload and original-key recovery`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      { response_id: 'resp_active', command_id: 'cmd_active', turn_sequence: 1 },
    );
    const edits: { key: string | undefined; body: unknown }[] = [];
    const text = 'Revised instruction\n保留原文';
    let conflict = true;
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/responses/resp_1/queued-input', async (route) => {
        const body = route.request().postDataJSON() as {
          text: string;
          source_command_id: string;
          expected_command_revision: number;
        };
        edits.push({ key: route.request().headers()['idempotency-key'], body });
        if (conflict)
          return route.fulfill({
            status: 409,
            json: {
              error: {
                type: 'invalid_request_error',
                code: 'queued_input_conflict',
                message: 'The source command or current authority changed.',
                param: null,
              },
            },
          });
        fixture.queued.admit(body.text);
        fixture.queued.revision = body.expected_command_revision + 1;
        await route.fulfill({
          json: {
            object: 'cma.response.queued_input',
            edit: {
              ...commandStatus(fixture.started, true),
              turn_sequence: null,
              observed_cursor: null,
              delivery: null,
            },
            response: fixture.queued.items()[0]!.response,
            target: {
              response_id: 'resp_1',
              source_command_id: body.source_command_id,
              expected_command_revision: body.expected_command_revision,
            },
            command_revision: fixture.queued.revision,
            text: body.text,
          },
        });
      });
      await page.goto('/agents/agt_contract');
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await composer.fill('Original queued message');
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      await composer.fill('Keep my separate draft');
      await page.getByRole('button', { name: 'Edit queued message content' }).click();
      await page.getByRole('textbox', { name: 'Queued message', exact: true }).fill(text);
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      const saved = page.getByRole('textbox', { name: 'Edit awaiting confirmation' });
      await expect(saved).toHaveValue(text);
      await page.reload();
      await expect(saved).toHaveValue(text);
      await expect(saved).toHaveAttribute('readonly', '');
      await expect(composer).toHaveValue('Keep my separate draft');
      await expect(page.getByRole('button', { name: 'Remove queued message 1' })).toBeDisabled();
      expect(edits).toHaveLength(1);
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      await page.getByRole('button', { name: 'Copy edit', exact: true }).click();
      await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(text);
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await saved.scrollIntoViewIfNeeded();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({
          path: info.outputPath(`pending-edit-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      expect(edits).toHaveLength(1);
      const recover = page.getByRole('button', { name: 'Recover request', exact: true });
      await recover.click();
      await expect.poll(() => edits.length).toBe(2);
      await expect(saved).toHaveValue(text);
      await expect(recover).toBeEnabled();
      conflict = false;
      await recover.click();
      await expect(saved).toHaveCount(0);
      expect(edits).toHaveLength(3);
      expect(edits[0]!.body).toEqual({
        source_command_id: 'cmd_1',
        expected_command_revision: 7,
        text,
      });
      expect(edits[0]!.key).toBeTruthy();
      expect(edits[1]).toEqual(edits[0]);
      expect(edits[2]).toEqual(edits[0]);
      await expect(composer).toHaveValue('Keep my separate draft');
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
