// One separate owned-Agent browser page. No prompts, reloads, focus repair,
// clicks, transport changes, or writes to the active continuation's evidence.
import { createRequire } from 'node:module';
import { loadOwnedConfig } from './cma-bridge-continuity-owned-config.mjs';
import { constants } from 'node:fs';
import { open, readFile, writeFile, mkdir, chmod } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { installReadOnlyObserver } from './cma-bridge-continuity-browser-guard.mjs';
import { scope, classify, compareUi, networkAssessment, hash } from './cma-bridge-continuity-browser-contract.mjs';

const [configPath, change, beforeCommit] = process.argv.slice(2);
const check = (condition, code) => { if (!condition) throw new Error(code); };
const json = async (path) => JSON.parse(await readFile(path, 'utf8'));
check(/^[a-z0-9][a-z0-9-]{0,47}$/.test(change ?? '') && /^[a-f0-9]{40}$/.test(beforeCommit ?? ''), 'explicit_change_and_full_commit_required');
const config = await loadOwnedConfig(configPath);
const root = config.outputDirectory;
const [original, originalResult] = await Promise.all([json(config.fixtureJournal), json(config.fixtureResult)]);
const owned = scope(config, original, originalResult, change);
const { chromium } = createRequire(new URL('../../web/package.json', import.meta.url))('playwright');
const here = join(root, 'browser');
await mkdir(here, { mode: 0o700, recursive: true });
const directory = join(here, change);
await mkdir(directory, { mode: 0o700 }); // Existing evidence is never reused.
const requestIds = new WeakMap();
const requests = [];
const blocked = [];
const samples = [];
let browser, page, lines, timer, expiry, token, baseline, armed, latest, lineage;
let navigationCount = 0, allowedMutations = 0, droppedRequests = 0, skippedSamples = 0;
let sampling = false, stopping = false, verified = false, serial = 0;
let lineageFailure;
const tasks = new Set();
const pending = (promise) => { tasks.add(promise); void promise.then(() => tasks.delete(promise), () => tasks.delete(promise)); };
async function save(name, value) {
  const text = JSON.stringify(value, null, 2) + '\n';
  check(!token || !text.includes(token), 'credential_in_browser_evidence');
  await writeFile(join(directory, name), text, { mode: 0o600 });
  await chmod(join(directory, name), 0o600);
}
async function network() {
  const instrumentation = page && !page.isClosed()
    ? await page.evaluate(() => window.__cmaReadOnlyObserver ?? null) : null;
  await save('network.json', { requests, blocked, allowedMutations, droppedRequests, instrumentation });
  return instrumentation;
}
async function snapshot() {
  check(page && !page.isClosed(), 'owned_browser_page_closed');
  const value = await page.evaluate(({ finalMarker, activeMarker }) => ({
    path: location.pathname, timeOrigin: performance.timeOrigin,
    navigationEntries: performance.getEntriesByType('navigation').length,
    documentId: window.__cmaReadOnlyObserver?.documentId,
    messages: [...document.querySelectorAll('.message-stream .message')].map((element) => ({
      role: element.getAttribute('aria-label'), text: element.innerText,
    })),
    userInputs: [...document.querySelectorAll('.message-stream .message--user p')].map((element) => element.textContent),
    finalMarkerVisible: [...document.querySelectorAll('.message-stream .message--assistant:not([data-streaming])')]
      .some((element) => element.innerText.includes(finalMarker)),
    activeMarkerPresent: document.querySelector('.message-stream')?.innerText.includes(activeMarker) ?? false,
    thinking: !!document.querySelector('.thinking'),
    alerts: [...document.querySelectorAll('[role=alert]')].map((element) => element.innerText),
    scripts: [...document.scripts].map((script) => new URL(script.src || location.href).pathname),
    attemptedMutations: window.__cmaReadOnlyObserver?.blocked.length ?? 0,
    instrumentationDropped: window.__cmaReadOnlyObserver?.dropped ?? 0,
  }), owned);
  check(!JSON.stringify(value).includes(token), 'credential_visible_in_owned_page');
  latest = { at: new Date().toISOString(), agentId: owned.agentId, path: value.path, closed: false,
    timeOrigin: value.timeOrigin, documentId: value.documentId, navigationCount,
    navigationEntries: value.navigationEntries, loadedScripts: value.scripts,
    messages: value.messages.map(({ role, text }) => ({ role, sha256: hash(text), characters: text.length })),
    userInputHashes: value.userInputs.map((text) => hash(text ?? '')),
    finalMarkerVisible: value.finalMarkerVisible, activeMarkerPresent: value.activeMarkerPresent,
    thinking: value.thinking, alertHashes: value.alerts.map(hash), alertCount: value.alerts.length,
    allowedMutations, blockedMutations: blocked.length + value.attemptedMutations,
    instrumentationDropped: value.instrumentationDropped, requestCount: requests.length };
  return latest;
}
async function shot(name) {
  await page.screenshot({ path: join(directory, name + '.png'), fullPage: true, timeout: 20_000 });
  await chmod(join(directory, name + '.png'), 0o600);
}
async function activeReceipt(requireCompletion = false) {
  const journal = await json(join(root, 'continuations', change, 'journal.json'));
  check(journal.run === owned.run && journal.change === change && journal.beforeCommit === beforeCommit
    && JSON.stringify(journal.resources) === JSON.stringify(owned.resources), 'active_receipt_scope_changed');
  check(journal.active?.spec?.final === owned.finalMarker && journal.active?.armed?.proof?.phase === 'active', 'active_prerollout_receipt_required');
  if (requireCompletion) check(journal.result?.status === 'passed' && journal.result.mode === 'active_owned_tool_turn', 'active_same_response_completion_required');
  return journal;
}

