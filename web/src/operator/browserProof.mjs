/* global process, URL, document, innerWidth, console, window */
import assert from 'node:assert/strict';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';
import { chrome } from '../../scripts/browser.mjs';
import { metricRow, reportFixture, seriesFixture } from '../usage/metrics/fixtures.test-support.ts';

const base = process.argv[2] ?? 'http://127.0.0.1:5188';
assert(
  ['127.0.0.1', 'localhost'].includes(new URL(base).hostname),
  'proof uses a local server only',
);
const isolated = process.argv.includes('--isolated');
const sdkModule = `/@fs${fileURLToPath(new URL('../api-client/index.ts', import.meta.url))}`;
const output = await mkdtemp(join(tmpdir(), 'cma-operator-browser-'));
console.log('Proof artifacts:', output);
const now = '2026-09-06T01:00:00Z';
let capabilities = [
  'read_all_sandboxes',
  'control_any_sandbox',
  'configure_platform',
  'read_activity_log',
];
const sandbox = {
  id: 'sbx-research',
  owner: 'operator@example.test',
  owner_name: 'Shining',
  name: 'Research environment',
  status: 'active',
  runtime_version: 'cma-runtime-2.4.1',
  agent_count: 2,
  last_activity_at: now,
  retired_at: null,
};
const profile = {
  kind: 'agent',
  id: 'agp-research',
  owner: 'operator@example.test',
  owner_name: 'Shining',
  name: 'Research and review',
  visibility: 'public',
  updated_at: now,
  retired_at: null,
};
let settings = {
  defaults: {
    sandbox_profile: { id: 'sbp_default', revision: 1 },
    workspace_profile: { id: 'wkp_default', revision: 2 },
    agent_profile: { id: 'agp_default', revision: 3 },
  },
  inactivity: { pause_after_seconds: 900 },
  retention: { agent_log_days: 30 },
  limits: { max_sandboxes_per_user: 10, visitor_max_sandboxes: 1, visitor_max_launches_per_day: 5 },
  triggers: { enabled: true, orchestration_paused: false },
  marketplace: { enabled: true },
  future: 'preserved',
};
let revision = 4;
let conflict = true;
const writes = [];
const requests = [];
const errors = [];
const browser = await puppeteer.launch({
  executablePath: chrome(),
  headless: true,
  args: ['--no-sandbox'],
});
try {
  const page = await browser.newPage();
  page.on('pageerror', (error) => {
    errors.push(String(error));
    console.error(String(error));
  });
  page.on('requestfailed', (request) =>
    console.error('Request failed:', request.url(), request.failure()?.errorText),
  );
  await page.setRequestInterception(true);
  page.on('request', async (request) => {
    const url = new URL(request.url());
    const path = url.pathname;
    const reply = (body, status = 200) =>
      request.respond({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (isolated && path === '/operations') {
      return request.respond({
        contentType: 'text/html',
        body: `<!doctype html><html data-theme="light"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root" style="height:100dvh"></div><script type="module">
import RefreshRuntime from '/@react-refresh'; RefreshRuntime.injectIntoGlobalHook(window); window.$RefreshReg$=()=>{}; window.$RefreshSig$=()=>type=>type; window.__vite_plugin_react_preamble_installed__=true;
await import('/@vite/client');
for (const sheet of ['tokens','design-platform','effects-typography','scrollbar','shiki','base','components']) await import('/src/styles/'+sheet+'.css');
const {default: React} = await import('/node_modules/.vite/deps/react.js');
const {default: ReactDOM} = await import('/node_modules/.vite/deps/react-dom_client.js');
const {Operations} = await import('/src/operator/Operations.tsx');
const {createOperatorClient} = await import('/src/operator/client.ts');
const {createClient} = await import(${JSON.stringify(sdkModule)});
ReactDOM.createRoot(document.getElementById('root')).render(React.createElement(Operations,{client:createOperatorClient(createClient()),capabilities:${JSON.stringify(capabilities)}}));
</script></body></html>`,
      });
    }
    if (path === '/auth/session')
      return reply({
        authenticated: true,
        subject: 'operator-1',
        display_name: 'Operator',
        capabilities,
      });
    if (!path.startsWith('/api/')) return request.continue();
    requests.push(`${request.method()} ${path}${url.search}`);
    if (path.startsWith('/api/v1/admin/usage/metrics/reports')) {
      const report = reportFixture(true);
      if (path.endsWith('/sandboxes'))
        return reply({
          report_id: report.report_id,
          as_of: report.as_of,
          items: [
            { ...metricRow, sandbox_id: sandbox.id, name: sandbox.name, status: sandbox.status },
          ],
          next: null,
          total_count: 1,
        });
      if (path.endsWith('/series')) return reply(seriesFixture(url.searchParams.get('metric')));
      return reply(report);
    }
    if (request.method() !== 'GET')
      writes.push({
        path,
        method: request.method(),
        body: request.postData() ? JSON.parse(request.postData()) : null,
      });
    if (path === '/api/v1/sandboxes') return reply({ items: [] });
    if (path === '/api/v1/admin/sandboxes') return reply({ items: [sandbox], next: null });
    if (path === '/api/v1/admin/sandboxes/sbx-research/retire' && request.method() === 'POST') {
      sandbox.status = 'retiring';
      return reply({
        ...sandbox,
        profile: { id: 'sbp_default', revision: 1, digest: 'digest' },
        limits: { max_workspaces: 3, max_agents_per_workspace: 4 },
        created_at: now,
        updated_at: now,
        retired_at: undefined,
        revision: 2,
      });
    }
    if (path === '/api/v1/admin/sandboxes/sbx-research')
      return reply({
        owner_name: 'Shining',
        sandbox: {
          ...sandbox,
          profile: { id: 'sbp_default', revision: 1, digest: 'digest' },
          runtime: { runtime_version: sandbox.runtime_version },
          limits: { max_workspaces: 3, max_agents_per_workspace: 4 },
          created_at: now,
          updated_at: now,
          revision: 1,
          retired_at: undefined,
        },
        workspaces: [
          {
            id: 'wks-research',
            name: 'Research workspace',
            status: 'ready',
            profile_id: 'wkp_default',
            profile_revision: 2,
          },
        ],
        agents: [
          {
            id: 'agt-review',
            workspace_id: 'wks-research',
            worktree_id: 'wkt-review',
            status: 'ready',
            profile_id: 'agp_default',
            profile_revision: 3,
            title: 'Review current findings',
            updated_at: now,
          },
        ],
      });
    if (path === '/api/v1/admin/sandboxes/sbx-research/events')
      return reply([
        {
          sandbox_id: sandbox.id,
          seq: 1,
          appended_at: now,
          kind: { type: 'status_changed', from: 'installing', to: 'active' },
        },
      ]);
    if (path === '/api/v1/admin/profiles')
      return reply({
        items: [profile, { ...profile, id: 'agp_default', name: 'Deployment Agent default' }],
        next: null,
      });
    if (path === '/api/v1/admin/settings' && request.method() === 'GET')
      return reply({ revision, settings });
    if (path === '/api/v1/admin/settings' && request.method() === 'PUT') {
      if (conflict) {
        conflict = false;
        revision = 6;
        return reply({ status: 409, title: 'Conflict', code: 'settings_conflict' }, 409);
      }
      const body = JSON.parse(request.postData());
      assert.equal(body.expected_revision, revision);
      assert.equal(body.settings.future, 'preserved');
      revision += 1;
      settings = body.settings;
      return reply({ revision, settings: body.settings });
    }
    if (path === '/api/v1/admin/activity')
      return reply({
        items: [
          {
            id: 'activity-1',
            at: now,
            actor: 'operator@example.test',
            action: 'configure',
            target: { kind: 'resource', id: 'platform-settings' },
            outcome: { status: 'succeeded' },
            detail: {},
          },
        ],
        next: null,
      });
    const kind = path.match(/^\/api\/v1\/marketplace\/(sandbox|workspace|agent)-profiles$/)?.[1];
    if (kind)
      return reply({
        items: [
          {
            kind,
            id: `${kind}-public`,
            name: `Published ${kind} default`,
            description: 'Published default',
            owner_display: 'Platform',
            latest_revision: 7,
            published_at: now,
            uses_count: 2,
          },
        ],
      });
    return reply({ status: 404, title: 'Unexpected proof request', code: 'not_found' }, 404);
  });
  const clickText = async (selector, text) => {
    await page.waitForFunction(
      (selector, text) =>
        [...document.querySelectorAll(selector)].some(
          (element) => element.textContent?.trim() === text,
        ),
      {},
      selector,
      text,
    );
    await page.evaluate(
      (selector, text) =>
        [...document.querySelectorAll(selector)]
          .find((element) => element.textContent?.trim() === text)
          ?.click(),
      selector,
      text,
    );
    if (selector === '[role="tab"]')
      await page.waitForFunction(
        (text) =>
          [...document.querySelectorAll('[role="tab"][aria-selected="true"]')].some(
            (element) => element.textContent === text,
          ),
        {},
        text,
      );
  };
  const assertLayout = async () => {
    await page.waitForFunction(() => {
      const frame = document.querySelector('.frame');
      if (!frame) return true;
      const tracks = window.getComputedStyle(frame).gridTemplateColumns.split(' ').map(parseFloat);
      return Math.abs(tracks.reduce((sum, width) => sum + width, 0) - frame.clientWidth) < 1;
    });
    const result = await page.evaluate(() => ({
      width: innerWidth,
      scroll: document.documentElement.scrollWidth,
      clipped: [
        ...document.querySelectorAll(
          '.frame__center, .console__surface, .operator, .operator-table',
        ),
      ]
        .filter((element) => element.getBoundingClientRect().right > innerWidth + 1)
        .map((element) => element.className),
      overflow: [...document.querySelectorAll('.operator-field, .operator-settings')]
        .filter((element) => element.scrollWidth > element.clientWidth + 1)
        .map((element) => element.className),
    }));
    assert(result.scroll <= result.width + 1, `viewport overflow ${JSON.stringify(result)}`);
    assert.deepEqual(result.overflow, []);
    assert.deepEqual(result.clipped, []);
  };
  await page.setViewport({ width: 1440, height: 1000 });
  await page.goto(`${base}/operations`, { waitUntil: 'networkidle0' });
  await page.waitForSelector('[aria-label="Sandboxes and allocated usage"] tbody tr');
  await assertLayout();
  await page.screenshot({ path: join(output, 'desktop-sandboxes.png'), fullPage: true });
  await page.focus('.operator-tabs [role="tab"][aria-selected="true"]');
  await page.keyboard.press('End');
  await page.waitForFunction(() => document.activeElement?.textContent === 'Activity');
  await page.keyboard.press('Home');
  await page.waitForFunction(() => document.activeElement?.textContent === 'Sandboxes & usage');
  await page.waitForSelector('[aria-label="Runtime details for Research environment"]');
  await page.click('[aria-label="Runtime details for Research environment"]');
  await page.waitForSelector('[role="dialog"] .operator-timeline li');
  assert.equal(
    await page.$eval('[role="dialog"]', (element) =>
      Math.round(element.getBoundingClientRect().width),
    ),
    700,
  );
  assert(
    await page.$eval('[role="dialog"]', (element) => element.contains(document.activeElement)),
  );
  await page.$eval('[role="dialog"]', (element) =>
    [...element.querySelectorAll('button:not(:disabled), a[href]')].at(-1).focus(),
  );
  await page.keyboard.press('Tab');
  assert.equal(
    await page.evaluate(() => document.activeElement?.getAttribute('aria-label')),
    'Close',
  );
  await page.keyboard.down('Shift');
  await page.keyboard.press('Tab');
  await page.keyboard.up('Shift');
  assert(
    await page.$eval('[role="dialog"]', (element) => element.contains(document.activeElement)),
  );
  await page.screenshot({ path: join(output, 'desktop-drawer.png'), fullPage: true });
  await page.keyboard.press('Escape');
  assert.equal(
    await page.evaluate(() => document.activeElement?.getAttribute('aria-label')),
    'Runtime details for Research environment',
  );
  await clickText('[role="tab"]', 'Platform settings');
  await page.waitForSelector('[aria-label="Settings overview"]');
  assert.equal(
    await page.$$eval('[aria-label="Settings overview"] a', (links) => links.length),
    19,
  );
  await page.click('button[aria-label="Edit Idle threshold"]');
  await page.click('button[aria-label="Edit Trigger orchestration"]');
  await page.waitForSelector('[aria-label="Pause after (seconds)"]');
  await page.locator('[aria-label="Pause after (seconds)"]').fill('600');
  assert.equal(await page.$('[aria-label="Default agent profile"]'), null);
  await clickText('label', 'Orchestration paused');
  assert.deepEqual(
    await page.$$eval('form input:invalid', (inputs) =>
      inputs.map((input) => ({
        name: input.getAttribute('aria-label'),
        value: input.value,
        message: input.validationMessage,
      })),
    ),
    [],
  );
  await clickText('button', 'Save settings');
  await page.waitForSelector('.operator-conflict');
  assert.equal(writes.length, 1);
  await page.screenshot({ path: join(output, 'desktop-settings-conflict.png'), fullPage: true });
  await clickText('button', 'Review retained draft');
  assert.equal(writes.length, 1);
  await clickText('button', 'Save settings');
  await page.waitForFunction(() =>
    document.body.textContent.includes('Settings saved at revision 7.'),
  );
  assert.equal(writes.length, 2);
  assert.equal(writes[1].body.settings.inactivity.pause_after_seconds, 600);
  assert.deepEqual(writes[1].body.settings.defaults.agent_profile, {
    id: 'agp_default',
    revision: 3,
  });
  assert.equal(writes[1].body.settings.triggers.orchestration_paused, true);
  for (const view of ['Sandboxes & usage', 'Platform settings', 'Activity']) {
    await page.setViewport({ width: 390, height: 844 });
    await clickText('[role="tab"]', view);
    await page.waitForFunction(
      () =>
        !document.querySelector('[aria-label="Loading records"], [aria-label="Loading settings"]'),
    );
    await assertLayout();
    await page.screenshot({
      path: join(output, `mobile-${view.replaceAll(' ', '-').toLowerCase()}.png`),
      fullPage: true,
    });
    if (view === 'Platform settings') {
      await page.$eval('.operator-scroll', (element) => {
        element.scrollTop = element.scrollHeight;
      });
      await page.screenshot({ path: join(output, 'mobile-settings-limits.png'), fullPage: true });
    }
  }
  await clickText('[role="tab"]', 'Sandboxes & usage');
  await page.waitForSelector('[aria-label="Runtime details for Research environment"]');
  await page.click('[aria-label="Runtime details for Research environment"]');
  await page.waitForSelector('.operator-timeline li');
  await assertLayout();
  assert.equal(
    await page.$eval('[role="dialog"]', (element) =>
      Math.round(element.getBoundingClientRect().width),
    ),
    358,
  );
  await page.screenshot({ path: join(output, 'mobile-drawer.png'), fullPage: true });
  await page.keyboard.press('Escape');
  await clickText('button', 'Retire');
  await page.waitForSelector('[role="dialog"]');
  assert.equal(await page.$eval('.risk__confirm', (button) => button.disabled), true);
  await page.click('.risk__acknowledgement input');
  assert.equal(await page.$eval('.risk__confirm', (button) => button.disabled), false);
  await page.screenshot({ path: join(output, 'mobile-confirmation.png'), fullPage: true });
  await clickText('button', 'Retire Sandbox');
  await page.waitForFunction(() => document.body.textContent.includes('Deleting'));
  assert.equal(writes.length, 3);
  assert.equal(writes[2].method, 'POST');
  assert.equal(writes[2].body, null);
  await page.setViewport({ width: 320, height: 740 });
  await assertLayout();
  await page.evaluate(async () => {
    const { setAppearance } = await import('/src/theme/appearance.ts');
    setAppearance('dark');
  });
  await page.screenshot({ path: join(output, 'compact-dark-sandboxes.png'), fullPage: true });
  const beforeRevocation = requests.length;
  capabilities = ['read_activity_log'];
  await page.goto(`${base}/operations`, { waitUntil: 'networkidle0' });
  await page.waitForSelector('.operator-table tbody tr');
  assert.deepEqual(
    await page.$$eval('.operator-tabs [role="tab"]', (tabs) => tabs.map((tab) => tab.textContent)),
    ['Activity'],
  );
  assert.equal(
    requests
      .slice(beforeRevocation)
      .filter((request) => request.includes('/admin/') && !request.includes('/admin/activity'))
      .length,
    0,
  );
  assert.deepEqual(errors, []);
  await writeFile(
    join(output, 'report.json'),
    JSON.stringify(
      {
        isolated,
        requests,
        writes,
        errors,
        viewports: ['1440x1000', '390x844', '320x740'],
        checks: [
          'layout',
          'drawer focus trap and restore',
          'tab keyboard navigation',
          'confirmation acknowledgement',
          'hidden compatibility defaults retain exact revision',
          'trigger pause',
          'settings conflict without automatic retry',
          'sandbox retirement',
          'capability-gated requests',
        ],
      },
      null,
      2,
    ),
  );
  console.log(
    JSON.stringify(
      {
        output,
        isolated,
        screenshots: 11,
        requests: requests.length,
        writes: writes.length,
        errors,
      },
      null,
      2,
    ),
  );
} finally {
  await browser.close();
}
