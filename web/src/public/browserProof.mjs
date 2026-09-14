/* global process, console, URL, document, window, location, innerWidth, getComputedStyle, requestAnimationFrame */
import assert from 'node:assert/strict';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { pathToFileURL } from 'node:url';
import puppeteer from 'puppeteer-core';
import { chrome } from '../../scripts/browser.mjs';
import {
  now,
  first,
  preview,
  initial,
  trust,
  ready,
  agent,
  workspace,
} from './browserFixtures.mjs';

// The optional third argument is an installed Lighthouse core/index.js, outside this package.
const base = process.argv[2] ?? 'http://127.0.0.1:4342';
assert(
  ['127.0.0.1', 'localhost'].includes(new URL(base).hostname),
  'proof uses a local server only',
);
const snapshot =
  process.argv[3] === undefined
    ? undefined
    : (await import(pathToFileURL(process.argv[3]).href)).snapshot;
const output = await mkdtemp(join(tmpdir(), 'cma-public-browser-'));
console.log('Proof artifacts:', output);
let signedIn = false;
let mode = 'normal';
let record = initial;
let connected = false;
let admissionFailure = false;
let repeat = false;
let availability = 'published';
const mutations = [];
const unknown = [];
const errors = [];
const screenshots = [];
const accessibility = {};
const browser = await puppeteer.launch({ executablePath: chrome(), headless: true });
try {
  const page = await browser.newPage();
  page.on('pageerror', (error) => {
    errors.push(error.message);
    console.error(error);
  });
  await page.setRequestInterception(true);
  page.on('request', async (request) => {
    const url = new URL(request.url());
    const path = url.pathname;
    const reply = (body, status = 200) =>
      request.respond({ status, contentType: 'application/json', body: JSON.stringify(body) });
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
        social_providers: ['github', 'google'],
        email_signup: false,
        invite_code_required: false,
        register_available: true,
      });
    if (path === '/auth/login') {
      signedIn = true;
      return request.respond({
        status: 302,
        headers: { location: url.searchParams.get('return_to') },
        body: '',
      });
    }
    if (!path.startsWith('/api/')) return request.continue();
    if (request.method() === 'POST') {
      const entry = {
        path,
        body: request.postData() === undefined ? undefined : JSON.parse(request.postData()),
        key: request.headers()['idempotency-key'],
      };
      mutations.push(entry);
      assert(entry.key, 'command key required');
      if (path.endsWith('/public/public-example/launches')) {
        if (admissionFailure) {
          admissionFailure = false;
          return reply(
            { status: 503, code: 'launch_service_unavailable', detail: 'Unavailable' },
            503,
          );
        }
        if (repeat && entry.body.choice === undefined)
          return reply(
            {
              status: 409,
              code: 'trigger_repeat_choice_required',
              detail: 'Choose',
              existing: {
                agent_id: 'agt_previous',
                title: 'Previous research',
                created_at: now,
                first_message_excerpt: 'Find the project tests',
              },
            },
            409,
          );
        record =
          mode === 'review'
            ? trust
            : mode === 'connection'
              ? {
                  ...initial,
                  state: 'draft',
                  review: {
                    can_start_now: false,
                    needs: [{ type: 'connection', service: 'code' }],
                  },
                }
              : initial;
        return reply(record, record.state === 'draft' ? 201 : 202);
      }
      if (path.endsWith('/lch_example/start')) {
        record = initial;
        return reply(record, 202);
      }
      if (path === '/api/v1/service-connections')
        return reply(
          {
            handoff_id: 'handoff-one',
            hosted_url: 'https://identity.example.test/connect',
            expires_at: new Date(Date.now() + 600_000).toISOString(),
          },
          201,
        );
    }
    if (path === '/api/v1/triggers/public/public-example')
      return reply({ ...preview, availability });
    if (path === '/api/v1/launches/lch_example') return reply(record);
    if (path === '/api/v1/launches/lch_example/requirements')
      return reply({
        services: [
          {
            service: 'code',
            state: connected ? 'satisfied' : 'missing',
            display_name: 'Code host',
            connection_mode: 'hosted',
          },
        ],
        checked_at: now,
      });
    if (path === '/api/v1/service-connections/handoff-one')
      return reply({
        handoff_id: 'handoff-one',
        launch_id: 'lch_example',
        service_id: 'code',
        state: connected ? 'satisfied' : 'pending',
        expires_at: new Date(Date.now() + 600_000).toISOString(),
        updated_at: now,
      });
    // Finite read-only companions exercise the caller's fallback without creating another run.
    if (path === '/api/v1/agui/launches/lch_example/events' || path === '/api/v1/agui/events')
      return request.respond({ contentType: 'text/event-stream', body: ': keep-alive\n\n' });
    if (path === '/api/v1/sandboxes') return reply({ items: [] });
    if (path === '/api/v1/agents/agt_ready') return reply(agent);
    if (path === '/api/v1/workspaces/wsp_one') return reply(workspace);
    if (path === '/api/v1/agents/agt_ready/usage')
      return reply({
        agent_id: 'agt_ready',
        through_seq: 5,
        input_tokens: 5,
        output_tokens: 6,
        context_used: 11,
      });
    if (path === '/api/v1/agents/agt_ready/transcript')
      return reply({
        agent_id: 'agt_ready',
        through_seq: 5,
        turns: [
          {
            turn_sequence: 2,
            started_at: now,
            owner_message: first,
            agent_messages: ['The repository is ready.'],
            messages: [
              { sequence: 3, role: 'user', content: first, appended_at: now },
              {
                sequence: 4,
                role: 'assistant',
                content: 'The repository is ready.',
                appended_at: now,
              },
            ],
            outcome: { type: 'completed', at: now },
          },
        ],
      });
    if (path === '/api/v1/agents/agt_ready/events')
      return reply({
        items: [
          { seq: 2, appended_at: now, kind: { type: 'turn_started', turn_sequence: 2 } },
          { seq: 3, appended_at: now, kind: { type: 'owner_message', content: first } },
          {
            seq: 4,
            appended_at: now,
            kind: { type: 'agent_message', content: 'The repository is ready.' },
          },
          { seq: 5, appended_at: now, kind: { type: 'turn_completed', turn_sequence: 2 } },
        ],
      });
    unknown.push(path);
    return reply({ status: 404, code: 'not_found', detail: 'Unknown fixture' }, 404);
  });
  const click = async (text) => {
    await page.waitForFunction(
      (text) =>
        [...document.querySelectorAll('button')].some(
          (node) => node.textContent.trim() === text && !node.disabled,
        ),
      {},
      text,
    );
    await page.evaluate(
      (text) =>
        [...document.querySelectorAll('button')]
          .find((node) => node.textContent.trim() === text && !node.disabled)
          .click(),
      text,
    );
  };
  const audit = async (name) => {
    if (snapshot === undefined) return;
    const report = await snapshot(page, {
      flags: { onlyCategories: ['accessibility'], screenEmulation: { disabled: true } },
    });
    accessibility[name] = report.lhr.categories.accessibility.score;
    await writeFile(join(output, `lighthouse-${name}.json`), JSON.stringify(report.lhr, null, 2));
    await writeFile(join(output, `lighthouse-${name}.html`), report.report);
    assert(accessibility[name] >= 0.95, `${name} accessibility ${accessibility[name]}`);
  };
  const capture = async (name) => {
    for (const [width, height] of [
      [1440, 900],
      [390, 844],
      [320, 740],
    ]) {
      await page.setViewport({ width, height });
      await page.waitForFunction(() => {
        const frame = document.querySelector('.frame');
        return (
          frame === null ||
          Math.abs(
            getComputedStyle(frame)
              .gridTemplateColumns.split(' ')
              .map(parseFloat)
              .reduce((sum, value) => sum + value, 0) - innerWidth,
          ) < 1
        );
      });
      await page.evaluate(async () => {
        await document.fonts.ready;
        await new Promise((resolve) => requestAnimationFrame(resolve));
      });
      const geometry = await page.evaluate(() => ({
        overflow: document.documentElement.scrollWidth > innerWidth,
        dialogs: document.querySelectorAll('[role="dialog"]').length,
        clipped: [...document.querySelectorAll('[role="dialog"],button')].some(
          (node) =>
            node.getBoundingClientRect().width > innerWidth ||
            node.scrollWidth > node.clientWidth + 1,
        ),
      }));
      assert(
        !geometry.overflow && !geometry.clipped && geometry.dialogs <= 1,
        `${name}/${width}: ${JSON.stringify(geometry)}`,
      );
      const target = join(output, `${name}-${width}.png`);
      await page.screenshot({ path: target });
      screenshots.push(target);
    }
    await page.evaluate(async () => {
      document.body.setAttribute('data-ds-dark-theme', '');
      await new Promise((resolve) => requestAnimationFrame(resolve));
      await Promise.all(
        document
          .getAnimations()
          .filter((animation) => animation.effect.getTiming().iterations !== Infinity)
          .map((animation) => animation.finished.catch(() => {})),
      );
    });
    const target = join(output, `${name}-dark.png`);
    await page.screenshot({ path: target });
    screenshots.push(target);
    if (['trust', 'connect-blocked', 'repeat'].includes(name)) await audit(name);
    await page.evaluate(() => document.body.removeAttribute('data-ds-dark-theme'));
    await page.setViewport({ width: 1440, height: 900 });
    console.log('Captured', name);
  };
  await page.goto(`${base}/t/public-example/plc_readme`, { waitUntil: 'networkidle0' });
  await page.waitForSelector('.public-signup');
  assert.equal(mutations.length, 0);
  await capture('landing');
  await audit('landing');
  await click('Open in CMA');
  await page.waitForFunction(() => location.pathname === '/launches/lch_example');
  await page.waitForSelector('.public-timeline');
  assert.equal(mutations.length, 1);
  await capture('progress');
  record = ready;
  await page.waitForFunction(() => location.pathname === '/agents/agt_ready');
  await page.waitForSelector('textarea#agent-message');
  await page.waitForFunction(() => document.activeElement?.id === 'agent-message');
  assert.equal(mutations.length, 1, 'progress must never submit another first message');
  await capture('agent');
  mode = 'review';
  await page.goto(`${base}/t/public-example`, { waitUntil: 'networkidle0' });
  await click('Open in CMA');
  await page.waitForSelector('[role="dialog"]');
  assert.equal(
    await page.$eval('pre code', (node) => node.textContent),
    trust.trust.disclosures[0].commands[0],
  );
  await capture('trust');
  await page.click('input[type="checkbox"]');
  await click('Continue');
  await page.waitForFunction(() => !document.querySelector('[role="dialog"]'));
  assert.equal(mutations.at(-1).body.trust_confirmed_digest, 'approved-one');
  mode = 'connection';
  await page.evaluateOnNewDocument(() => {
    window.open = () => null;
  });
  await page.goto(`${base}/t/public-example`, { waitUntil: 'networkidle0' });
  await click('Open in CMA');
  await page.waitForFunction(() => document.body.textContent.includes('Connect Code host'));
  await click('Connect');
  await page.waitForFunction(() => document.body.textContent.includes('Your browser blocked'));
  await capture('connect-blocked');
  const connectionMutations = mutations.length;
  await page.reload({ waitUntil: 'networkidle0' });
  await page.waitForFunction(() => document.body.textContent.includes('Verify'));
  assert.equal(
    mutations.length,
    connectionMutations,
    'same-tab return must only read the existing handoff',
  );
  connected = true;
  await page.waitForFunction(() => !document.querySelector('[role="dialog"]'));
  assert.equal(mutations.at(-1).path, '/api/v1/launches/lch_example/start');
  repeat = true;
  mode = 'normal';
  await page.goto(`${base}/t/public-example`, { waitUntil: 'networkidle0' });
  await click('Open in CMA');
  await page.waitForFunction(() => document.body.textContent.includes('Previous research'));
  await capture('repeat');
  await click('Continue where you left off');
  await page.waitForFunction(() => location.pathname === '/launches/lch_example');
  assert.equal(mutations.at(-1).body.choice, 'resume');
  repeat = false;
  admissionFailure = true;
  await page.goto(`${base}/t/public-example`, { waitUntil: 'networkidle0' });
  await click('Open in CMA');
  await page.waitForFunction(() => document.body.textContent.includes('CMA could not complete'));
  await capture('uncertain');
  const before = mutations.at(-1);
  await click('Try again');
  await page.waitForFunction(() => location.pathname === '/launches/lch_example');
  assert.deepEqual(mutations.at(-1), before);
  availability = 'retired';
  await page.goto(`${base}/t/public-example`, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => document.body.textContent.includes('no longer available'));
  assert(!(await page.$('.public-actions button')));
  await capture('retired');
  assert.deepEqual(unknown, []);
  assert.deepEqual(errors, []);
  const result = {
    screenshots,
    accessibility,
    lighthouseRan: snapshot !== undefined,
    mutations,
    unknown,
    errors,
  };
  await writeFile(join(output, 'result.json'), JSON.stringify(result, null, 2));
  console.log(
    JSON.stringify({
      captures: screenshots.length,
      accessibility,
      mutationCount: mutations.length,
      unknown,
      errors,
    }),
  );
} finally {
  await browser.close();
}
