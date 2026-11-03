import { expect, test } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

for (const width of [1440, 390, 320]) {
  for (const theme of ['light', 'dark']) {
    test(`comparison suggestion ${width}px ${theme}`, async ({ page, baseURL }, info) => {
      const fixture = new LocalAgentV2();
      page.on('pageerror', (error) => fixture.errors.push(String(error)));
      const text = 'Add explicit comparison criteria and explain the trade-offs.';
      fixture.append({ type: 'turn_started', turn_sequence: 1 });
      fixture.append({ type: 'message', role: 'user', content: 'Compare PostgreSQL and SQLite.' });
      fixture.append({
        type: 'message',
        role: 'assistant',
        content: 'PostgreSQL supports concurrent writers; SQLite suits local storage.',
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
              through_cursor: `ag1.${'a'.repeat(64)}.${fixture.items.length}`,
              active_turn: null,
            },
          }),
        );
        await page.setViewportSize({ width, height: 900 });
        await page.goto('/agents/agt_contract');
        const field = page.getByRole('textbox', { name: 'Message', exact: true });
        await expect(field).toBeEnabled();
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        await field.fill('Keep the budget limit.');
        if (process.env.CMA_SUGGESTION_BASELINE === '1') {
          await page.screenshot({ path: info.outputPath('before.png'), fullPage: true });
          await expect(page.getByRole('region', { name: 'Next you can ask' })).toHaveCount(0);
          return;
        }
        const region = page.getByRole('region', { name: 'Next you can ask' });
        const suggestion = region.getByRole('button', { name: text, exact: true });
        await expect(region).toHaveCount(1);
        await expect(suggestion).toBeEnabled();
        await suggestion.scrollIntoViewIfNeeded();
        expect(
          await suggestion.evaluate((button) => {
            const bounds = button.getBoundingClientRect();
            return {
              inside: bounds.left >= 0 && bounds.right <= innerWidth,
              height: bounds.height,
              clipped: button.scrollWidth > button.clientWidth,
              text: button.textContent,
            };
          }),
        ).toMatchObject({ inside: true, clipped: false, text });
        expect((await suggestion.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        await page.screenshot({ path: info.outputPath('after.png'), fullPage: true });
        let uncertain = true;
        await page.route('**/api/v2/agents/agt_contract/responses', async (route) => {
          if (!uncertain) return route.fallback();
          uncertain = false;
          fixture.writes.push({
            path: '/api/v2/agents/agt_contract/responses',
            body: route.request().postDataJSON(),
            key: route.request().headers()['idempotency-key'],
          });
          await route.fulfill({
            status: 503,
            json: {
              error: {
                type: 'server_error',
                code: 'upstream_response_lost',
                message: 'The upstream receipt could not be observed.',
                param: null,
              },
            },
          });
        });
        if (width === 1440) {
          await suggestion.focus();
          await expect(suggestion).toBeFocused();
          expect(
            await suggestion.evaluate((button) => getComputedStyle(button).boxShadow),
          ).not.toBe('none');
          await page.keyboard.press('Enter');
        } else {
          await suggestion.tap();
        }
        await expect(page.getByText('Awaiting confirmation', { exact: true })).toBeVisible();
        await expect(suggestion).toBeDisabled();
        await expect(field).toHaveValue('Keep the budget limit.');
        await page.screenshot({ path: info.outputPath('recovery.png'), fullPage: true });
        await page.getByRole('button', { name: 'Recover pending request' }).click();
        await expect(region).toHaveCount(0);
        expect(fixture.writes).toHaveLength(2);
        expect(fixture.writes[1]).toEqual(fixture.writes[0]);
        expect(fixture.writes[0]!.body).toEqual({
          input: text,
          stream: true,
          answer_context: { turn_sequence: 1, answer_sequence: 3 },
        });
        expect(fixture.errors).toEqual([]);
      } catch (error) {
        console.log(
          JSON.stringify({
            errors: fixture.errors,
            unexpected: fixture.boundary.unexpected,
            boundary: fixture.boundary.errors,
            content: await page.locator('body').innerText(),
          }),
        );
        throw error;
      } finally {
        await fixture.close(page);
      }
    });
  }
}
