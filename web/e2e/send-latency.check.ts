import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { LocalAgentV2, binding } from './agent-v2.fixture';

const input = 'Local Send scheduling measurement';
for (const phase of ['dispatch', 'receipt', 'unblocked'] as const) {
  const samples = phase === 'dispatch' ? 20 : 5;
  for (let sample = 1; sample <= samples; sample += 1) {
    test(`LOCAL Send ${phase} sample ${String(sample)}`, async ({ page, baseURL }, info) => {
      const fixture = new LocalAgentV2();
      fixture.holdInput = phase === 'receipt';
      const baseline = process.env.CMA_SEND_BASELINE === '1';
      let release!: () => void;
      let reached!: () => void;
      const held = new Promise<void>((resolve) => {
        release = resolve;
      });
      const ready = new Promise<void>((resolve) => {
        reached = resolve;
      });
      let heldReads = 0;
      let intercept = false;
      let released = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const marks: Record<string, number> = {};
      const releaseBackground = () => {
        released = true;
        marks.backgroundReleased = performance.now();
        release();
      };
      try {
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
        const composer = page.getByRole('textbox', { name: 'Message', exact: true });
        const send = page.getByRole('button', { name: 'Send message', exact: true });
        await composer.fill(input);
        await expect(send).toBeEnabled();
        await page.route('**/auth/session', async (route) => {
          if (intercept) {
            heldReads += 1;
            if (heldReads >= 2) reached();
            await held;
          }
          await route.fallback();
        });
        const holdBackground = async () => {
          // Application focus refresh and transport observation are independent.
          // Hold both so the transport gate is definitely occupied.
          intercept = true;
          await page.evaluate(() => window.dispatchEvent(new Event('focus')));
          await ready;
          intercept = false;
        };
        if (phase === 'dispatch') await holdBackground();
        await page.evaluate((text) => {
          const marks: Record<string, number> = {};
          Object.assign(window, { sendTiming: marks });
          document.addEventListener(
            'click',
            (event) => {
              if ((event.target as Element).closest('[aria-label="Send message"]'))
                marks.click = performance.now();
            },
            { capture: true },
          );
          const observer = new MutationObserver(() => {
            const row = [...document.querySelectorAll('.message--user')].find((node) =>
              node.textContent?.includes(text),
            );
            if (!row || row.getBoundingClientRect().height === 0) return;
            marks.echo ??= performance.now();
            if (row.querySelector('[role="status"]')?.textContent === 'Sent')
              marks.accepted ??= performance.now();
            const composer = document.querySelector<HTMLTextAreaElement>('[aria-label="Message"]');
            if (composer?.value === '') marks.composerClear ??= performance.now();
          });
          observer.observe(document.body, { childList: true, subtree: true, characterData: true });
        }, input);
        page.on('request', (request) => {
          if (
            request.method() === 'POST' &&
            new URL(request.url()).pathname === '/api/v2/agents/agt_contract/responses'
          )
            marks.request ??= performance.now();
        });
        const receipt = page.waitForResponse(
          (response) =>
            response.request().method() === 'POST' &&
            new URL(response.url()).pathname === '/api/v2/agents/agt_contract/responses',
        );
        marks.clickInvoked = performance.now();
        if (phase === 'dispatch' && baseline) timer = setTimeout(releaseBackground, 1000);
        await send.click();
        marks.clickReturned = performance.now();
        const echo = page.locator('.message--user').filter({ hasText: input });
        await expect(echo).toHaveCount(1);
        await expect(echo).toBeVisible();
        marks.echoObserved = performance.now();
        if (phase === 'receipt') {
          await expect.poll(() => fixture.writes.length).toBe(1);
          await holdBackground();
          await expect(echo.getByRole('status')).toHaveText('Sending…');
          if (baseline) timer = setTimeout(releaseBackground, 1000);
          marks.receiptReleased = performance.now();
          fixture.acceptInput();
        }
        const response = await receipt;
        marks.receiptHeaders = performance.now();
        expect(response.status()).toBe(200);
        await expect(echo.getByRole('status')).toHaveText('Sent');
        marks.acceptedObserved = performance.now();
        await expect(composer).toHaveValue('');
        expect(released).toBe(baseline && phase !== 'unblocked');
        expect(fixture.writes).toHaveLength(1);
        expect(fixture.writes[0]!.body.input).toBe(input);
        await expect(page.getByRole('button', { name: 'Stop response' })).toHaveCount(0);
        const dom = await page.evaluate(
          () => Reflect.get(window, 'sendTiming') as Record<string, number>,
        );
        expect(dom.echo).toBeGreaterThanOrEqual(dom.click!);
        expect(dom.accepted).toBeGreaterThanOrEqual(dom.echo!);
        releaseBackground();
        fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
        fixture.append({ type: 'message', role: 'user', content: input });
        await expect(echo.getByRole('status')).toHaveCount(0);
        await expect(echo).toHaveCount(1);
        expect(fixture.errors).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        await writeFile(
          info.outputPath('timings.json'),
          JSON.stringify(
            {
              source: process.env.CMA_SEND_SOURCE,
              environment: 'compiled Chrome loopback; synthetic background holds; not production',
              phase,
              sample,
              baseline,
              heldReads,
              marks,
              dom,
              clickToRequestMs: marks.request! - marks.clickInvoked!,
              clickToEchoObservedMs: marks.echoObserved! - marks.clickInvoked!,
              clickToAcceptedObservedMs: marks.acceptedObserved! - marks.clickInvoked!,
              domClickToEchoMs: dom.echo! - dom.click!,
              domClickToAcceptedMs: dom.accepted! - dom.click!,
              receiptReleaseToAcceptedMs:
                phase === 'receipt' ? marks.acceptedObserved! - marks.receiptReleased! : undefined,
              posts: fixture.writes.length,
            },
            null,
            2,
          ),
        );
      } finally {
        clearTimeout(timer);
        release();
        await fixture.close(page);
      }
    });
  }
}
