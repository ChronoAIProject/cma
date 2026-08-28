/** A real-browser CMA Trigger journey against a deterministic control-plane fake. */
import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { open, ready, settle } from './browser.mjs';

const PORT = 4321;
const FIXTURES = new URL('../../sdk/typescript/tests/fixtures/', import.meta.url);
const fixture = (name) => JSON.parse(readFileSync(new URL(name, FIXTURES), 'utf8'));
const DRAFT = fixture('launch.json');
const PREVIEW = fixture('trigger_preview.json');
const DIFF = fixture('diff_review.json');
const PR_DRAFT = fixture('pull_request_draft.json');
const PROGRESS = fixture('launch_progress.json');
const SESSION_ID = 'sesn_trigger_browser';
const BRANCH_NAME = `cma/${DRAFT.id}`;
const COMMAND_DATABASE_NAME = 'cma-trigger-command-ledger';
const COMMAND_DATABASE_STORE = 'owner-envelopes';
const COMMAND_STORAGE_PREFIX = 'cma:trigger-command-ledger:v1:';

const FORK_REPOSITORY = {
  ...DRAFT.revision_content.upstream.repository,
  repository_id: 11,
  repository_node_id: 'R_contributor_fork',
  owner_id: 21,
  owner_node_id: 'U_contributor',
  canonical_owner: 'contributor',
};

function launchAt(state, completedThrough, runningAt) {
  return {
    ...DRAFT,
    state,
    revision_number: state === 'ready' ? 10 : 1,
    steps: DRAFT.steps.map((step) => ({
      ...step,
      status:
        step.sequence <= completedThrough
          ? 'completed'
          : step.sequence === runningAt
            ? 'running'
            : 'pending',
      started_at:
        step.sequence <= completedThrough || step.sequence === runningAt
          ? '2026-08-27T00:00:01Z'
          : null,
      completed_at: step.sequence <= completedThrough ? '2026-08-27T00:00:10Z' : null,
      attempt: step.sequence <= completedThrough || step.sequence === runningAt ? 1 : 0,
      safe_detail: step.sequence === runningAt ? 'CMA is reconciling this exact durable step.' : '',
    })),
    started_at: '2026-08-27T00:00:01Z',
    updated_at: '2026-08-27T00:00:10Z',
  };
}

function progressingLaunch() {
  return launchAt('fork_pending', 3, 4);
}

function readyLaunch() {
  return {
    ...launchAt('ready', 11, -1),
    fork: {
      repository: FORK_REPOSITORY,
      reused: false,
      source_revision: DRAFT.source_sha,
      canonical_url: 'https://github.com/contributor/project',
      ready_at: '2026-08-27T00:00:04Z',
    },
    branch: {
      name: BRANCH_NAME,
      expected_head: DRAFT.source_sha,
      current_head: DRAFT.source_sha,
      canonical_url: `https://github.com/contributor/project/tree/${BRANCH_NAME}`,
      created_at: '2026-08-27T00:00:05Z',
    },
    workspace: {
      latest_checkpoint: { id: '55555555-5555-4555-8555-555555555555' },
    },
    session: { session_id: SESSION_ID },
    reach: {
      service_id: DRAFT.connection.service_id,
      state: 'active',
      policy_generation: 1,
      policy_fence: 1,
      runtime_audience: 'runtime-fixture',
      expires_at_epoch_seconds: 1_800_000_000,
      numeric_binding_observed_at: '2026-08-27T00:00:06Z',
      refreshed_at: '2026-08-27T00:00:06Z',
      tombstone_acknowledged: false,
      agent_key_revocation_confirmed: false,
    },
    delivery: {
      launch_id: DRAFT.id,
      session_id: SESSION_ID,
      revision_digest: DRAFT.revision_digest,
      origin_key: `cma-trigger:${DRAFT.id}:${DRAFT.revision_digest}:initial`,
      payload_hash: 'a'.repeat(64),
      state: 'completed',
      attempt: 1,
      recovery_generation: 0,
      dispatch_fence_sequence: 42,
      queued_at: '2026-08-27T00:00:06Z',
      updated_at: '2026-08-27T00:00:09Z',
    },
    contribution_checkpoint_version: 1,
  };
}

