/* global process, URL, document, innerWidth, window, console */
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import puppeteer from 'puppeteer-core';
import { chrome } from '../../scripts/browser.mjs';

const base = process.argv[2] ?? 'http://127.0.0.1:5191';
assert(['127.0.0.1', 'localhost'].includes(new URL(base).hostname));
const output = await mkdtemp(join(tmpdir(), 'cma-publisher-browser-'));
console.log('Proof artifacts:', output);
const golden = JSON.parse(
  await readFile(
    new URL(
      '../../../api/crates/layer1-public/tests/fixtures/trigger-embed/descriptor.json',
      import.meta.url,
    ),
    'utf8',
  ),
);
const now = '2026-09-07T00:00:00Z';
const profiles = {
  sandbox: {
    id: 'sbp_standard',
    name: 'Standard',
    revision: 2,
    content: {
      cpu_millis: 2000,
      memory_mib: 4096,
      nyxid_services: ['github'],
      agent_provider: 'codex',
      model_access: 'chronoai_managed',
      max_workspaces: 4,
      max_agents_per_workspace: 4,
      workspace_profiles: [{ id: 'wkp_trureturing', revision: 3 }],
      software_install_commands: ['corepack enable'],
      environment_variables: [{ name: 'PROJECT_MODE', value: 'development' }],
    },
  },
  workspace: {
    id: 'wkp_trureturing',
    name: 'trureturing',
    revision: 3,
    content: {
      name: 'trureturing',
      location: {
        type: 'git_repository',
        url: 'https://github.com/the-omega-institute/trureturing',
        reference: 'main',
      },
      setup_commands: ['npm ci'],
      agent_profiles: [{ id: 'agp_codex', revision: 4 }],
    },
  },
  agent: {
    id: 'agp_codex',
    name: 'Codex',
    revision: 4,
    content: {
      model: 'gpt-5-codex',
      thinking_effort: 'high',
      permission_level: 'guarded',
      skills: [],
      system_prompt: '',
    },
  },
};
const revisionFor = (profile) => ({
  profile_id: profile.id,
  revision: profile.revision,
  content: profile.content,
  digest: 'a'.repeat(64),
  published_at: now,
});
const documentFor = (profile) => ({
  id: profile.id,
  owner: 'publisher',
  name: profile.name,
  description: `Published ${profile.name} profile`,
  visibility: 'public',
  uses_count: 0,
  draft: profile.content,
  draft_version: 8,
  latest_revision: revisionFor(profile),
  created_at: now,
  updated_at: now,
});
const requests = [];
const writes = [];
const errors = [];
let stored;
let statisticsFail = false;
const browser = await puppeteer.launch({
  executablePath: chrome(),
  headless: true,
  args: ['--no-sandbox'],
});
try {
  const page = await browser.newPage();
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.setRequestInterception(true);
  page.on('request', async (request) => {
    const url = new URL(request.url());
    const path = url.pathname;
    const reply = (body, status = 200) =>
      request.respond({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (url.origin === 'https://cma.example' && path.endsWith('/badge.svg')) {
      const theme = url.searchParams.get('theme') === 'dark' ? 'dark' : 'light';
      const body = await readFile(
        new URL(`./badges/open_in_cma-${theme}-available.svg`, import.meta.url),
        'utf8',
      );
      return request.respond({ contentType: 'image/svg+xml', body });
    }
    if (url.origin !== base) return request.abort();
    if (path === '/auth/session')
      return reply({
        authenticated: true,
        subject: 'publisher',
        display_name: 'Publisher',
        capabilities: ['publish_triggers'],
      });
    if (!path.startsWith('/api/')) return request.continue();
    requests.push(`${request.method()} ${path}${url.search}`);
    if (request.method() !== 'GET')
      writes.push({
        path,
        method: request.method(),
        key: request.headers()['idempotency-key'],
        body: request.postData() ? JSON.parse(request.postData()) : null,
      });
    if (path === '/api/v1/sandboxes') return reply({ items: [] });
    const profileRoute = path.match(
      /^\/api\/v1\/(sandbox|workspace|agent)-profiles(?:\/([^/]+))?(?:\/revisions\/(\d+))?$/,
    );
    if (profileRoute) {
      const profile = profiles[profileRoute[1]];
      if (profileRoute[3]) {
        assert.equal(Number(profileRoute[3]), profile.revision);
        return reply(revisionFor(profile));
      }
      return reply(profileRoute[2] ? documentFor(profile) : { items: [documentFor(profile)] });
    }
    if (path === '/api/v1/triggers' && request.method() === 'GET')
      return reply({ items: stored ? [stored] : [] });
    if (path === '/api/v1/triggers' && request.method() === 'POST') {
      stored = {
        id: 'trg_project',
        draft: JSON.parse(request.postData()).content,
        draft_version: 1,
        revision_count: 0,
        availability: 'draft',
        placements: [],
        stats: { launches_total: 0, agents_ready_total: 0 },
        created_at: now,
        updated_at: now,
      };
      return reply(stored, 201);
    }
    if (path === '/api/v1/triggers/trg_project' && request.method() === 'GET') return reply(stored);
    if (path === '/api/v1/triggers/trg_project/placements' && request.method() === 'POST') {
      const placement = {
        id: `plc_${stored.placements.length + 1}`,
        ...JSON.parse(request.postData()),
      };
      stored.placements.push(placement);
      return reply(placement, 201);
    }
    if (path === '/api/v1/triggers/trg_project/publish') {
      assert.deepEqual(JSON.parse(request.postData()), { draft_version: stored.draft_version });
      stored = {
        ...stored,
        availability: 'published',
        public_id: 'pub_example',
        revision_count: 1,
      };
      return reply(
        {
          trigger_id: stored.id,
          public_id: stored.public_id,
          availability: 'published',
          revision: 1,
          content: stored.draft,
          digest: 'b'.repeat(64),
          published_at: now,
        },
        201,
      );
    }
    if (path === '/api/v1/triggers/trg_project/embed') return reply(golden);
    if (path === '/api/v1/triggers/trg_project/stats')
      return statisticsFail
        ? reply({ status: 503, title: 'Unavailable' }, 503)
        : reply({
            totals: { launches_total: 12, agents_ready_total: 9 },
            placements: [
              { placement_id: 'plc_1', launches_total: 7, agents_ready_total: 5 },
              { placement_id: 'plc_2', launches_total: 5, agents_ready_total: 4 },
            ],
          });
    return reply({ status: 404, title: 'Unexpected proof request', code: 'not_found' }, 404);
  });
  const clickText = async (selector, text) => {
    await page.waitForFunction(
      (selector, text) =>
        [...document.querySelectorAll(selector)].some(
          (element) => element.textContent?.trim() === text && !element.disabled,
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
  };
  const checkLayout = async () => {
    await page.waitForFunction(() => {
      const frame = document.querySelector('.frame');
      if (!frame) return true;
      const tracks = window.getComputedStyle(frame).gridTemplateColumns.split(' ').map(parseFloat);
      return Math.abs(tracks.reduce((sum, width) => sum + width, 0) - frame.clientWidth) < 1;
    });
    const layout = await page.evaluate(() => ({
      width: innerWidth,
      scroll: document.documentElement.scrollWidth,
      overflow: [
        ...document.querySelectorAll(
          '.trigger-publisher__body, .trigger-publisher__editor, .trigger-field, .trigger-badge-preview, .trigger-code, .trigger-stats',
        ),
      ]
        .filter(
          (element) =>
            element.scrollWidth > element.clientWidth + 1 ||
            element.getBoundingClientRect().right > innerWidth + 1,
        )
        .map((element) => element.className),
      images: [...document.querySelectorAll('.trigger-publisher img')].map((element) => ({
        width: element.naturalWidth,
        height: element.naturalHeight,
      })),
    }));
    assert(layout.scroll <= layout.width + 1, JSON.stringify(layout));
    assert.deepEqual(layout.overflow, []);
    for (const image of layout.images)
      assert(image.width > 0 && image.height > 0, 'badge asset is nonblank');
  };
  const screenshot = async (name) => {
    await page.evaluate(() => {
      const scroll = document.querySelector('.trigger-publisher__body');
      if (scroll) scroll.scrollTop = 0;
    });
    await checkLayout();
    await page.screenshot({ path: join(output, `${name}.png`), fullPage: true });
  };
  const theme = (appearance) =>
    page.evaluate(async (appearance) => {
      const { setAppearance } = await import('/src/theme/appearance.ts');
      setAppearance(appearance);
    }, appearance);
  await page.setViewport({ width: 1440, height: 1000 });
  await page.goto(`${base}/triggers/manage`, { waitUntil: 'networkidle0' });
  await page.waitForSelector('#publisher-sandbox:not(:disabled)');
  for (const kind of ['sandbox', 'workspace', 'agent']) {
    const profile = profiles[kind];
    await page.waitForSelector(`#publisher-${kind}:not(:disabled)`);
    await page.select(`#publisher-${kind}`, `${profile.id}:${profile.revision}`);
  }
  await page.waitForFunction(() => document.body.textContent.includes('that reaches github.'));
  await theme('light');
  await screenshot('desktop-light-targets');
  await clickText('button', 'Next');
  await page.click('[aria-label="Agent reuse"] input[value="always_new"]');
  await clickText('button', 'Next');
  await page.type('#trigger-title', 'trureturing');
  await page.type('#trigger-description', 'Explore the project and prepare a reviewed change.');
  await page.type(
    '#trigger-first_message',
    'Inspect the project and summarize the next useful change.',
  );
  await page.waitForFunction(() =>
    [...document.querySelectorAll('.trigger-badge-options img')].every(
      (element) => element.complete && element.naturalWidth > 0,
    ),
  );
  await screenshot('desktop-light-presentation');
  await theme('dark');
  await screenshot('desktop-dark-presentation');
  await page.setViewport({ width: 390, height: 844 });
  await screenshot('mobile-dark-presentation');
  await theme('light');
  await screenshot('mobile-light-presentation');
  await clickText('button', 'Next');
  await page.type('#placement-label', 'README');
  await page.type('#placement-source', 'https://github.com/the-omega-institute/trureturing');
  await clickText('button', 'Add source URL');
  await page.waitForFunction(() => document.body.textContent.includes('plc_1'));
  await page.type('#placement-label', 'Website');
  await page.type('#placement-source', 'https://example.test/project');
  await clickText('button', 'Add source URL');
  await page.waitForFunction(() => document.body.textContent.includes('plc_2'));
  await clickText('button', 'Next');
  await screenshot('mobile-light-publish-confirmation');
  await page.click('.trigger-review input[type="checkbox"]');
  await clickText('button', 'Publish revision 1');
  await page.waitForSelector('.trigger-tabs');
  assert.equal(new URL(page.url()).pathname, '/triggers/manage/trg_project/embed');
  await page.waitForFunction(() =>
    [...document.querySelectorAll('.trigger-badge-preview img')].every(
      (element) => element.complete && element.naturalWidth > 0,
    ),
  );
  assert.equal(
    await page.$eval('[aria-label="README or Markdown snippet"]', (element) => element.textContent),
    golden.snippets.markdown,
  );
  for (const appearance of ['light', 'dark']) {
    await theme(appearance);
    await page.setViewport({ width: 1440, height: 1000 });
    await screenshot(`desktop-${appearance}-embed`);
    await page.setViewport({ width: 390, height: 844 });
    await screenshot(`mobile-${appearance}-embed`);
  }
  await clickText('[role="tab"]', 'Web page');
  assert.equal(
    await page.$eval('[aria-label="Web page snippet"]', (element) => element.textContent),
    golden.snippets.html,
  );
  await clickText('[role="tab"]', 'Page with JavaScript');
  assert.equal(
    await page.$eval('[aria-label="Subresource integrity"]', (element) => element.textContent),
    golden.javascript_sri,
  );
  assert.equal(
    await page.$eval('[aria-label="Image CSP directive"]', (element) => element.textContent),
    golden.content_security_policy.img_src,
  );
  assert.equal(
    await page.$eval('[aria-label="Script CSP directive"]', (element) => element.textContent),
    golden.content_security_policy.script_src,
  );
  await page.setViewport({ width: 320, height: 740 });
  await screenshot('compact-dark-javascript');
  await page.select('#embed-placement', 'plc_2');
  await page.waitForFunction(() => document.querySelector('.trigger-tabs') !== null);
  assert(requests.some((request) => request.endsWith('/embed?placement_id=plc_2')));
  statisticsFail = true;
  await page.reload({ waitUntil: 'networkidle0' });
  await page.waitForFunction(() =>
    document.querySelector('.trigger-stats')?.textContent.includes('Unavailable'),
  );
  assert(
    !(await page.$eval('.trigger-stats', (element) => element.textContent.includes('Loading'))),
  );
  assert.equal(writes.filter((write) => write.path === '/api/v1/triggers').length, 1);
  assert.equal(writes.filter((write) => write.path.endsWith('/publish')).length, 1);
  assert.equal(writes.filter((write) => write.path.endsWith('/placements')).length, 2);
  for (const write of writes) assert(typeof write.key === 'string' && write.key.length > 0);
  assert.deepEqual(stored.draft.target, {
    sandbox_profile: { id: 'sbp_standard', revision: 2 },
    workspace_profile: { id: 'wkp_trureturing', revision: 3 },
    agent_profile: { id: 'agp_codex', revision: 4 },
  });
  assert.deepEqual(errors, []);
  await writeFile(
    join(output, 'report.json'),
    JSON.stringify(
      {
        screenshots: 10,
        requests,
        writes,
        errors,
        viewports: ['1440x1000', '390x844', '320x740'],
        checks: [
          'integrated authenticated routes',
          'five-step publication',
          'immutable targets',
          'real server badge assets',
          'light and dark screenshots',
          'no overflow',
          'verbatim README, HTML, SRI and CSP',
          'placement switching',
          'unavailable statistics',
          'idempotent writes',
        ],
      },
      null,
      2,
    ),
  );
  console.log(
    JSON.stringify(
      { output, screenshots: 10, requests: requests.length, writes: writes.length, errors },
      null,
      2,
    ),
  );
} finally {
  await browser.close();
}