try {
  const handle = await open(config.tokenFile, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await handle.stat();
    check(stat.isFile() && stat.uid === process.getuid() && !(stat.mode & 0o077) && stat.size < 16384, 'private_token_required');
    token = (await handle.readFile('utf8')).trim();
  } finally { await handle.close(); }
  check(token && !/\s/.test(token), 'invalid_token');
  const sessionResponse = await fetch(owned.origin + '/auth/session', { redirect: 'error', signal: AbortSignal.timeout(30000),
    headers: { Authorization: `Bearer ${token}`, 'User-Agent': 'Mozilla/5.0' } });
  check(sessionResponse.ok, 'saved_token_session_unavailable');
  const session = await sessionResponse.json();
  check(session.authenticated === true && session.visitor === false && session.subject === owned.owner, 'saved_token_owner_changed');
  browser = await chromium.launch({ headless: true, ...(config.browserExecutable ? { executablePath: config.browserExecutable } : {}) });
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, serviceWorkers: 'block' });
  await context.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const method = request.method().toUpperCase();
    if (!['GET', 'HEAD', 'OPTIONS'].includes(method)
      || (url.origin === owned.origin && url.pathname.startsWith('/auth/') && url.pathname !== '/auth/session')) {
      blocked.push({ method, origin: url.origin, path: url.pathname });
      await route.abort('blockedbyclient'); return;
    }
    if (request.headers().authorization && url.origin !== owned.origin) { await route.abort('blockedbyclient'); return; }
    await route.continue();
  });
  await context.routeWebSocket('**/*', (socket) => socket.close());
  await context.addInitScript(installReadOnlyObserver, { exactOrigin: owned.origin, credential: token,
    documentId: randomBytes(16).toString('hex'), agentId: owned.agentId });
  page = await context.newPage();
  page.on('framenavigated', (frame) => { if (frame === page.mainFrame()) navigationCount++; });
  page.on('request', (request) => {
    if (requests.length >= 5000) { droppedRequests++; return; }
    const value = { id: ++serial, startedAt: new Date().toISOString(), method: request.method(), ...classify(request.url(), owned) };
    requests.push(value); requestIds.set(request, value);
  });
  page.on('requestfinished', (request) => { const value = requestIds.get(request); if (value) value.finishedAt = new Date().toISOString(); });
  page.on('requestfailed', (request) => {
    const value = requestIds.get(request);
    if (value) { value.failedAt = new Date().toISOString(); value.failure = request.failure()?.errorText?.match(/^net::ERR_[A-Z_]+$/)?.[0] ?? 'network_failure'; }
  });
  page.on('response', (response) => {
    const request = response.request();
    const value = requestIds.get(request);
    if (value) Object.assign(value, { status: response.status(), headersAt: new Date().toISOString(),
      sse: /^text\/event-stream(?:;|$)/i.test(response.headers()['content-type'] ?? '') });
    if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method())) allowedMutations++;
    if (response.url() === `${owned.origin}/api/v1/agents/${owned.agentId}` && response.ok()) {
      pending(response.json().then((agent) => {
        check(agent.id === owned.agentId && agent.sandbox.id === owned.resources.sandboxId
          && agent.workspace.id === owned.resources.workspaceId, 'owned_browser_lineage_changed');
        const next = { agentId: agent.id, sandboxId: agent.sandbox.id, workspaceId: agent.workspace.id, worktreeId: agent.worktree.id };
        check(!lineage || JSON.stringify(lineage) === JSON.stringify(next), 'owned_browser_lineage_changed');
        lineage = next;
      }).catch((error) => { lineageFailure = /^[a-z0-9_]+$/.test(error.message) ? error.message : 'owned_lineage_read_failed'; }));
    }
  });
  await page.goto(`${owned.origin}/agents/${owned.agentId}`, { waitUntil: 'domcontentloaded', timeout: 45000 });
  await page.waitForFunction((marker) => [...document.querySelectorAll('.message-stream .message--assistant')]
    .some((element) => element.innerText.includes(marker)), owned.originalMarker, { timeout: 45000 });
  await Promise.allSettled([...tasks]);
  baseline = await snapshot();
  check(lineage && !lineageFailure && !baseline.thinking && !baseline.finalMarkerVisible
    && baseline.allowedMutations === 0 && baseline.blockedMutations === 0, 'quiet_owned_browser_baseline_required');
  await save('before.json', { beforeCommit, owned, lineage, snapshot: baseline });
  await shot('before');
  await network();
  await save('hold.json', { status: 'holding_owned_active_browser', pid: process.pid, change, agentId: owned.agentId,
    beforeCommit, openedAt: baseline.at, maximumDurationSeconds: 2700 });
  console.log(JSON.stringify({ status: 'holding_owned_active_browser', agentId: owned.agentId, change, beforeCommit }));
  timer = setInterval(() => {
    if (sampling || stopping) { skippedSamples++; return; }
    sampling = true;
    pending(snapshot().then(async (value) => {
      if (samples.length < 1000) samples.push(value);
      await save('live.json', value);
      if (samples.length % 5 === 0) { await save('samples.json', { samples, skippedSamples }); await network(); }
      if (samples.length % 7 === 0) console.log(JSON.stringify({ status: 'owned_page_observing', change,
        finalMarkerVisible: value.finalMarkerVisible, thinking: value.thinking, requests: requests.length }));
    }).catch(async () => { await save('observation-gap.json', { at: new Date().toISOString(), code: 'owned_page_sample_unavailable' }); })
      .finally(() => { sampling = false; }));
  }, 3000);
  lines = createInterface({ input: process.stdin });
  expiry = setTimeout(() => { stopping = true; lines.close(); }, 2700000);
  for await (const line of lines) {
    let instruction;
    try { instruction = JSON.parse(line); } catch { console.log(JSON.stringify({ status: 'invalid_local_instruction' })); continue; }
    if (instruction.action === 'close') break;
    if (instruction.action === 'status') {
      console.log(JSON.stringify({ status: 'owned_page_observing', change, finalMarkerVisible: latest.finalMarkerVisible, requests: requests.length })); continue;
    }
    check(['arm', 'verify'].includes(instruction.action), 'known_local_action_required');
    const active = await activeReceipt(instruction.action === 'verify');
    if (instruction.action === 'arm') {
      check(!armed, 'owned_browser_already_armed');
      const value = await snapshot();
      const continuity = compareUi(baseline, value, owned);
      check(continuity.sameDocument && continuity.historyPreserved && continuity.sameAgent && continuity.noMutation
        && value.thinking && !value.finalMarkerVisible && value.userInputHashes.includes(hash(active.active.spec.prompt)), 'owned_active_turn_not_visible');
      armed = { at: value.at, responseId: active.active.armed.proof.responseId,
        commandId: active.active.armed.proof.commandId, turnSequence: active.active.armed.proof.turnSequence, snapshot: value };
      await save('armed.json', armed); await shot('armed'); await network();
      console.log(JSON.stringify({ status: 'owned_browser_armed', change, at: armed.at, responseId: armed.responseId }));
      continue;
    }
    check(armed && /^[a-f0-9]{40}$/.test(instruction.commit ?? '') && active.afterCommit === instruction.commit, 'matching_completed_rollout_required');
    check(active.result.responseId === armed.responseId && active.result.commandId === armed.commandId
      && active.result.turnSequence === armed.turnSequence, 'active_browser_response_changed');
    // Passive DOM waiting. No focus, reload, click, route change or GET refresh.
    try {
      await page.waitForFunction((marker) => [...document.querySelectorAll('.message-stream .message--assistant:not([data-streaming])')]
        .some((element) => element.innerText.includes(marker)), owned.finalMarker, { timeout: 90000 });
    } catch { /* Retain a failed observation; never repair the page. */ }
    const after = await snapshot();
    const instrumentation = await network();
    const ui = compareUi(baseline, after, owned);
    const transport = networkAssessment(requests, instrumentation?.events ?? [], armed.at, after.at);
    const instrumentationGaps = instrumentation?.events.filter((value) => ['stream_instrumentation_gap', 'frame_observation_limit'].includes(value.type)).length ?? 0;
    const completeEvidence = !!instrumentation && !droppedRequests && !instrumentation.dropped && !lineageFailure && !instrumentationGaps;
    const result = { status: ui.status === 'failed' || !completeEvidence ? 'failed'
      : transport.reconnectEvidence === 'observed' ? 'passed' : 'partial',
    change, beforeCommit, afterCommit: instruction.commit, agentId: owned.agentId, lineage,
    responseId: armed.responseId, commandId: armed.commandId, turnSequence: armed.turnSequence,
    armedAt: armed.at, at: after.at, ui, transport, completeEvidence, instrumentationGaps,
    scriptsPreserved: JSON.stringify(baseline.loadedScripts) === JSON.stringify(after.loadedScripts),
    allowedMutations, blockedMutations: after.blockedMutations,
    limitations: ['Root must correlate the browser armed/connection times with actual rollout receipts; SHAs are assertions.',
      'The page is an isolated saved-bearer session; cookie/OAuth renewal is not covered.',
      'Stream reopen proves automatic observation recovery on this page, not uninterrupted availability or exactly-once process execution.',
      'The same-response/native-runtime proof is linked from the active-turn result; DOM alone does not expose native identity.',
      'If only GETs or no closure/reopen were observed, reconnect evidence remains not observed.'] };
    await save('after.json', after); await shot('after'); await save('result.json', result);
    verified = true;
    console.log(JSON.stringify({ status: result.status, change, ui: ui.status,
      reconnectEvidence: transport.reconnectEvidence, transports: transport.observedTransports }));
  }
} catch (error) {
  const code = /^[a-z0-9_]+$/.test(error.message) ? error.message : 'owned_browser_observation_unavailable';
  await save('failure.json', { status: 'blocked', code, allowedMutations, blockedMutations: blocked.length });
  console.error(JSON.stringify({ status: 'blocked', code })); process.exitCode = 1;
} finally {
  stopping = true; clearInterval(timer); clearTimeout(expiry); lines?.close();
  await Promise.allSettled([...tasks]);
  await save('samples.json', { samples, skippedSamples });
  await network().catch(() => undefined);
  if (!verified) await save('incomplete.json', { status: 'incomplete', at: new Date().toISOString(), armed: !!armed });
  if (browser) await browser.close();
}