function openPullRequest() {
  return {
    ...PR_DRAFT,
    provider_id: 101,
    provider_node_id: 'PR_provider_node',
    number: 17,
    canonical_url: 'https://github.com/upstream-owner/project/pull/17',
    state: 'open',
    base_revision: PR_DRAFT.expected_base_revision,
    head_revision: PR_DRAFT.expected_head_revision,
    observed_title: PR_DRAFT.title,
    observed_body_digest: PR_DRAFT.body_digest,
    mergeability: 'computing',
    checks: 'pending',
    status_failure: null,
    provider_request_id: null,
    head_matches_launch: true,
    confirmation_snapshot_digest: PR_DRAFT.intent_digest,
    confirmed_at: '2026-08-27T00:00:15Z',
    created_at: '2026-08-27T00:00:15Z',
    updated_at: '2026-08-27T00:00:15Z',
    provider_created_at: '2026-08-27T00:00:15Z',
    provider_updated_at: '2026-08-27T00:00:15Z',
  };
}

function browserState() {
  return {
    authenticated: false,
    launch: DRAFT,
    review: {
      ...DIFF,
      revision_number: 10,
      state: 'ready',
      artifact: null,
      contribution: null,
      pull_request: null,
    },
    startRequests: [],
    startTransitions: 0,
    captureWrites: 0,
    publicationWrites: 0,
    proposalReads: 0,
    pullRequestWrites: 0,
  };
}

