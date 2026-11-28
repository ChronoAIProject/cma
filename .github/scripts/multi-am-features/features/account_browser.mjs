import {check, id} from '../cma/core.mjs';

const timeout = 15000;
const account = page => page.getByRole('button', {name: /^Account menu for /});
const choice = (page, group, name, options = {}) => page.getByRole('group', {name: group, exact: true})
  .getByRole('menuitemradio', {name, exact: true, ...options});

async function browserSession(page, status, owner) {
  const observed = await page.evaluate(async timeoutMs => {
    const response = await fetch('/auth/session', {credentials: 'same-origin', redirect: 'error',
      cache: 'no-store', signal: AbortSignal.timeout(timeoutMs)});
    const {authenticated, subject} = await response.json();
    return {status: response.status, authenticated, subject};
  }, timeout);
  check(observed.status === status, 'browser_session_status_unexpected_' + observed.status);
  check(observed.authenticated === (status === 200) && (owner === undefined || observed.subject === owner),
    'browser_session_identity_unexpected');
}

export async function accountSignIn(page, owner) {
  await page.goto('/', {waitUntil: 'domcontentloaded', timeout});
  await browserSession(page, 401);
  const responseFor = (path, status) => page.waitForResponse(response => {
    const request = response.request();
    return request.isNavigationRequest() && request.method() === 'GET'
      && new URL(response.url()).pathname === path && response.status() === status;
  }, {timeout});
  await Promise.all([
    responseFor('/auth/login', 303),
    responseFor('/fixture/authorize', 302),
    responseFor('/auth/callback', 303),
    page.getByRole('button', {name: 'Get Started', exact: true}).click({timeout}),
  ]);
  await account(page).waitFor({timeout});
  await browserSession(page, 200, owner);
}

export async function accountSignOut(page, peerOrigin, publicHost) {
  const context = page.context();
  const cookie = (await context.cookies()).find(value => value.name === 'cma_session');
  check(cookie?.httpOnly && cookie.secure, 'browser_sign_out_session_missing');
  await account(page).click({timeout});
  await Promise.all([
    page.waitForRequest(request => new URL(request.url()).pathname === '/auth/logout'
      && request.method() === 'GET', {timeout}),
    page.getByRole('menuitem', {name: 'Sign out', exact: true}).click({timeout}),
  ]);
  await page.getByRole('button', {name: 'Get Started', exact: true}).waitFor({timeout});
  await browserSession(page, 401);
  check(!(await context.cookies()).some(value => value.name === 'cma_session'), 'browser_sign_out_cookie_retained');
  const peer = new URL(peerOrigin);
  check(peer.protocol === 'http:' && peer.hostname === '127.0.0.1' && peer.port
    && peer.pathname === '/' && !peer.username && !peer.password && !peer.search && !peer.hash,
  'browser_sign_out_peer_unbound');
  const replay = await fetch(new URL('/auth/session', peer), {redirect: 'error', signal: AbortSignal.timeout(timeout),
    headers: {Host: publicHost, Cookie: 'cma_session=' + cookie.value}});
  check(replay.status === 401 && (await replay.json()).authenticated === false, 'browser_sign_out_peer_still_authenticated');
}

async function openAccount(page, reload = false) {
  if (reload) await page.reload({waitUntil: 'domcontentloaded', timeout});
  else await page.goto('/', {waitUntil: 'domcontentloaded', timeout});
  await account(page).click({timeout});
}

export async function accountOperationsDenied(page, owner) {
  const operationsRequests = [];
  const observe = request => {
    if (new URL(request.url()).pathname.startsWith('/api/v1/admin/')) operationsRequests.push(request.method());
  };
  page.on('request', observe);
  try {
    await openAccount(page);
    await browserSession(page, 200, owner);
    await page.getByRole('menuitem', {name: 'Sign out', exact: true}).waitFor({timeout});
    check(await page.getByRole('menuitem', {name: 'Operations', exact: true}).count() === 0,
      'browser_non_operator_has_operations_menu_item');
    await page.goto('/operations', {waitUntil: 'domcontentloaded', timeout});
    await page.waitForURL(url => url.pathname === '/operations', {timeout});
    await page.getByRole('heading', {name: 'Operations unavailable', exact: true}).waitFor({timeout});
    await browserSession(page, 200, owner);
    check(await page.getByRole('tablist', {name: 'Operations views', exact: true}).count() === 0,
      'browser_non_operator_has_operations_controls');
    check(operationsRequests.length === 0, 'browser_non_operator_requested_admin_data');
    await page.getByRole('link', {name: 'Return to Agent Manager', exact: true}).click({timeout});
    await page.waitForURL(url => url.pathname === '/', {timeout});
    await account(page).waitFor({timeout});
  } finally {
    page.off('request', observe);
  }
}

async function select(page, group, label) {
  await choice(page, group, label).click({timeout});
  await choice(page, group, label, {checked: true}).waitFor({timeout});
}

