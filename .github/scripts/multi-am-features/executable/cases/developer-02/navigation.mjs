import {define,setup,button,visible,absent,contains,enc,peer,api} from './helpers.mjs';
const forbidden = ['read_all_sandboxes','control_any_sandbox','read_runtime_inventory','read_event_log',
  'read_activity_log','read_user_directory','configure_platform'];
async function identity(ctx) {
  const response = await ctx.state.b.page.request.get(ctx.state.b.origin + '/auth/session');
  ctx.assert(response.status() === 200, 'browser_session_authenticated');
  const session = await response.json();
  ctx.assert(session.authenticated === true && session.subject === ctx.state.f.principal,
    'browser_session_exact_actor');
  return session;
}
export const cases = [
  define('navigation.eg-transition', [
    async ctx => {
      const {page,origin,f} = await setup(ctx,'navigation');
      await page.goto(origin + '/agents/' + enc(f.agentId));
      await button(page,'Search').click();
      await page.getByRole('textbox',{name:'Search commands'}).fill('Event Gateway');
      await page.getByRole('option',{name:'Event Gateway',exact:true}).click();
      await visible(ctx,button(page,'Service: Event Gateway'),'eg_service_selected');
    },
    async ctx => {
      await identity(ctx);
      await visible(ctx,button(ctx.state.b.page,/^Account menu for /),'eg_account_visible');
      ctx.assert(new URL(ctx.state.b.page.url()).pathname === '/event-gateway','eg_route_exact');
    },
    async ctx => {
      const {page} = ctx.state.b;
      await ctx.state.b.switchAM(peer(ctx));
      await button(page,'Service: Event Gateway').click();
      await page.getByRole('menuitemradio',{name:'Agent Manager',exact:true}).click();
      await page.locator(`a[href="/agents/${ctx.state.f.agentId}"]`).first().click();
      await visible(ctx,button(page,'More Agent actions'),'returned_authorized_agent');
      await identity(ctx);
    },
    async ctx => {
      const f = ctx.state.f;
      for (const am of [ctx.am,peer(ctx)]) {
        const r = await api(ctx,'/api/v1/agents/' + enc(f.agentId),{am,agentId:f.agentId});
        ctx.assert(r.body.id === f.agentId && r.body.sandbox.id === f.sandboxId && r.timing.am === am,
          'returned_agent_exact_resource');
        const sandbox = await api(ctx,'/api/v1/sandboxes/' + enc(f.sandboxId),{am,agentId:f.agentId});
        ctx.assert(sandbox.body.owner === f.principal,'returned_agent_current_owner');
      }
    },
  ]),
  define('navigation.admin-denied', [
    async ctx => {
      const b = await setup(ctx,'navigation'); await b.page.goto(b.origin + '/');
      const session = await identity(ctx);
      ctx.assert(Array.isArray(session.capabilities) && !forbidden.some(v => session.capabilities.includes(v)),
        'actor_has_no_operator_capability');
    },
    async ctx => {
      const {page} = ctx.state.b; await button(page,/^Account menu for /).click();
      await absent(ctx,page.getByRole('menuitem',{name:'Operations',exact:true}),'operations_hidden');
      await page.keyboard.press('Escape');
    },
    async ctx => {
      const {page,origin} = ctx.state.b; await page.goto(origin + '/operations');
      await visible(ctx,page.getByRole('heading',{name:'Operations unavailable',exact:true}),'operations_denied');
    },
    async ctx => {
      const {page,origin} = ctx.state.b;
      for (const am of [ctx.am,peer(ctx)]) {
        await ctx.state.b.switchAM(am); await page.reload(); await identity(ctx);
        await absent(ctx,page.getByRole('tab'),'no_operator_tabs');
        await absent(ctx,page.getByRole('table'),'no_operator_data');
        await contains(ctx,page.locator('main'),'Operations is for CMA operators','explicit_operator_denial');
        const denied = await page.request.get(origin + '/api/v1/admin/settings');
        ctx.assert(denied.status() === 403,'operator_api_denied_on_each_am');
      }
    },
  ]),
];