function respond(request, body, status = 200, headers = {}) {
  return request.respond({
    status,
    headers,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

async function commandLedgerSnapshot(page) {
  return page.evaluate(
    async ({ databaseName, storeName, storagePrefix }) => {
      const local = Object.fromEntries(
        Object.entries(localStorage).filter(([key]) => key.startsWith(storagePrefix)),
      );
      const databases = await indexedDB.databases();
      if (!databases.some(({ name }) => name === databaseName)) {
        throw new Error('the durable command ledger database was not created');
      }
      const indexed = await new Promise((resolve, reject) => {
        const opening = indexedDB.open(databaseName);
        opening.onerror = () => reject(new Error('the durable command ledger could not be opened'));
        opening.onsuccess = () => {
          const database = opening.result;
          if (!database.objectStoreNames.contains(storeName)) {
            database.close();
            reject(new Error('the durable command ledger store is missing'));
            return;
          }
          const transaction = database.transaction(storeName, 'readonly');
          const keys = transaction.objectStore(storeName).getAllKeys();
          const values = transaction.objectStore(storeName).getAll();
          transaction.oncomplete = () => {
            database.close();
            resolve(
              Object.fromEntries(
                keys.result.map((key, index) => [String(key), String(values.result[index])]),
              ),
            );
          };
          transaction.onerror = () => {
            database.close();
            reject(new Error('the durable command ledger could not be read'));
          };
          transaction.onabort = transaction.onerror;
        };
      });
      return { local, indexed };
    },
    {
      databaseName: COMMAND_DATABASE_NAME,
      storeName: COMMAND_DATABASE_STORE,
      storagePrefix: COMMAND_STORAGE_PREFIX,
    },
  );
}

async function stub(page, state) {
  await page.setRequestInterception(true);
  page.on('request', (request) => {
    void (async () => {
      const url = new URL(request.url());
      const path = url.pathname;
      const method = request.method();
      const launchPath = `/api/v1/launches/${DRAFT.id}`;

      if (path === '/auth/session') {
        if (!state.authenticated) return respond(request, { authenticated: false }, 401);
        return respond(request, {
          authenticated: true,
          subject: 'trigger-browser-owner',
          display_name: 'CMA Contributor',
          email: 'contributor@example.test',
          capabilities: ['create_session', 'use_own_sessions'],
        });
      }
      if (path === '/auth/login') {
        state.authenticated = true;
        const location = url.searchParams.get('return_to') ?? '/';
        return request.respond({ status: 302, headers: { location }, body: '' });
      }
      if (path === `/api/v1/triggers/public/${PREVIEW.public_id}` && method === 'GET') {
        return respond(request, PREVIEW);
      }
      if (path === `/api/v1/triggers/public/${PREVIEW.public_id}/launches` && method === 'POST') {
        return respond(request, state.launch, 201);
      }
      if (path === '/api/v1/trigger-connections' && method === 'GET') {
        return respond(request, { connections: [DRAFT.connection] });
      }
      if (
        path === `/api/v1/trigger-connections/${DRAFT.connection.service_id}/preflight` &&
        method === 'GET'
      ) {
        return respond(request, DRAFT.connection);
      }
      if (path === `${launchPath}/start` && method === 'POST') {
        state.startRequests.push({
          key: request.headers()['idempotency-key'],
          body: JSON.parse(request.postData() ?? '{}'),
        });
        if (state.launch.state === 'draft') {
          state.startTransitions += 1;
          state.launch = progressingLaunch();
        }
        return respond(request, state.launch, 202);
      }
      if (path === `${launchPath}/events` && method === 'GET') {
        const after = Number(url.searchParams.get('after') ?? '0');
        return respond(
          request,
          after === 0 ? PROGRESS : { launch_id: DRAFT.id, events: [], next_cursor: after },
        );
      }
      if (path === launchPath && method === 'GET') return respond(request, state.launch);
      if (path === `${launchPath}/diff` && method === 'GET') {
        return respond(request, state.review);
      }
      if (path === `${launchPath}/contributions/review` && method === 'POST') {
        state.captureWrites += 1;
        state.review = {
          ...DIFF,
          revision_number: 11,
          state: 'contributing',
          contribution: null,
          pull_request: null,
        };
        state.launch = {
          ...state.launch,
          state: 'contributing',
          revision_number: 11,
          diff: state.review.artifact,
        };
        return respond(request, state.launch, 202);
      }
      if (path === `${launchPath}/contributions/publish` && method === 'POST') {
        state.publicationWrites += 1;
        state.review = { ...DIFF, revision_number: 12, state: 'pr_pending', pull_request: null };
        state.launch = {
          ...state.launch,
          state: 'pr_pending',
          revision_number: 12,
          diff: state.review.artifact,
          contribution: state.review.contribution,
        };
        return respond(request, state.launch, 202);
      }
      if (path === `${launchPath}/pull-request/review` && method === 'POST') {
        state.proposalReads += 1;
        return respond(request, PR_DRAFT);
      }
      if (path === `${launchPath}/pull-request/open` && method === 'POST') {
        state.pullRequestWrites += 1;
        const pullRequest = openPullRequest();
        state.review = {
          ...state.review,
          revision_number: 13,
          state: 'pr_open',
          pull_request: pullRequest,
        };
        state.launch = {
          ...state.launch,
          state: 'pr_open',
          revision_number: 13,
          pull_request: pullRequest,
        };
        return respond(request, state.launch, 202);
      }
      if (path === '/api/v1/sessions' && method === 'GET') {
        return respond(request, { sessions: [] });
      }
      if (path === '/api/v1/kernels' && method === 'GET') {
        return respond(request, { kernels: [{ id: 'codex', modes: ['conversation'] }] });
      }
      if (path === '/api/v1/environment-profiles' && method === 'GET') {
        return respond(request, []);
      }
      if (path.startsWith('/api/')) {
        return respond(
          request,
          { type: 'about:blank', title: 'Not found', status: 404, code: 'not_found' },
          404,
        );
      }
      return request.continue();
    })().catch(() => {
      request.abort().catch(() => {});
    });
  });
}

async function exactControl(page, selector, text, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      const clicked = await page.evaluate(
        (wantedSelector, wantedText) => {
          const element = [...document.querySelectorAll(wantedSelector)].find(
            (candidate) => candidate.textContent?.trim() === wantedText,
          );
          if (
            !(element instanceof HTMLElement) ||
            (element instanceof HTMLButtonElement && element.disabled)
          ) {
            return false;
          }
          element.click();
          return true;
        },
        selector,
        text,
      );
      if (clicked) return true;
    } catch {
      // A route transition can replace the execution context between polls.
    }
    await settle(50);
  }
  return false;
}

async function dialogControl(page, title, text, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      const clicked = await page.evaluate(
        (wantedTitle, wantedText) => {
          const dialog = [...document.querySelectorAll('[role="dialog"]')].find((candidate) =>
            candidate.textContent?.includes(wantedTitle),
          );
          const button = [...(dialog?.querySelectorAll('button') ?? [])].find(
            (candidate) => candidate.textContent?.trim() === wantedText,
          );
          if (!(button instanceof HTMLButtonElement) || button.disabled) return false;
          button.click();
          return true;
        },
        title,
        text,
      );
      if (clicked) return true;
    } catch {
      // A route transition can replace the execution context between polls.
    }
    await settle(50);
  }
  return false;
}

