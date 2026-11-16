import {randomUUID, createHash} from 'node:crypto';
import {readFileSync} from 'node:fs';
import {Blocked} from '../../runner.mjs';

const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
export const hash = value => createHash('sha256').update(value).digest('hex');
export const other = am => am === 'a' ? 'b' : 'a';
export const check = (value, code) => {if (!value) throw Error(code);};
export const button = (page, name) => page.getByRole('list', {name: 'Home files', exact: true})
  .getByRole('button', {name, exact: true});
export const filePath = (fixture, suffix = '', path = fixture.root) =>
  '/api/v1/agents/' + encodeURIComponent(fixture.agentId) + '/files' + suffix + '?' + new URLSearchParams({path});
export function define(name, run) {
  const row = catalog.rows.find(row => row.id === 'cma-user.files.' + name);
  return {id: row.case_id, scope: 'agent', steps: row.required_steps, fixtures: ['browser', 'user-fixtures'], run};
}
export async function visible(ctx, locator, code) {
  await locator.waitFor({state: 'visible', timeout: 30000}); ctx.assert(await locator.isVisible(), code);
}
export async function raw(ctx, fixture, method, suffix, path, options = {}) {
  return ctx.request({actor: fixture.actor, agentId: fixture.agentId, method,
    path: filePath(fixture, suffix, path), ...options});
}
export async function write(ctx, fixture, path, content = '', extra = {}) {
  const result = await ctx.request({actor: fixture.actor, agentId: fixture.agentId,
    method: 'POST', path: '/api/v1/agents/' + encodeURIComponent(fixture.agentId) + '/files',
    body: {path, content_base64: Buffer.from(content).toString('base64'), ...extra}, key: randomUUID()});
  check(result.body?.path === path && result.body.deleted !== true, 'file_write_receipt');
  fixture.lastWriteAm = result.timing.am; return result.body;
}
export const read = (ctx, fixture, path) => raw(ctx, fixture, 'GET', '/content', path,
  {am: other(fixture.lastWriteAm ?? ctx.am)});
export async function observedPeer(ctx, fixture) {
  const evidence = await ctx.fixture('browser').evidence(fixture.page);
  const last = evidence.requests.filter(request => ['POST', 'PATCH', 'PUT', 'DELETE'].includes(request.method)
    && request.agent_id_sha256 === hash(fixture.agentId)).at(-1);
  check(last && ['a', 'b'].includes(last.am), 'actual_browser_write_ingress_required');
  return other(last.am);
}
export async function setup(ctx, scenario = 'files') {
  const fixture = await ctx.fixture('user-fixtures').take(scenario, ctx);
  ctx.defer('release_file_fixture', () => fixture.cleanup());
  const browser = ctx.fixture('browser');
  const session = await browser.open({actor: fixture.actor, am: ctx.am, agentId: fixture.agentId});
  ctx.defer('close_file_browser', () => session.close());
  ctx.defer('retain_file_browser', () => ctx.observe('file_browser', () => browser.evidence(session.page)));
  return {...fixture, session, page: session.page};
}
export async function open(ctx, fixture) {
  const {page, session} = fixture;
  await page.goto(session.origin + '/agents/' + encodeURIComponent(fixture.agentId));
  await page.getByRole('button', {name: 'Show Agent computer', exact: true}).click();
  await page.getByRole('tab', {name: 'Files', exact: true}).click();
  await page.getByRole('list', {name: 'Home files', exact: true}).waitFor({state: 'visible'});
  await button(page, fixture.root).click();
  await button(page, fixture.root).waitFor({state: 'visible'});
}

export async function fileSetup(ctx, primitives) {
  const witness = primitives.fileAgent;
  if (!witness?.ownedByRun || typeof witness.agentId !== 'string' || !witness.actor
    || typeof primitives.runtimeFor !== 'function') throw new Blocked('file_runtime_fixture_unavailable');
  const runtime = await primitives.runtimeFor({agentId: witness.agentId});
  if (!runtime?.ready || runtime.image !== witness.runtimeImage || !runtime.pod_uid) {
    throw new Blocked('file_runtime_image_unbound');
  }
  const fixture = {agentId: witness.agentId, actor: witness.actor,
    root: 'coverage-' + randomUUID(), local: true, ownedByRun: true};
  let dispatched = false;
  fixture.cleanup = async () => {
    if (!dispatched) return;
    const current = await raw(ctx, fixture, 'GET', '/tree', fixture.root, {allowed: [200, 404]});
    if (current.status === 404) return;
    const deleted = await raw(ctx, fixture, 'DELETE', '', fixture.root,
      {allowed: [200], body: undefined, ...{path: filePath(fixture) + '&recursive=true'}});
    check(deleted.body?.deleted === true && deleted.body.path === fixture.root, 'file_cleanup_receipt');
    dispatched = false;
  };
  // Register before the first mutation: a lost acknowledgement cannot orphan
  // an exact uniquely named public fixture directory.
  ctx.defer('partial_file_setup', () => fixture.cleanup());
  const existing = await raw(ctx, fixture, 'GET', '/tree', '');
  check(!existing.body.entries.some(entry => entry.name === fixture.root), 'file_fixture_collision');
  dispatched = true;
  await write(ctx, fixture, fixture.root, '', {directory: true});
  return fixture;
}

export const setups = {
  files: fileSetup,
  async 'files-download'(ctx, primitives) {
    if (typeof primitives.largeFile !== 'function') throw new Blocked('large_file_runtime_fixture_unavailable');
    const fixture = await fileSetup(ctx, primitives);
    const large = await primitives.largeFile({agentId: fixture.agentId, directory: fixture.root,
      bytes: 32 * 1024 * 1024 + 1});
    check(large?.path?.startsWith(fixture.root + '/') && large.size_bytes === 32 * 1024 * 1024 + 1,
      'large_file_fixture_receipt');
    fixture.large = large; return fixture;
  },
};
