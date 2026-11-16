import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { binding, LocalAgentV2 } from './agent-v2.fixture';
import { StartupDiagnostics } from './startup-diagnostics';

test('LOCAL queue discovery resumes after an older server returns 404', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  const diagnostics = new StartupDiagnostics(page);
  fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
  let legacy = true;
  let reads = 0;
  try {
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) => {
      reads += 1;
      return route.fulfill(
        legacy
          ? {
              status: 404,
              json: {
                error: {
                  type: 'invalid_request_error',
                  code: 'not_found',
                  message: 'Feature unavailable',
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
    diagnostics.mark('navigation-started');
    await page.goto('/agents/agt_contract');
    diagnostics.mark('textbox-assertion-started');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    diagnostics.mark('textbox-enabled');
    await expect.poll(() => reads).toBeGreaterThan(0);
    legacy = false;
    fixture.append({
      type: 'message',
      role: 'user',
      content: 'Recorded queue discovery hint',
    });
    await expect.poll(() => reads).toBeGreaterThanOrEqual(2);
    expect(reads).toBeLessThanOrEqual(3);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    await info.attach('compatibility-recovery', {
      body: JSON.stringify({ reads, remounted: false }),
      contentType: 'application/json',
    });
  } catch (error) {
    await diagnostics.capture(info, {
      queueReads: reads,
      reads: fixture.reads,
      errors: fixture.errors,
      boundaryErrors: fixture.boundary.errors,
      unexpected: fixture.boundary.unexpected,
    });
    throw error;
  } finally {
    diagnostics.dispose();
    await fixture.close(page);
  }
});

for (const fail of [false, true]) {
  test(`LOCAL stream hints preserve queue ${fail ? 'failure backoff' : 'polling bounds'}`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    const diagnostics = new StartupDiagnostics(page);
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
      diagnostics.mark('navigation-started');
      await page.goto('/agents/agt_contract');
      diagnostics.mark('textbox-assertion-started');
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
      diagnostics.mark('textbox-enabled');
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
      expect(reads).toBeGreaterThanOrEqual(fail ? 3 : 1);
      expect(reads).toBeLessThanOrEqual(fail ? 3 : 2);
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
    } catch (error) {
      await diagnostics.capture(info, {
        queueReads: reads,
        reads: fixture.reads,
        errors: fixture.errors,
        boundaryErrors: fixture.boundary.errors,
        unexpected: fixture.boundary.unexpected,
      });
      throw error;
    } finally {
      diagnostics.dispose();
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
