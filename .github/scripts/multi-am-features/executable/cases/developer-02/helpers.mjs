import {readFileSync} from 'node:fs';
import {createHash, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
const catalog = JSON.parse(readFileSync(new URL('../../catalog.json', import.meta.url)));
export const enc = encodeURIComponent;
export const hash = value => createHash('sha256').update(typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
export const key = () => 'coverage02-' + randomUUID();
export const peer = ctx => ctx.am === 'a' ? 'b' : 'a';
export const need = (value, code) => {if (!value) throw new Blocked(code);};
export const button = (page, name) => page.getByRole('button', {name, exact: typeof name === 'string'});
export const api = (ctx, path, options = {}) => ctx.request({actor:'user', method:'GET', path, ...options});
export async function waitFor(predicate, code, timeout = 30000, interval = 200) {
  const deadline = Date.now() + timeout;
  do {if (await predicate()) return; await new Promise(resolve => setTimeout(resolve, interval));}
  while (Date.now() < deadline);
  throw Error(code);
}
export async function visible(ctx, node, code) {
  await node.waitFor({state:'visible', timeout:30000}); ctx.assert(await node.isVisible(), code);
}
export async function contains(ctx, node, text, code) {
  await waitFor(() => node.innerText().then(value => value.includes(text)), code);
  ctx.assert((await node.innerText()).includes(text), code);
}
export async function absent(ctx, node, code) {
  await waitFor(() => node.count().then(value => value === 0), code); ctx.assert(await node.count() === 0, code);
}
export async function poll(ctx, path, predicate, options = {}) {
  let found;
  await waitFor(async () => {
    found = await api(ctx, path, options); return predicate(found.body, found.status);
  }, 'public_state_deadline', 240000, 2000);
  return found.body;
}
export async function capture(page, method, path, action) {
  const pending = page.waitForResponse(r => r.request().method() === method
    && new URL(r.url()).pathname.replace(/^\/api\/agent-manager\//, '/api/') === path,
  {timeout:45000}).then(response => ({response}), () => ({error:true}));
  await action(); const {response} = await pending;
  if (!response) throw Error('ui_response_missing');
  const text = await response.text();
  return {status:response.status(), body:text ? JSON.parse(text) : null,
    request:response.request().postDataJSON(), request_id:response.headers()['x-request-id']};
}
export async function setup(ctx, name, actor = 'user') {
  const f = await ctx.fixture('user-fixtures').take(name, ctx);
  need(typeof f?.cleanup === 'function', 'ui_fixture_cleanup_missing');
  ctx.defer('dispose_ui_fixture', async () => {
    const receipt = await f.cleanup();
    ctx.assert(receipt?.complete === true && receipt.owned_only === true, 'ui_cleanup_complete');
    await ctx.observe('ui_cleanup', async () => receipt);
  });
  need(f.local === true && f.run_owned === true, 'local_owned_ui_fixture_required');
  const b = await ctx.fixture('browser').open({actor, am:ctx.am, agentId:f.agentId});
  ctx.defer('close_ui_session', async () => {
    try {await ctx.observe('browser_final_receipt', () => ctx.fixture('browser').evidence(b.page));}
    finally {await b.close();}
  });
  ctx.state.f = f; ctx.state.b = b; ctx.state.actor = actor;
  return {...b, f};
}
export async function enter(ctx, scenario) {
  const b = await setup(ctx, scenario); need(b.f.agentId, 'ui_agent_fixture_missing');
  await b.page.goto(b.origin + '/agents/' + enc(b.f.agentId));
  await visible(ctx, button(b.page, 'More Agent actions'), 'agent_toolbar_visible');
  return b;
}
export async function peerReload(ctx) {
  await ctx.state.b.switchAM(peer(ctx)); await ctx.state.b.page.reload({waitUntil:'domcontentloaded'});
}
export function validReceipt(receipt, expectedSource) {
  if (receipt?.schema !== 'multi-am-browser-evidence-v1' || !receipt.real_ui
    || receipt.compiled_source !== expectedSource || !/^sha256:[a-f0-9]{64}$/.test(receipt.compiled_manifest ?? '')
    || receipt.secrets_redacted !== true || !receipt.requests?.length) return false;
  return receipt.requests.every(r => {
    if (!r.request_id?.trim() || !['a','b'].includes(r.am) || !Number.isFinite(r.elapsed_ms)) return false;
    if (!r.agent_id_sha256) return true;
    const p = r.owner_proof;
    return r.owner_stable === true && Number.isSafeInteger(r.owner_epoch) && r.owner_epoch > 0
      && p?.verdict === 'PASS' && p.request_id === r.request_id && p.ingress === r.am
      && p.owner === r.owner && p.owner_epoch === r.owner_epoch
      && (r.owner !== r.am || (p.kind === 'peer_read' && ['GET','HEAD'].includes(r.method)));
  });
}
export async function finish(ctx) {
  const receipt = await ctx.observe('browser_receipt', () => ctx.fixture('browser').evidence(ctx.state.b.page));
  ctx.assert(validReceipt(receipt, ctx.state.f.webSource), 'ui_source_and_routing_proven');
  ctx.assert(new Set(receipt.requests.map(r => r.am)).size === 2, 'ui_both_am_receipts');
  if (ctx.state.f.agentId) ctx.assert(receipt.requests.some(r => r.agent_id_sha256 === hash(ctx.state.f.agentId)
    && r.owner !== r.am), 'ui_exact_agent_nonowner_receipt');
}
export function define(suffix, handlers, {scope = 'browser'} = {}) {
  const id = 'C275:cma-user.' + suffix, row = catalog.rows.find(r => r.case_id === id);
  if (!row || handlers.length !== row.required_steps.length) throw Error('ui_case_step_shape');
  return {id, scope, steps:[...row.required_steps], fixtures:['browser','user-fixtures'], async run(ctx) {
    for (let i = 0; i < handlers.length; i++) await ctx.step(row.required_steps[i], async () => {
      await handlers[i](ctx); if (i === handlers.length - 1) await finish(ctx);
    });
  }};
}