const themeApplied = (page, dark) => page.waitForFunction(expected =>
  document.body.hasAttribute('data-ds-dark-theme') === expected
    && document.documentElement.style.colorScheme === (expected ? 'dark' : 'light'), dark, {timeout});

export async function accountAppearance(page) {
  await page.emulateMedia({colorScheme: 'light'});
  await openAccount(page);
  for (const [label, dark] of [['Light', false], ['Dark', true], ['System', false]]) {
    await select(page, 'Appearance', label);
    await themeApplied(page, dark);
    await openAccount(page, true);
    await choice(page, 'Appearance', label, {checked: true}).waitFor({timeout});
    await themeApplied(page, dark);
  }
  await page.emulateMedia({colorScheme: 'dark'});
  await themeApplied(page, true);
  await page.emulateMedia({colorScheme: 'light'});
  await themeApplied(page, false);
}

const textApplied = (page, label, factor) => page.waitForFunction(({size, factor}) =>
  document.documentElement.dataset.systemTextSize === size
    && getComputedStyle(document.documentElement).getPropertyValue('--cma-system-font-scale').trim() === factor,
{size: label.toLowerCase(), factor}, {timeout});

export async function accountTextSize(page) {
  await openAccount(page);
  const group = page.getByRole('group', {name: 'System text size', exact: true});
  const original = (await group.getByRole('menuitemradio', {checked: true}).innerText({timeout})).trim();
  const sizes = new Map([['Small', '0.9'], ['Medium', '1'], ['Large', '1.15']]);
  check(sizes.has(original), 'browser_original_text_size_unknown');
  for (const [label, scale] of sizes) {
    await select(page, 'System text size', label);
    await textApplied(page, label, scale);
    await openAccount(page, true);
    await textApplied(page, label, scale);
    await choice(page, 'System text size', label, {checked: true}).waitFor({timeout});
  }
  await select(page, 'System text size', original);
  await textApplied(page, original, sizes.get(original));
}

export async function accountBuildVersion(page, expectedSource) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await openAccount(page);
  const response = await page.request.get('/build-info.json', {maxRedirects: 0, timeout});
  check(response.ok(), 'browser_web_build_info_unavailable');
  const {version, revision, local} = await response.json();
  check(typeof version === 'string' && revision === expectedSource && typeof local === 'boolean',
    'browser_web_build_info_unbound');
  const copy = page.getByRole('menuitem', {name: /^Copy build version/});
  const label = await copy.innerText({timeout});
  check(label.startsWith(`Copy build version · Web ${version}${local ? ' (local)' : ''} · `),
    'browser_web_build_label_unbound');
  await copy.click({timeout});
  await page.getByRole('menuitem', {name: 'Build version copied', exact: true}).waitFor({timeout});
  const value = await page.evaluate(() => navigator.clipboard.readText());
  check(value.split(' · ')[0] === `Web ${version} (${revision})${local ? ' (local)' : ''}`,
    'browser_copied_web_build_unbound');
}

export async function accountDocumentation(page, agentId) {
  check(id(agentId), 'browser_docs_agent_required');
  const resource = '/agents/' + agentId;
  const openResource = async () => {
    await Promise.all([
      page.waitForResponse(async response => response.ok() && response.request().method() === 'GET'
        && new URL(response.url()).pathname === '/api/v1' + resource
        && (await response.json()).id === agentId, {timeout}),
      page.goto(resource, {waitUntil: 'domcontentloaded', timeout}),
    ]);
    await page.waitForURL(url => url.pathname === resource, {timeout});
  };
  await openResource();
  await account(page).click({timeout});
  await page.getByRole('menuitem', {name: 'Docs', exact: true}).click({timeout});
  await page.waitForURL('**/docs', {timeout});
  await page.getByRole('heading', {name: 'Docs', exact: true}).waitFor({timeout});
  await page.getByRole('link', {name: /^Start a task/}).click({timeout});
  await page.waitForURL('**/docs/start-a-task', {timeout});
  await page.getByRole('heading', {name: 'Write and review your task', exact: true}).waitFor({timeout});
  await page.goBack({waitUntil: 'domcontentloaded', timeout});
  await openGatewayDocumentation(page);
  await page.getByRole('link', {name: 'Docs', exact: true}).click({timeout});
  await page.waitForURL('**/docs', {timeout});
  await openResource();
  await account(page).waitFor({timeout});
}

async function openGatewayDocumentation(page) {
  await page.getByRole('link', {name: /^Event Gateway Protocol/}).click({timeout});
  await page.waitForURL('**/docs/event-gateway', {timeout});
  await page.getByRole('heading', {name: 'Event Gateway Protocol', level: 1}).waitFor({timeout});
  const outline = page.getByRole('navigation', {name: 'On this page'});
  for (const [title, section] of [['Integration guide', 'integration'], ['Architecture', 'architecture'],
    ['Protocol reference', 'protocol']]) {
    await outline.getByRole('link', {name: title, exact: true}).click({timeout});
    await page.waitForURL(`**/docs/event-gateway#${section}`, {timeout});
    await page.getByRole('region', {name: title, exact: true}).getByRole('heading').first().waitFor({timeout});
  }
}

