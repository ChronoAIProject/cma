import assert from 'node:assert/strict';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:http';
import { chromium, expect } from '@playwright/test';
import { chrome } from './browser.mjs';
import {
  now,
  first,
  preview,
  initial,
  ready,
  agent,
  workspace,
} from '../src/public/browserFixtures.mjs';

const base = process.argv[2] ?? 'http://127.0.0.1:4342';
assert(['localhost', '127.0.0.1'].includes(new URL(base).hostname));
const output = await mkdtemp(join(tmpdir(), 'cma-restored-ui-'));
console.log('Evidence:', output);
const browser = await chromium.launch({ executablePath: chrome(), headless: true });
let navigationEntry = () => {};
let agentStream;
const fixture = createServer((request, response) => {
  if (request.url === '/agent-stream') {
    response.writeHead(200, {
      'Content-Type': 'text/event-stream',
      'Access-Control-Allow-Origin': base,
      'Access-Control-Allow-Credentials': 'true',
      'Access-Control-Allow-Methods': 'GET, OPTIONS',
      'Access-Control-Allow-Headers': 'content-type, x-request-id',
    });
    if (request.method === 'OPTIONS') return response.end();
    response.write(': ready\n\n');
    agentStream = response;
  } else if (request.url === '/fixture-readme') {
    response.setHeader('Content-Type', 'text/html');
    response.end('<a href="/api/v1/triggers/public/public-example/open/plc_readme">Try it</a>');
  } else {
    navigationEntry(request.headers);
    response.writeHead(303, { location: `${base}/t/public-example/plc_readme` });
    response.end();
  }
});
await new Promise((resolve) => fixture.listen(0, '127.0.0.1', resolve));
const fixtureBase = `http://127.0.0.1:${fixture.address().port}`;
const evidence = [];
try {
  for (const theme of ['light', 'dark']) {
    for (const viewport of [
      { width: 1440, height: 900 },
      { width: 390, height: 844 },
    ]) {
      const context = await browser.newContext({ viewport, colorScheme: theme });
      const page = await context.newPage();
      let signedIn = false;
      let entry = false;
      navigationEntry = (headers) => {
        entry =
          headers['sec-fetch-user'] === '?1' &&
          headers['sec-fetch-mode'] === 'navigate' &&
          headers['sec-fetch-dest'] === 'document';
        assert(entry, 'README button must carry a real browser navigation gesture');
      };
      let launches = 0;
      let samples = 0;
      let finished = false;
      let activityUnavailable = true;
      const historyCursors = [];
      const errors = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.route('**/*', async (route) => {
        const request = route.request();
        const url = new URL(request.url());
        if (url.origin === fixtureBase) return route.continue();
        const path = url.pathname;
        const reply = (body) => route.fulfill({ json: body });
        if (path === '/auth/session')
          return reply(
            signedIn
              ? {
                  authenticated: true,
                  subject: 'visitor-one',
                  display_name: 'Ada',
                  visitor: true,
                  capabilities: ['launch_triggers'],
                }
              : { authenticated: false },
          );
        if (path === '/auth/signup-options')
          return reply({
            social_providers: ['github'],
            email_signup: false,
            invite_code_required: false,
            register_available: true,
          });
        if (path === '/auth/login') {
          signedIn = true;
          return route.fulfill({
            status: 302,
            headers: { location: url.searchParams.get('return_to') },
            body: '',
          });
        }
        if (path === '/api/v1/triggers/public/public-example/entry-intent') {
          const accepted = entry;
          entry = false;
          return reply({ accepted, ...(accepted ? { placement_id: 'plc_readme' } : {}) });
        }
        if (path === '/api/v1/triggers/public/public-example')
          return reply({ ...preview, always_review: false });
        if (path.endsWith('/public/public-example/launches')) {
          launches++;
          return route.fulfill({ status: 202, json: initial });
        }
        if (path === '/api/v1/launches/lch_example') return reply(finished ? ready : initial);
        if (path === '/api/v1/agui/events')
          return route.continue({ url: `${fixtureBase}/agent-stream` });
        if (path === '/api/v1/agui/launches/lch_example/events')
          return route.fulfill({ contentType: 'text/event-stream', body: ': keep-alive\n\n' });
        if (path === '/api/v1/sandboxes') return reply({ items: [] });
        if (path === '/api/v1/agents/agt_ready')
          return reply({
            ...agent,
            title: 'A long Agent title that must never widen the available mobile center column',
          });
        if (
          path ===
          `/api/v1/sandbox-profiles/${agent.sandbox.profile.id}/revisions/${agent.sandbox.profile.revision}`
        )
          return reply({
            profile_id: agent.sandbox.profile.id,
            revision: agent.sandbox.profile.revision,
            digest: agent.sandbox.profile.digest,
            published_at: now,
            content: {
              cpu_millis: 4000,
              memory_mib: 8192,
              nyxid_services: [],
              agent_provider: 'codex',
              model_access: 'chronoai_managed',
              max_workspaces: 1,
              max_agents_per_workspace: 1,
              workspace_profiles: [],
              software_install_commands: [],
              environment_variables: [],
            },
          });
        if (path === '/api/v1/workspaces/wsp_one') return reply(workspace);
        if (path.endsWith('/agt_ready/usage'))
          return reply({
            agent_id: 'agt_ready',
            through_seq: 7,
            input_tokens: 12450,
            output_tokens: 1600,
            context_used: 14050,
            context_window: 200000,
          });
        if (path.endsWith('/agt_ready/resources')) {
          samples++;
          return reply({
            agent_id: 'agt_ready',
            sandbox_id: 'sbx_one',
            uptime_ms: samples * 5000,
            cpu_usage_usec: samples * 2500000,
            cpu_millis_limit: theme === 'dark' ? null : 4000,
            memory_bytes_used: 1024 ** 3,
            memory_bytes_limit: theme === 'dark' ? null : 8 * 1024 ** 3,
            memory_bytes_peak: null,
            stall_permille: null,
            ceiling_hits: null,
            allocation_failures: null,
            kills: null,
            received_bytes: 2048,
            sent_bytes: 1024,
          });
        }
        if (path.endsWith('/agt_ready/transcript'))
          return reply({
            agent_id: 'agt_ready',
            through_seq: 7,
            turns: [
              {
                turn_sequence: 2,
                started_at: now,
                owner_message: first,
                agent_messages: ['The repository is ready.'],
                messages: [
                  { sequence: 3, role: 'user', content: first, appended_at: now },
                  {
                    sequence: 6,
                    role: 'assistant',
                    content: 'The repository is ready.',
                    appended_at: now,
                  },
                ],
                outcome: { type: 'completed', at: now },
              },
            ],
          });
        if (path.endsWith('/agt_ready/events')) {
          const cursor = Number(url.searchParams.get('after') ?? 0);
          historyCursors.push(cursor);
          if (cursor === 4 && activityUnavailable)
            return route.fulfill({
              status: 503,
              json: {
                status: 503,
                code: 'auth_temporarily_unavailable',
                detail: 'Authentication authority is temporarily unavailable.',
              },
            });
          const events = [
            { seq: 2, appended_at: now, kind: { type: 'turn_started', turn_sequence: 2 } },
            { seq: 3, appended_at: now, kind: { type: 'owner_message', content: first } },
            {
              seq: 4,
              appended_at: now,
              kind: {
                type: 'tool_call_started',
                call_id: 'read',
                title:
                  '/home/cma/trureturing/.worktrees/formalize/docs/very-long-formalization-path/README.md',
                kind: 'read',
                status: 'running',
              },
            },
            {
              seq: 5,
              appended_at: now,
              kind: { type: 'tool_call_updated', call_id: 'read', status: 'completed' },
            },
            {
              seq: 6,
              appended_at: now,
              kind: { type: 'agent_message', content: 'The repository is ready.' },
            },
            { seq: 7, appended_at: now, kind: { type: 'turn_completed', turn_sequence: 2 } },
          ];
          return reply(
            cursor === 0
              ? { items: events.filter((event) => event.seq <= 4), next: 4 }
              : { items: events.filter((event) => event.seq > cursor) },
          );
        }
        if (path.startsWith('/api/')) throw new Error(`Missing fixture ${path}`);
        return route.continue();
      });
      const tag = `${theme}-${viewport.width}`;
      const screenshot = async (name) => {
        const path = join(output, `${tag}-${name}.png`);
        await page.screenshot({ path, fullPage: true });
        return path;
      };
      const pixels = async (locator) => {
        const png = await locator.screenshot({ omitBackground: true });
        const data = await page.evaluate(
          async (url) => {
            const img = new Image();
            img.src = url;
            await img.decode();
            const canvas = document.createElement('canvas');
            canvas.width = img.width;
            canvas.height = img.height;
            const ctx = canvas.getContext('2d');
            ctx.drawImage(img, 0, 0);
            const values = ctx.getImageData(0, 0, img.width, img.height).data;
            let painted = 0;
            let blue = 0;
            let green = 0;
            let yellow = 0;
            for (let index = 0; index < values.length; index += 4) {
              const [r, g, b, a] = values.slice(index, index + 4);
              if (a > 32) painted++;
              if (a > 32 && b > r * 1.3 && b > g * 1.1) blue++;
              if (a > 32 && g > r * 1.15 && g > b * 1.15) green++;
              if (a > 32 && r > b * 1.3 && g > b * 1.3) yellow++;
            }
            return { painted, blue, green, yellow, width: img.width, height: img.height };
          },
          `data:image/png;base64,${png.toString('base64')}`,
        );
        return { png, ...data };
      };
      await page.goto(base);
      await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeEnabled();
      const brand = page.locator('.landing__character[data-ready]');
      await expect(brand).toHaveCount(1);
      const brandA = await pixels(brand);
      assert(brandA.painted > 500, 'Original 3D character must have painted pixels');
      await brand.click();
      const brandB = await pixels(brand);
      assert(!brandA.png.equals(brandB.png), 'Character interaction must move its actual pixels');
      const authShot = await screenshot('auth');
      await page.goto(`${base}/t/public-example/plc_readme`);
      await expect(page.getByRole('button', { name: 'Open in CMA', exact: true })).toBeVisible();
      assert.equal(launches, 0, 'Plain navigation must not admit work');
      await page.goto(`${fixtureBase}/fixture-readme`);
      await page.getByRole('link', { name: 'Try it' }).click();
      await expect(page).toHaveURL(/\/launches\/lch_example$/);
      assert.equal(launches, 1, 'One external click must admit exactly once through sign-in');
      await expect(page.locator('.public-step .landing__character[data-ready]')).toHaveCount(7);
      const running = page.locator('.public-step [data-state="running"]');
      const done = page.locator('.public-step [data-state="completed"]').first();
      const pending = page.locator('.public-step [data-state="pending"]').first();
      const runningA = await pixels(running);
      const doneA = await pixels(done);
      const pendingA = await pixels(pending);
      assert(runningA.blue > 20, 'Running step must be blue');
      assert(doneA.green > 20, 'Completed step must be green');
      assert(pendingA.yellow > 20, 'Pending step must be yellow');
      const runningB = await pixels(running);
      const doneB = await pixels(done);
      const pendingB = await pixels(pending);
      assert(!runningA.png.equals(runningB.png), 'Running character must spin');
      assert(doneA.png.equals(doneB.png), 'Completed character must stay still');
      assert(pendingA.png.equals(pendingB.png), 'Pending character must stay still');
      const progressShot = await screenshot('progress');
      await page.emulateMedia({ reducedMotion: 'reduce' });
      const stillA = await pixels(running);
      const stillB = await pixels(running);
      assert(stillA.png.equals(stillB.png), 'Reduced motion stops progress animation');
      await page.emulateMedia({ reducedMotion: 'no-preference' });
      finished = true;
      await expect(page).toHaveURL(/\/agents\/agt_ready$/, { timeout: 15000 });
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeFocused();
      await expect(page.getByText('The repository is ready.')).toBeVisible();
      await expect(
        page.getByText('Conversation activity is unavailable. Reconnecting.'),
      ).toBeVisible();
      const draft = page.getByRole('textbox', { name: 'Message', exact: true });
      await draft.fill('Preserve this unsubmitted draft');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeDisabled();
      const recoveryShot = await screenshot('recovering-activity');
      activityUnavailable = false;
      await expect(
        page.getByText('Conversation activity is unavailable. Reconnecting.'),
      ).toHaveCount(0, { timeout: 15000 });
      await expect(draft).toHaveValue('Preserve this unsubmitted draft');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      assert.deepEqual(historyCursors.slice(0, 4), [0, 4, 0, 4]);
      await draft.fill('');
      await page.getByRole('button', { name: /^Thinking/ }).click();
      await expect(page.locator('.thinking__body')).toBeVisible();
      const capacityLabel = theme === 'dark' ? ' configured' : '';
      await expect(page.getByText(`CPU · 0.5 cores / 4 cores${capacityLabel}`)).toBeVisible({
        timeout: 10000,
      });
      await expect(page.getByText(`RAM · 1 GiB / 8 GiB${capacityLabel}`)).toBeVisible();
      await expect(page.getByRole('progressbar', { name: 'RAM' })).toHaveAttribute(
        'aria-valuetext',
        `1 GiB of 8 GiB${capacityLabel}`,
      );
      await expect(page.getByText('Network · ↓ 2 KiB · ↑ 1 KiB')).toBeVisible();
      await expect(page.getByRole('progressbar', { name: 'Context' })).toHaveAttribute(
        'aria-valuenow',
        '14050',
      );
      const fits = await page.evaluate(() => ({
        width: document.documentElement.scrollWidth,
        viewport: innerWidth,
        send: getComputedStyle(document.querySelector('.agent-composer__submit')).borderRadius,
        sendWidth: document.querySelector('.agent-composer__submit').getBoundingClientRect().width,
        sendHeight: document.querySelector('.agent-composer__submit').getBoundingClientRect()
          .height,
        centerWidth: document.querySelector('.frame__center').getBoundingClientRect().width,
        childWidths: [...document.querySelector('.agent-surface').children].map(
          (element) => element.getBoundingClientRect().width,
        ),
        metrics: getComputedStyle(document.querySelector('.resources')).gridTemplateColumns.split(
          ' ',
        ).length,
      }));
      assert(fits.width <= fits.viewport);
      assert.equal(fits.send, '50%');
      assert.equal(fits.sendWidth, viewport.width === 390 ? 44 : 34);
      assert.equal(fits.sendHeight, fits.sendWidth);
      assert(fits.childWidths.every((width) => width <= fits.centerWidth + 1));
      assert.equal(fits.metrics, 3);
      const agentShot = await screenshot('agent');
      let streamCursor = 7;
      const emit = (event) => {
        assert(agentStream !== undefined && !agentStream.destroyed);
        const boundary = event.type === 'RUN_STARTED' || event.type === 'RUN_FINISHED';
        agentStream.write(
          `${boundary ? `id: ${++streamCursor}\n` : ''}data: ${JSON.stringify({ threadId: agent.id, runId: 'follow-proof', ...(!boundary ? { 'cma:live': true } : {}), ...event })}\n\n`,
        );
      };
      const bottomGap = () =>
        page
          .locator('.agent-surface__conversation')
          .evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight);
      emit({ type: 'RUN_STARTED' });
      emit({ type: 'TEXT_MESSAGE_START', messageId: 'follow-message', role: 'assistant' });
      emit({
        type: 'TEXT_MESSAGE_CONTENT',
        messageId: 'follow-message',
        delta: 'Follow proof.\n\n'.repeat(70),
      });
      await expect(page.locator('.message--assistant').last()).toContainText('Follow proof.');
      await expect.poll(bottomGap).toBeLessThan(2);
      emit({
        type: 'TEXT_MESSAGE_CONTENT',
        messageId: 'follow-message',
        delta: 'New content at the end.\n\n'.repeat(10),
      });
      await expect(page.locator('.message--assistant').last()).toContainText(
        'New content at the end.',
      );
      await expect.poll(bottomGap).toBeLessThan(2);
      const scroller = page.locator('.agent-surface__conversation');
      await scroller.hover();
      await page.mouse.wheel(0, -450);
      await expect.poll(bottomGap).toBeGreaterThan(200);
      const manualTop = await scroller.evaluate((element) => element.scrollTop);
      emit({
        type: 'TEXT_MESSAGE_CONTENT',
        messageId: 'follow-message',
        delta: 'Preserve manual position.\n\n'.repeat(10),
      });
      await expect(page.locator('.message--assistant').last()).toContainText(
        'Preserve manual position.',
      );
      await expect.poll(() => scroller.evaluate((element) => element.scrollTop)).toBe(manualTop);
      await page.mouse.wheel(0, 100000);
      await expect.poll(bottomGap).toBeLessThan(2);
      emit({
        type: 'TEXT_MESSAGE_CONTENT',
        messageId: 'follow-message',
        delta: 'Following restored.\n\n'.repeat(10),
      });
      await expect(page.locator('.message--assistant').last()).toContainText('Following restored.');
      await expect.poll(bottomGap).toBeLessThan(2);
      emit({ type: 'TEXT_MESSAGE_END', messageId: 'follow-message' });
      emit({ type: 'RUN_FINISHED' });
      const followShot = await screenshot('follow');
      assert.deepEqual(errors, []);
      evidence.push({
        theme,
        viewport,
        authShot,
        progressShot,
        agentShot,
        recoveryShot,
        transientHistoryRecoveryPreservesDraft: true,
        followShot,
        followsLiveOutputAndPreservesManualScroll: true,
        launches,
        samples,
        pixels: {
          brand: brandA.painted,
          running: runningA.blue,
          completed: doneA.green,
          pending: pendingA.yellow,
        },
      });
      await context.close();
      agentStream?.end();
      agentStream = undefined;
    }
  }
  await writeFile(
    join(output, 'results.json'),
    JSON.stringify({ result: 'passed', evidence }, null, 2),
  );
  console.log(output);
} finally {
  await browser.close();
  await new Promise((resolve) => fixture.close(resolve));
}
