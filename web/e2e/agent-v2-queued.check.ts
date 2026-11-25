import { test, expect } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';
import { commandStatus } from './agent-v2-command.fixture';

for (const outcome of ['delivered', 'cancelled'] as const) {
  test(`LOCAL: server resolves a queued ${outcome} message after switching input mode`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.queued.conflict = true;
    let confirmed = false;
    const input = 'Check whether the build is still running';
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      { response_id: 'resp_active', command_id: 'cmd_active', turn_sequence: 1 },
    );
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/queued-inputs*', (route) => {
        if (!confirmed) return route.fallback();
        if (outcome === 'cancelled')
          return route.fulfill({
            status: 503,
            json: {
              error: {
                type: 'server_error',
                code: 'unavailable',
                message: 'Queue unavailable',
                param: null,
              },
            },
          });
        return route.fulfill({
          json: {
            object: 'cma.agent.queued_inputs',
            agent_id: 'agt_contract',
            data: [],
            next_after: null,
          },
        });
      });
      await page.route('**/api/v2/commands/cmd_1', (route) =>
        confirmed && outcome === 'cancelled'
          ? route.fulfill({
              json: {
                ...commandStatus(fixture.started),
                status: 'cancelled',
                turn_sequence: null,
                observed_cursor: null,
                delivery: null,
              },
            })
          : route.fallback(),
      );
      await page.goto('/agents/agt_contract');
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await composer.fill(input);
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      const row = page.getByRole('article', { name: 'Queued message 1', exact: true });
      await expect(row.getByText(input)).toBeVisible();

      await page.getByRole('button', { name: 'Steering mode: Queue', exact: true }).click();
      await page.getByRole('menuitemradio', { name: /Interrupt/ }).click();
      await expect(page.getByRole('button', { name: 'Steering mode: Interrupt' })).toBeVisible();
      await expect(row.getByText(input)).toBeVisible();
      await row.getByRole('button', { name: 'Remove queued message 1' }).click();
      const recover = page.getByRole('button', { name: 'Recover request', exact: true });
      await expect(recover).toBeEnabled();
      await composer.fill('Keep the draft');

      confirmed = true;
      if (outcome === 'delivered') {
        fixture.append({ type: 'turn_completed', turn_sequence: 1 });
        const turnSequence = fixture.items.length + 1;
        fixture.append(
          { type: 'turn_started', turn_sequence: turnSequence },
          { ...binding, turn_sequence: turnSequence },
        );
        fixture.append({ type: 'message', role: 'user', content: input });
        fixture.append({ type: 'message', role: 'assistant', content: 'The build finished.' });
        await expect(page.getByText('The build finished.', { exact: true })).toBeVisible();
      }
      // A read before confirmation can defer the next command read by ten seconds.
      await expect(recover).toHaveCount(0, { timeout: outcome === 'cancelled' ? 15_000 : 5_000 });
      await expect(row).toHaveCount(0);
      await expect(composer).toHaveValue('Keep the draft');
      await expect(page.getByRole('button', { name: 'Steering mode: Interrupt' })).toBeVisible();
      if (outcome === 'delivered') {
        await expect(page.getByText(input, { exact: true })).toHaveCount(1);
      } else {
        const cancelled = page.getByText('Cancelled messages (1)', { exact: true });
        await expect(cancelled).toBeVisible();
        const visibleInput = page.getByText(input, { exact: true }).filter({ visible: true });
        await expect(visibleInput).toHaveCount(0);
        await cancelled.click();
        await expect(page.getByText('Message was cancelled.', { exact: true })).toBeVisible();
        await expect(visibleInput).toHaveCount(1);
        await cancelled.click();
        await expect(visibleInput).toHaveCount(0);
      }
      expect(fixture.writes.filter(({ path }) => path.endsWith('/responses'))).toHaveLength(1);
      expect(
        fixture.writes.filter(({ path }) => path.endsWith('/queued-cancellation')),
      ).toHaveLength(1);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

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

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: cancel a queued message and recover its exact lost receipt`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.queued.loseReceipt = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      { response_id: 'resp_active', command_id: 'cmd_active', turn_sequence: 1 },
    );
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(field).toBeVisible();
      await field.fill('A waiting message with a separate cancellation');
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      const cancel = page.getByRole('button', { name: 'Remove queued message 1', exact: true });
      await expect(cancel).toBeEnabled();
      await field.fill('Keep this editable draft');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(cancel).toBeVisible();
        await expect
          .poll(() =>
            cancel.evaluate((element) => {
              const box = element.getBoundingClientRect();
              return (
                box.left >= 0 &&
                box.right <= innerWidth &&
                box.top >= 0 &&
                box.bottom <= innerHeight
              );
            }),
          )
          .toBe(true);
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({
          path: info.outputPath(`queued-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await cancel.focus();
      await page.keyboard.press('Enter');
      const recover = page.getByRole('button', { name: 'Recover request', exact: true });
      await expect(recover).toBeEnabled();
      await expect(field).toHaveValue('Keep this editable draft');
      await expect(
        page.getByRole('status').filter({ hasText: /^Waiting for a response$/ }),
      ).toBeVisible();
      fixture.queued.conflict = true;
      await page.reload();
      await expect(recover).toBeEnabled();
      await recover.click();
      await expect(
        page.getByRole('alert').filter({ hasText: 'Cancellation observation changed' }),
      ).toBeVisible();
      await expect(recover).toBeEnabled();
      fixture.queued.conflict = false;
      await recover.click();
      await expect(
        page.getByRole('article', { name: 'Queued message 1', exact: true }),
      ).toHaveCount(0);
      await expect(recover).toHaveCount(0);
      await expect(
        page.getByRole('status').filter({ hasText: /^Waiting for a response$/ }),
      ).toBeVisible();
      const writes = fixture.writes.filter((write) => write.path.endsWith('/queued-cancellation'));
      expect(writes).toHaveLength(3);
      expect(writes[1]).toEqual(writes[0]);
      expect(writes[2]).toEqual(writes[0]);
      expect(writes[0]!.body).toEqual({ source_command_id: 'cmd_1', expected_command_revision: 7 });
      expect(fixture.writes.filter((write) => write.path.endsWith('/controls'))).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: an absent admitted message explains expiry and restores only to a draft`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      { response_id: 'resp_active', command_id: 'cmd_active', turn_sequence: 1 },
    );
    let expired = false;
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      await page
        .getByRole('textbox', { name: 'Message', exact: true })
        .fill('Retain this admitted instruction');
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      await expect(
        page.getByRole('button', { name: 'Remove queued message 1', exact: true }),
      ).toBeVisible();
      await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.queued_inputs',
            agent_id: 'agt_contract',
            data: [],
            next_after: null,
          },
        }),
      );
      await page.route('**/api/v2/commands/cmd_1', (route) =>
        route.fulfill(
          expired
            ? {
                json: {
                  ...commandStatus(fixture.started),
                  status: 'failed',
                  failure: 'queue_timeout',
                  turn_sequence: null,
                  observed_cursor: null,
                  delivery: null,
                },
              }
            : {
                status: 404,
                json: {
                  error: {
                    type: 'invalid_request_error',
                    code: 'not_found',
                    message: 'Unavailable',
                    param: null,
                  },
                },
              },
        ),
      );
      const outcomes = page.getByRole('region', { name: 'Message status', exact: true });
      await expect(
        outcomes.getByText('Message status could not be confirmed. Refresh status.'),
      ).toBeVisible();
      await expect(outcomes.getByText('Retain this admitted instruction')).toBeVisible();
      await expect(
        outcomes.getByRole('button', { name: 'Restore to draft', exact: true }),
      ).toHaveCount(0);
      await expect(outcomes.getByRole('button', { name: 'Copy message', exact: true })).toHaveCount(
        0,
      );
      expect(fixture.writes).toHaveLength(1);
      expired = true;
      await outcomes.getByRole('button', { name: 'Refresh status', exact: true }).click();
      await expect(outcomes.getByText('Expired before starting')).toBeVisible({ timeout: 10000 });
      await expect(
        page.getByRole('article', { name: 'Queued message 1', exact: true }),
      ).toHaveCount(0);
      await expect(page.getByText('Retain this admitted instruction', { exact: true })).toHaveCount(
        1,
      );
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(outcomes).toBeInViewport();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({
          path: info.outputPath(`outcome-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      expect(fixture.writes.filter((write) => write.path.endsWith('/responses'))).toHaveLength(1);
      expect(fixture.writes).toHaveLength(1);
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const restore = outcomes.getByRole('button', { name: 'Restore to draft', exact: true });
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      await outcomes.getByRole('button', { name: 'Copy message', exact: true }).click();
      await expect
        .poll(() => page.evaluate(() => navigator.clipboard.readText()))
        .toBe('Retain this admitted instruction');
      await expect(field).toHaveValue('');
      await restore.focus();
      await page.keyboard.press('Enter');
      await expect(field).toHaveValue('Retain this admitted instruction');
      await expect(field).toBeFocused();
      expect(fixture.writes).toHaveLength(1);
      await page.reload();
      await expect(field).toHaveValue('Retain this admitted instruction');
      await expect(restore).toBeEnabled();
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      await field.fill('Keep the current draft');
      await restore.click();
      const dialog = page.getByRole('dialog', { name: 'Keep your draft?', exact: true });
      await expect(dialog.getByRole('button', { name: 'Keep editing', exact: true })).toBeFocused();
      await page.screenshot({
        path: info.outputPath(`restore-confirmation-${theme}-320.png`),
        animations: 'disabled',
      });
      await page.keyboard.press('Enter');
      await expect(dialog).toHaveCount(0);
      await expect(field).toHaveValue('Keep the current draft');
      await restore.click();
      await dialog.getByRole('button', { name: 'Add to current draft', exact: true }).click();
      await expect(dialog).toHaveCount(0);
      await expect(field).toBeFocused();
      await expect(field).toHaveValue('Keep the current draft\nRetain this admitted instruction');
      expect(fixture.writes).toHaveLength(1);
      await page.reload();
      await expect(field).toHaveValue('Keep the current draft\nRetain this admitted instruction');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      expect(fixture.writes).toHaveLength(1);
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      await expect.poll(() => fixture.writes.length).toBe(2);
      expect(fixture.writes[1]!.body.input).toBe(
        'Keep the current draft\nRetain this admitted instruction',
      );
      expect(fixture.writes[1]!.key).toBeTruthy();
      expect(fixture.writes[1]!.key).not.toBe(fixture.writes[0]!.key);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const delivery of ['runtime_lost', 'recovery_required'] as const) {
  test(`LOCAL: ${delivery} retains the original request without draft restoration or automatic send`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      { response_id: 'resp_active', command_id: 'cmd_active', turn_sequence: 1 },
    );
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      await field.fill('Retain the uncertain original');
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      await expect(
        page.getByRole('button', { name: 'Remove queued message 1', exact: true }),
      ).toBeVisible();
      await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.queued_inputs',
            agent_id: 'agt_contract',
            data: [],
            next_after: null,
          },
        }),
      );
      await page.route('**/api/v2/commands/cmd_1', (route) =>
        route.fulfill({
          json: {
            ...commandStatus(fixture.started),
            status: 'failed',
            failure: 'runtime_lost',
            delivery,
            turn_sequence: 1,
            observed_cursor: null,
          },
        }),
      );
      const outcomes = page.getByRole('region', { name: 'Message status', exact: true });
      await expect(
        outcomes.getByText(
          'Message execution could not be confirmed. Review the conversation before trying again.',
        ),
      ).toBeVisible();
      await expect(outcomes.getByText('Retain the uncertain original')).toBeVisible();
      await expect(
        outcomes.getByRole('button', { name: 'Restore to draft', exact: true }),
      ).toHaveCount(0);
      await expect(outcomes.getByRole('button', { name: 'Copy message', exact: true })).toHaveCount(
        0,
      );
      await expect(field).toHaveValue('');
      await page.reload();
      await expect(outcomes.getByText('Retain the uncertain original')).toBeVisible();
      await expect(
        outcomes.getByRole('button', { name: 'Restore to draft', exact: true }),
      ).toHaveCount(0);
      await expect(outcomes.getByRole('button', { name: 'Copy message', exact: true })).toHaveCount(
        0,
      );
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
