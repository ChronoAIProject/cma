import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { commandStatus } from './agent-v2-command.fixture';

for (const theme of ['light', 'dark'] as const) {
  test(`LOCAL ${theme}: deferred send refused before dispatch retains the draft without a phantom pending message`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.sandbox = {
      ...fixture.boundary.sandbox,
      status: 'paused',
      lifecycle: { pause: { allowed: false, reason: 'not_active' }, revive: { allowed: true } },
    };
    const paused = structuredClone(fixture.boundary.sandbox);
    const writes: { key: string; body: unknown }[] = [];
    let holdCheck = false;
    let checking = false;
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/auth/connections/check', async (route) => {
        if (holdCheck) {
          checking = true;
          await pending;
        }
        await route.fallback();
      });
      await page.route('**/api/v2/agents/agt_contract/responses', async (route) => {
        const request = route.request();
        writes.push({ key: request.headers()['idempotency-key']!, body: request.postDataJSON() });
        if (writes.length === 1) return route.fallback();
        await route.fulfill({
          status: 200,
          headers: {
            'content-type': 'text/event-stream',
            'x-cma-response-id': 'resp_2',
            'x-cma-command-id': 'cmd_2',
          },
          body: ': accepted\n\n',
        });
      });
      await page.route('**/api/v2/commands/cmd_2', (route) => {
        const original = commandStatus(fixture.started);
        return route.fulfill({
          json: {
            ...original,
            receipt: {
              ...original.receipt,
              command_id: 'cmd_2',
              response_id: 'resp_2',
              agent_order: 2,
            },
          },
        });
      });
      await page.goto('/agents/agt_contract');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const send = page.getByRole('button', { name: 'Send message', exact: true });
      await field.fill('Earlier accepted message');
      await send.click();
      await expect(field).toHaveValue('');
      await expect.poll(() => writes.length).toBe(1);
      await expect(page.getByText('Sent', { exact: true })).toBeVisible();
      const text = 'Send after checking connections';
      const outgoing = page.locator('.message--user').filter({ hasText: text });
      await field.fill(text);
      await page.evaluate(() => window.dispatchEvent(new Event('blur')));
      holdCheck = true;
      await send.click();
      await expect.poll(() => checking).toBe(true);
      await expect(outgoing).toContainText('Sending…');
      expect(writes).toHaveLength(1);

      fixture.boundary.sandbox = { ...paused, status: 'failed' };
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      await expect(
        page.getByText('This Sandbox is unavailable. Messages cannot be sent.', { exact: true }),
      ).toBeVisible();
      release();
      holdCheck = false;
      await expect(outgoing).toContainText('Not sent');
      await expect(page.getByText('Awaiting confirmation', { exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Recover pending request' })).toHaveCount(0);
      await expect(field).toHaveValue(text);
      expect(writes).toHaveLength(1);
      expect(fixture.receiptReads).toHaveLength(0);
      await page.screenshot({
        path: info.outputPath(`deferred-not-sent-${theme}.png`),
        animations: 'disabled',
      });

      fixture.boundary.sandbox = paused;
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      await expect(send).toBeEnabled();
      expect(writes).toHaveLength(1);
      await send.click();
      await expect.poll(() => writes.length).toBe(2);
      await expect(field).toHaveValue('');
      await expect(outgoing).toHaveCount(1);
      await expect(outgoing).toContainText('Sent');
      expect(writes[1]!.key).not.toBe(writes[0]!.key);
      expect(writes[1]!.body).toMatchObject({ input: text });
      fixture.append(
        { type: 'turn_started', turn_sequence: 1 },
        { response_id: 'resp_2', command_id: 'cmd_2', turn_sequence: 1 },
      );
      fixture.append({ type: 'message', role: 'user', content: text });
      fixture.append({ type: 'turn_completed', turn_sequence: 1 });
      await expect(outgoing).toHaveCount(1);
      await expect(outgoing.getByText('Sent', { exact: true })).toHaveCount(0);
      expect(writes).toHaveLength(2);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      release();
      await fixture.close(page);
    }
  });
}
