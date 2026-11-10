import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { binding, LocalAgentV2 } from './agent-v2.fixture';

for (const fail of [false, true]) {
  test(`LOCAL stream hints preserve queue ${fail ? 'failure backoff' : 'polling bounds'}`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    let busy = fail;
    let reads = 0;
    const failures: string[] = [];
    page.on('requestfailed', (request) =>
      failures.push(`${new URL(request.url()).pathname}: ${request.failure()?.errorText}`),
    );
    page.on('console', (message) => {
      if (message.type() === 'error') failures.push(message.text());
    });
    let stream: ReturnType<typeof setInterval> | undefined;
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) => {
        reads += 1;
        return route.fulfill(
          busy
            ? {
                status: 503,
                json: {
                  error: {
                    type: 'server_error',
                    code: 'unavailable',
                    message: 'Temporarily unavailable',
                    param: null,
                  },
                },
              }
            : {
                json: {
                  object: 'cma.agent.queued_inputs',
                  agent_id: 'agt_contract',
                  data: [],
                  next_after: null,
                },
              },
        );
      });
      await page.goto('/agents/agt_contract');
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
      await expect.poll(() => fixture.streams.size).toBe(1);
      await expect.poll(() => reads).toBeGreaterThan(0);
      let tokens = 0;
      stream = setInterval(
        () =>
          fixture.append({
            type: 'usage',
            input_tokens: 1,
            output_tokens: ++tokens,
            context_used: tokens + 1,
            context_window: 100000,
          }),
        200,
      );
      await page.waitForTimeout(7000);
      expect(reads).toBeGreaterThanOrEqual(3);
      expect(reads).toBeLessThanOrEqual(fail ? 3 : 5);
      if (fail) {
        await expect(
          page.getByText('The message queue could not be refreshed. Your messages are retained.'),
        ).toBeVisible();
        busy = false;
        await expect(
          page.getByText('The message queue could not be refreshed. Your messages are retained.'),
        ).toHaveCount(0, { timeout: 10000 });
      }
      await info.attach('queue-observations', {
        body: JSON.stringify({ initialFailure: fail, reads, events: tokens }),
        contentType: 'application/json',
      });
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
    } finally {
      const path = info.outputPath('boundary.json');
      await writeFile(
        path,
        JSON.stringify({ failures, reads: fixture.reads, errors: fixture.errors }),
      );
      await info.attach('boundary-failures', { path, contentType: 'application/json' });
      clearInterval(stream);
      await fixture.close(page);
    }
  });
}
