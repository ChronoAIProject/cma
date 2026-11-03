import { readFileSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { createServer, type ServerResponse } from 'node:http';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalAgentV2 } from './agent-v2.fixture';

const proofPath = process.env.CMA_COMPLETION_PUBLIC_PROOF;
if (!proofPath) throw new Error('Set CMA_COMPLETION_PUBLIC_PROOF to the public Rust route output.');
interface Evidence {
  active: string;
  agui: string;
  tail: string;
  responses: {
    agent_id: string;
    incarnation: string;
    through_cursor: string;
    items: { sequence: number; cursor: string }[];
  };
}
const proof = JSON.parse(readFileSync(proofPath, 'utf8')) as Record<
  'completed' | 'interrupted',
  Evidence
>;
const directory =
  process.env.CMA_COMPLETION_EVIDENCE_DIR ?? '/tmp/cma-ticket-02-evidence/browser-after';
const baseline = process.env.CMA_COMPLETION_BASELINE === '1';

for (const transport of ['agui', 'responses'] as const) {
  for (const width of [1440, 390]) {
    test(`LOCAL ${transport} confirmed duration and persistent composer at ${width}px`, async ({
      page,
      baseURL,
    }) => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
      await page.emulateMedia({
        reducedMotion: 'reduce',
        colorScheme: width === 390 ? 'dark' : 'light',
      });
      await page.addInitScript(
        (appearance) => localStorage.setItem('cma:appearance', appearance),
        width === 390 ? 'dark' : 'light',
      );
      const boundary = new LocalStartup();
      const v2 = transport === 'responses' ? new LocalAgentV2() : undefined;
      let stream: ServerResponse | undefined;
      let terminal = false;
      let outcome: keyof typeof proof = 'completed';
      let connections = 0;
      const activeSnapshot = () => {
        const full = proof[outcome].responses;
        return terminal
          ? full
          : {
              ...full,
              items: full.items.filter((item) => item.sequence <= 4),
              through_cursor: full.items.find((item) => item.sequence === 4)!.cursor,
            };
      };
      const server = createServer((request, response) => {
        response.writeHead(200, {
          'Content-Type': 'text/event-stream',
          'Access-Control-Allow-Origin': baseURL!,
          'Access-Control-Allow-Credentials': 'true',
          'Access-Control-Allow-Headers': 'content-type, x-request-id, last-event-id',
          'Access-Control-Allow-Methods': 'GET, OPTIONS',
        });
        if (request.method === 'OPTIONS') return response.end();
        stream = response;
        connections += 1;
        if (transport === 'agui')
          response.write(terminal ? proof[outcome].agui : proof[outcome].active);
        else response.write(': connected\n\n');
      });
      await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
      const address = server.address();
      if (!address || typeof address === 'string') throw new Error('Missing loopback stream port');
      const streamURL = `http://127.0.0.1:${address.port}/events`;
      try {
        if (v2) await v2.route(page, baseURL!);
        else await boundary.route(page, baseURL!);
        await page.route(`${streamURL}*`, (route) => route.continue());
        await page.route(
          transport === 'agui'
            ? '**/api/v1/agui/events*'
            : '**/api/v2/agents/agt_contract/events?*',
          (route) =>
            route.continue({ url: `${streamURL}${new URL(route.request().url()).search}` }),
        );
        if (v2) {
          await page.route('**/api/v2/agents/agt_contract/events/snapshot*', (route) =>
            route.fulfill({ json: activeSnapshot() }),
          );
          await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
            route.fulfill({
              status: 503,
              json: {
                error: {
                  type: 'server_error',
                  code: 'agent_read_unavailable',
                  message: 'Head temporarily unavailable',
                  param: null,
                },
              },
            }),
          );
        }
        const model = 'GPT-6 with a deliberately long catalog label for narrow screens';
        await page.route('**/api/v1/agents/agt_contract/models', (route) =>
          route.fulfill({ json: { models: [{ id: 'gpt-6', label: model, available: true }] } }),
        );
        const attachmentId = `att_${'a'.repeat(64)}`;
        await page.route('**/api/v1/agents/agt_contract/attachments?*', (route) =>
          route.fulfill({
            status: 201,
            json: {
              id: attachmentId,
              agent_id: 'agt_contract',
              name: 'notes.txt',
              size_bytes: 5,
              media_type: 'text/plain',
              sha256: 'b'.repeat(64),
              path: `/home/attachments/agt_contract/${attachmentId}/notes.txt`,
            },
          }),
        );
        await page.goto('/agents/agt_contract');
        await expect(page.locator('html')).toHaveCSS(
          'color-scheme',
          width === 390 ? 'dark' : 'light',
        );
        const field = page.getByRole('textbox', { name: 'Message', exact: true });
        await expect(field).toBeEnabled();
        await expect.poll(() => connections).toBeGreaterThan(0);
        await field.fill('Keep my draft and attachment while settings and header change.');
        await page.locator('input[type="file"]').setInputFiles({
          name: 'notes.txt',
          mimeType: 'text/plain',
          buffer: Buffer.from('notes'),
        });
        await expect(page.getByText('Ready', { exact: true })).toBeVisible();
        await mkdir(directory, { recursive: true });
        await page.screenshot({
          path: `${directory}/${transport}-initial-${width}.png`,
          fullPage: true,
        });
        if (baseline) return;
        const summary = page.getByRole('button', { name: /^Agent settings:/ });
        await expect(summary).toHaveAttribute('aria-expanded', 'false');
        const original = await field.elementHandle();
        await summary.focus();
        await page.keyboard.press('Enter');
        await expect(page.getByRole('region', { name: 'Agent settings' })).toBeVisible();
        await expect(
          page.getByRole('button', { name: 'Permission mode', exact: true }),
        ).toBeVisible();
        await page.screenshot({
          path: `${directory}/${transport}-settings-${width}.png`,
          fullPage: true,
        });
        await summary.press('Space');
        await expect(summary).toHaveAttribute('aria-expanded', 'false');
        expect(await field.evaluate((element, previous) => element === previous, original)).toBe(
          true,
        );
        await expect(page.getByText('notes.txt', { exact: true })).toBeVisible();
        await expect(page.getByText(/Completed in/)).toHaveCount(0);
        terminal = true;
        if (transport === 'agui') stream!.write(proof.completed.tail);
        else
          for (const item of proof.completed.responses.items.filter((entry) => entry.sequence > 4))
            stream!.write(
              `event: cma.agent.event\nid: ${item.cursor}\ndata: ${JSON.stringify({ ...item, agent_id: 'agt_contract', incarnation: proof.completed.responses.incarnation })}\n\n`,
            );
        await expect(page.locator('header').getByRole('status')).toHaveText(
          'Completed in 4 minutes',
        );
        expect(await field.evaluate((element, previous) => element === previous, original)).toBe(
          true,
        );
        await expect(field).toHaveValue(
          'Keep my draft and attachment while settings and header change.',
        );
        await expect(page.getByText('notes.txt', { exact: true })).toBeVisible();
        await field.fill('Long draft line\n'.repeat(70));
        await summary.click();
        await expect(
          page.getByRole('button', { name: 'Send message', exact: true }),
        ).toBeInViewport();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        if (width === 390) {
          expect((await summary.boundingBox())!.height).toBeGreaterThanOrEqual(44);
          expect(
            (await page
              .getByRole('button', { name: 'Permission mode', exact: true })
              .boundingBox())!.height,
          ).toBeGreaterThanOrEqual(44);
        }
        await page.screenshot({
          path: `${directory}/${transport}-completed-${width}.png`,
          fullPage: true,
        });
        await page.reload();
        await expect(page.locator('header').getByRole('status')).toHaveText(
          'Completed in 4 minutes',
        );
        outcome = 'interrupted';
        await page.reload();
        await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible();
        await expect(page.getByText(/Completed in/)).toHaveCount(0);
        expect((v2?.boundary ?? boundary).errors).toEqual([]);
        expect((v2?.boundary ?? boundary).unexpected).toEqual([]);
      } finally {
        if (v2) await v2.close(page);
        server.closeAllConnections();
        await new Promise<void>((resolve) => server.close(() => resolve()));
      }
    });
  }
}