async function textAppears(page, text, timeout = 15_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      const found = await page.evaluate(
        (wanted) => document.body.textContent?.includes(wanted) === true,
        text,
      );
      if (found) return;
    } catch {
      // A route reload can replace the execution context between polls.
    }
    await settle(50);
  }
  const visible = await page.evaluate(
    () => document.querySelector('main')?.textContent?.trim().slice(0, 2_000) ?? '',
  );
  throw new Error(
    `waited ${String(timeout)}ms for ${JSON.stringify(text)} at ${page.url()}: ${visible}`,
  );
}

async function pathAppears(page, path, timeout = 15_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (new URL(page.url()).pathname === path) return;
    await settle(50);
  }
  throw new Error(`waited ${String(timeout)}ms for browser path ${path}, found ${page.url()}`);
}

async function assertLayout(page, label) {
  const measured = await page.evaluate(() => {
    const visible = (element) => {
      const box = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      return box.width > 0 && box.height > 0 && style.visibility !== 'hidden';
    };
    const controls = [...document.querySelectorAll('button, input, textarea, a')]
      .filter(visible)
      .map((element) => {
        const box = element.getBoundingClientRect();
        return {
          text: element.textContent?.trim().slice(0, 60) ?? element.getAttribute('aria-label'),
          left: box.left,
          right: box.right,
          clientWidth: element.clientWidth,
          scrollWidth: element.scrollWidth,
        };
      });
    const box = (selector) => {
      const element = document.querySelector(selector);
      if (!(element instanceof HTMLElement)) return null;
      const bounds = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      return {
        left: bounds.left,
        right: bounds.right,
        width: bounds.width,
        display: style.display,
        flex: style.flex,
        gridTemplateColumns: style.gridTemplateColumns,
      };
    };
    return {
      viewport: window.innerWidth,
      documentWidth: document.documentElement.scrollWidth,
      h1Count: document.querySelectorAll('h1').length,
      controls,
      frame: box('.frame'),
      center: box('.frame__center'),
      workflow: box('.trigger-workflow'),
      launchRoute: box('.launch-route'),
      stylesheets: [...document.styleSheets].map((sheet) => sheet.href),
    };
  });
  const bad = measured.controls.filter(
    (control) =>
      control.left < -1 ||
      control.right > measured.viewport + 1 ||
      control.scrollWidth > control.clientWidth + 2,
  );
  if (measured.documentWidth > measured.viewport + 1 || measured.h1Count !== 1 || bad.length > 0) {
    throw new Error(`${label} layout is not bounded: ${JSON.stringify({ measured, bad })}`);
  }
}

async function waitForBoundedLayout(page) {
  await page.waitForFunction(
    () => {
      if (
        document.documentElement.scrollWidth > window.innerWidth + 1 ||
        document.querySelectorAll('h1').length !== 1
      ) {
        return false;
      }
      return [...document.querySelectorAll('button, input, textarea, a')].every((element) => {
        const bounds = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        if (bounds.width <= 0 || bounds.height <= 0 || style.visibility === 'hidden') return true;
        return (
          bounds.left >= -1 &&
          bounds.right <= window.innerWidth + 1 &&
          element.scrollWidth <= element.clientWidth + 2
        );
      });
    },
    { polling: 'raf', timeout: 5_000 },
  );
}

