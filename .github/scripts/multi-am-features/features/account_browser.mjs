import {check, id} from '../cma/core.mjs';

const timeout = 15000;
const account = page => page.getByRole('button', {name: /^Account menu for /});
const choice = (page, group, name, options = {}) => page.getByRole('group', {name: group, exact: true})
  .getByRole('menuitemradio', {name, exact: true, ...options});

async function openAccount(page, reload = false) {
  if (reload) await page.reload({waitUntil: 'domcontentloaded', timeout});
  else await page.goto('/', {waitUntil: 'domcontentloaded', timeout});
  await account(page).click({timeout});
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
  await page.getByRole('link', {name: 'Docs', exact: true}).click({timeout});
  await page.waitForURL('**/docs', {timeout});
  await openResource();
  await account(page).waitFor({timeout});
}