export async function gatewayDocumentation(page) {
  await page.goto('/event-gateway', {waitUntil: 'domcontentloaded', timeout});
  const historyLength = await page.evaluate(() => history.length);
  await account(page).click({timeout});
  await page.getByRole('menuitem', {name: 'Docs', exact: true}).click({timeout});
  await page.waitForURL('**/docs', {timeout});
  await openGatewayDocumentation(page);

  const downloadLink = page.getByRole('link', {name: 'Download document', exact: true});
  const [document] = await Promise.all([page.waitForEvent('download', {timeout}), downloadLink.click({timeout})]);
  check(document.suggestedFilename() === 'agent-event-gateway.md' && await document.failure() === null,
    'browser_gateway_document_download_unbound');
  await document.delete();
  const reference = page.getByRole('link', {name: 'OpenAPI JSON', exact: true});
  check(await reference.getAttribute('href', {timeout}) === '/event-gateway/openapi.json',
    'browser_gateway_reference_link_unbound');
  const response = await page.request.get('/event-gateway/openapi.json', {maxRedirects: 0, timeout});
  check(response.ok(), 'browser_gateway_openapi_unavailable');
  const spec = await response.json();
  check(/^3\.\d+\.\d+$/.test(spec.openapi ?? '') && Object.keys(spec.paths ?? {}).length > 0,
    'browser_gateway_openapi_unbound');

  await page.evaluate(original => history.go(original - history.length), historyLength);
  await page.waitForURL(url => url.pathname === '/event-gateway', {timeout});
  await account(page).waitFor({timeout});
}

export async function gatewayNavigation(page, {agentId, sandboxId}, owner) {
  check(id(agentId) && id(sandboxId), 'browser_gateway_agent_lineage_required');
  const resource = '/agents/' + agentId;
  const service = name => page.getByRole('button', {name: 'Service: ' + name, exact: true});
  const agentActions = page.getByRole('button', {name: 'More Agent actions', exact: true});
  await page.goto(resource, {waitUntil: 'domcontentloaded', timeout});
  await service('Agent Manager').waitFor({timeout});
  await agentActions.waitFor({timeout});
  await browserSession(page, 200, owner);

  await page.getByRole('button', {name: 'Search', exact: true}).click({timeout});
  await page.getByRole('combobox', {name: 'Search commands'}).fill('Event Gateway', {timeout});
  await page.getByRole('option', {name: 'Event Gateway', exact: true}).click({timeout});
  await page.waitForURL(url => url.pathname === '/event-gateway', {timeout});
  await service('Event Gateway').waitFor({timeout});
  await browserSession(page, 200, owner);

  await service('Event Gateway').click({timeout});
  await page.getByRole('menuitemradio', {name: 'Agent Manager', exact: true}).click({timeout});
  await service('Agent Manager').waitFor({timeout});
  if (new URL(page.url()).pathname !== resource) {
    await page.locator(`[role="treeitem"][data-node-id="agent:${agentId}"]:visible a[href="${resource}"]`)
      .click({timeout});
  }
  await page.waitForURL(url => url.pathname === resource, {timeout});
  await agentActions.waitFor({timeout});
  await browserSession(page, 200, owner);
  const response = await page.request.get('/api/v1' + resource, {maxRedirects: 0, timeout});
  check(response.ok(), 'browser_gateway_returned_agent_unavailable');
  const detail = await response.json();
  check(detail.id === agentId && detail.sandbox?.id === sandboxId && detail.status === 'ready',
    'browser_gateway_returned_agent_lineage_changed');
}

export async function legacyNavigation(page) {
  const visit = path => page.goto(path, {waitUntil: 'domcontentloaded', timeout});
  const canonical = path => page.waitForURL(url => url.pathname === path, {timeout});
  const appearance = () => page.getByRole('status').filter({hasText: 'Appearance is now in your account menu.'});
  for (const [path, movedNotice] of [['/settings', true], ['/settings/general', true], ['/settings/usage', false]]) {
    await visit(path);
    await canonical('/usage');
    await page.getByRole('dialog', {name: 'Your usage', exact: true}).waitFor({timeout});
    await appearance().waitFor({state: movedNotice ? 'visible' : 'hidden', timeout});
  }
  await visit('/developer');
  await canonical('/docs');
  await page.getByRole('heading', {name: 'Docs', exact: true}).waitFor({timeout});
  for (const path of ['/profiles', '/profiles/agent-profiles/retired', '/marketplace', '/marketplace/agent/retired']) {
    await visit(path);
    await canonical(path);
    await page.getByRole('heading', {name: 'Configure Agents directly', exact: true}).waitFor({timeout});
  }
  for (const path of ['/image-manager', '/unknown-ci-route', '/agents/invalid%2Fsegment', '/agents/too/many/segments', '/docs/']) {
    await visit(path);
    await canonical(path);
    await page.getByRole('heading', {name: 'Page not found', exact: true}).waitFor({timeout});
  }
}