async function screenshot(page, name) {
  const directory = process.env.CMA_TRIGGER_SCREENSHOT_DIR;
  if (directory === undefined || directory === '') return;
  mkdirSync(directory, { recursive: true });
  await page.screenshot({ path: join(directory, `${name}.png`), fullPage: true });
}

async function openTrigger(page) {
  await page.goto(`http://localhost:${String(PORT)}/triggers/${PREVIEW.public_id}`, {
    waitUntil: 'domcontentloaded',
  });
}

async function main() {
  const state = browserState();
  const { page, close } = await open({ port: PORT });
  const failures = [];
  const observe = (target) => {
    target.on('pageerror', (error) => failures.push(error.message));
  };
  try {
    observe(page);
    await stub(page, state);
    await openTrigger(page);
    await ready(page, '.trigger-public', { label: 'the anonymous Trigger preview' });
    await textAppears(page, PREVIEW.public_summary);
    if ((await page.content()).includes(DRAFT.effective.rendered_prompt)) {
      throw new Error('the anonymous preview exposed the configured prompt');
    }
    await assertLayout(page, 'anonymous desktop preview');
    await screenshot(page, 'anonymous-desktop');
    await page.setViewport({ width: 390, height: 844 });
    await waitForBoundedLayout(page);
    await assertLayout(page, 'anonymous phone preview');
    await screenshot(page, 'anonymous-phone');
    await page.setViewport({ width: 1440, height: 900 });
    await waitForBoundedLayout(page);

    if (!(await exactControl(page, 'button', 'Sign in to review'))) {
      throw new Error('the anonymous preview offered no sign-in command');
    }
    await page.waitForNavigation({ waitUntil: 'domcontentloaded' });
    await textAppears(page, 'Start Launch');
    await settle(700);
    await assertLayout(page, 'authenticated review');

    const second = await page.browser().newPage();
    observe(second);
    await second.setViewport({ width: 1180, height: 800 });
    await stub(second, state);
    await openTrigger(second);
    await textAppears(second, 'Start Launch');

    await exactControl(page, 'button', 'Start Launch');
    await textAppears(page, 'Start this Launch?');
    const focusInsideDialog = await page.evaluate(
      () => document.activeElement?.closest('[role="dialog"]') !== null,
    );
    if (!focusInsideDialog) throw new Error('the Start dialog did not receive keyboard focus');
    await page.keyboard.press('Escape');
    await settle(150);
    if ((await page.content()).includes('Start this Launch?')) {
      throw new Error('Escape did not close the Start confirmation');
    }

    await exactControl(page, 'button', 'Start Launch');
    await exactControl(second, 'button', 'Start Launch');
    await Promise.all([
      dialogControl(page, 'Start this Launch?', 'Start Launch'),
      dialogControl(second, 'Start this Launch?', 'Start Launch'),
    ]);
    try {
      const launchPath = `/launches/${DRAFT.id}`;
      await Promise.all([pathAppears(page, launchPath), pathAppears(second, launchPath)]);
    } catch (cause) {
      const tabs = await Promise.all(
        [page, second].map((tab) =>
          tab.evaluate(() => ({
            path: location.pathname,
            text: document.body.textContent?.trim().slice(0, 1_000) ?? '',
            storage: { ...localStorage },
          })),
        ),
      );
      throw new Error(
        `two-tab Start did not converge: ${JSON.stringify({ tabs, startRequests: state.startRequests })}`,
        { cause },
      );
    }
    await textAppears(page, 'CMA is reconciling this exact durable step.');
    const startKeys = new Set(state.startRequests.map(({ key }) => key));
    if (
      state.startRequests.length < 1 ||
      state.startRequests.length > 2 ||
      startKeys.size !== 1 ||
      state.startTransitions !== 1 ||
      state.startRequests.some(({ body }) => body.snapshot_digest !== DRAFT.effective.digest)
    ) {
      const ledgers = await Promise.all([page, second].map(commandLedgerSnapshot));
      throw new Error(
        `two-tab Start did not retain one reviewed Launch, key, and orchestration: ${JSON.stringify({ requests: state.startRequests, transitions: state.startTransitions, ledgers })}`,
      );
    }
    const ledgerSnapshots = await Promise.all([page, second].map(commandLedgerSnapshot));
    const serializedLedgers = ledgerSnapshots.map(({ indexed }) => JSON.stringify(indexed));
    const indexedEnvelopes = Object.values(ledgerSnapshots[0].indexed);
    const retainedCommands = indexedEnvelopes
      .flatMap((value) => JSON.parse(value).commands ?? [])
      .filter(
        (command) =>
          command.slot === `launch:start:${DRAFT.id}` &&
          command.resource === `launch:${DRAFT.id}` &&
          command.action === 'start',
      );
    if (
      ledgerSnapshots.some(({ local }) => Object.keys(local).length !== 0) ||
      new Set(serializedLedgers).size !== 1 ||
      indexedEnvelopes.length !== 1 ||
      retainedCommands.length !== 1 ||
      retainedCommands[0].key !== state.startRequests[0].key ||
      !Number.isSafeInteger(retainedCommands[0].completedAt)
    ) {
      throw new Error(
        `two-tab Start did not converge in the transactional command ledger: ${JSON.stringify({ requests: state.startRequests, ledgerSnapshots })}`,
      );
    }
    await screenshot(page, 'progress-desktop');

    state.launch = readyLaunch();
    // Opening the second Start tab made it Chromium's foreground page. A
    // person following this path returns to the Launch before using it, so do
    // the same before reloading and asserting its resumed layout.
    await page.bringToFront();
    await page.reload({ waitUntil: 'domcontentloaded' });
    await textAppears(page, 'Open CMA session');
    await settle(700);
    await screenshot(page, 'ready-debug');
    await assertLayout(page, 'resumed ready Launch');
    if (!(await exactControl(page, 'a', 'Review contribution'))) {
      throw new Error('the ready Launch offered no contribution route');
    }
    await textAppears(page, 'Contribution review');
    const reviewClicked = await exactControl(page, 'button', 'Review diff');
    await settle(500);
    if (!reviewClicked || state.captureWrites !== 1) {
      const reviewControl = await page.evaluate(() => {
        const button = [...document.querySelectorAll('button')].find(
          (candidate) => candidate.textContent?.trim() === 'Review diff',
        );
        return button instanceof HTMLButtonElement
          ? { disabled: button.disabled, text: button.textContent }
          : null;
      });
      throw new Error(
        `diff review was not submitted: ${JSON.stringify({ reviewClicked, reviewControl, captureWrites: state.captureWrites, failures })}`,
      );
    }
    await textAppears(page, 'README.md');
    await assertLayout(page, 'reviewed contribution');
    await screenshot(page, 'contribution-desktop');

    await exactControl(page, 'button', 'Publish branch');
    await textAppears(page, DIFF.artifact.id);
    await dialogControl(page, 'Publish this reviewed branch?', 'Publish branch');
    await textAppears(page, 'Open pull request');
    await exactControl(page, 'button', 'Open pull request');
    await textAppears(page, 'Open pull request?');
    await dialogControl(page, 'Open pull request?', 'Open pull request');
    await textAppears(page, '#17 CMA contribution');

    // Keep the contribution surface foregrounded before changing its
    // viewport. AppFrame coalesces resize measurements with
    // requestAnimationFrame, which a background page may throttle.
    await page.bringToFront();
    await page.setViewport({ width: 390, height: 844 });
    await waitForBoundedLayout(page);
    await assertLayout(page, 'completed contribution on phone');
    await screenshot(page, 'contribution-phone');
    if (
      state.captureWrites !== 1 ||
      state.publicationWrites !== 1 ||
      state.proposalReads !== 2 ||
      state.pullRequestWrites !== 1
    ) {
      throw new Error(
        `the journey crossed a write boundary more than intended: ${JSON.stringify(state)}`,
      );
    }
    if (failures.length > 0) throw new Error(`browser errors: ${failures.join(' | ')}`);
    await second.close();
    console.log(
      'Trigger browser journey: anonymous preview, OAuth return, two-tab Start, progress resume, diff, branch, PR, keyboard, desktop and phone passed',
    );
  } finally {
    await close();
  }
}

await main();
