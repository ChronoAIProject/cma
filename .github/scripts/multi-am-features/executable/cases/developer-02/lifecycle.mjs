import {define,enter,api,button,visible,contains,enc,peerReload,poll,capture,need} from './helpers.mjs';
import {transcript,shown} from './history.mjs';
const agentPath = ctx => '/api/v1/agents/' + enc(ctx.state.f.agentId);
export const stopEligible = status => status === 'working';
export const cases = [
  define('agent.stop',[
    async ctx => {
      const {page,f} = await enter(ctx,'agent-stop');
      need(f.createdByRun === true && f.stopBarrier,'owned_running_agent_and_stop_barrier_missing');
      ctx.state.before = await transcript(ctx);
      const initial = await api(ctx,agentPath(ctx),{agentId:f.agentId});
      ctx.assert(stopEligible(initial.body.status),'stop_starts_working');
      await page.locator(`[data-node-id="agent:${f.agentId}"]`).getByRole('button',{name:'Options',exact:true}).click();
      ctx.state.acceptance = await capture(page,'POST',agentPath(ctx)+'/stop',
        () => {ctx.state.finishStop = ctx.startAction('stop_to_terminal');
          return page.getByRole('menuitem',{name:'Stop',exact:true}).click();});
      ctx.assert([200,202,204].includes(ctx.state.acceptance.status),'stop_request_accepted');
      const progress = page.getByRole('list',{name:'Stop Agent progress'});
      await contains(ctx,progress,'Accepted','stop_accepted_label');
      await contains(ctx,progress,'Waiting for confirmation','acceptance_is_not_completion');
    },
    async ctx => {
      await ctx.state.f.stopBarrier.release();
      await visible(ctx,ctx.state.b.page.getByRole('heading',{name:'Agent stopped',exact:true}),'stop_confirmed');
      await contains(ctx,ctx.state.b.page.getByRole('list',{name:'Stop Agent progress'}),'Confirmed','stop_confirmed_label');
      const current = await poll(ctx,agentPath(ctx),b => b.status === 'stopped',{agentId:ctx.state.f.agentId});
      ctx.assert(current.id === ctx.state.f.agentId,'stopped_exact_agent');
      ctx.state.finishStop();
      await button(ctx.state.b.page,'Done').click();
    },
    async ctx => {await peerReload(ctx); await shown(ctx,ctx.state.before);},
    async ctx => {
      const current = await api(ctx,agentPath(ctx),{agentId:ctx.state.f.agentId});
      ctx.assert(current.body.status === 'stopped','stop_persists');
      const after = await transcript(ctx); await shown(ctx,after);
      ctx.assert(after.turns.length >= ctx.state.before.turns.length,'stop_retains_history');
    },
  ],{scope:'agent'}),
  define('agent.delete',[
    async ctx => {
      const {f} = await enter(ctx,'agent-delete');
      need(f.createdByRun === true && f.unrelatedAgentId && f.unrelatedAgentId !== f.agentId,'owned_delete_fixture_missing');
      const before = await api(ctx,agentPath(ctx),{agentId:f.agentId});
      ctx.assert(['stopped','failed'].includes(before.body.status),'delete_stopped_or_failed');
      ctx.state.title = before.body.title?.trim() || 'Untitled agent';
      ctx.state.sibling = (await api(ctx,'/api/v1/agents/'+enc(f.unrelatedAgentId),{agentId:f.unrelatedAgentId})).body;
    },
    async ctx => {
      const {page} = ctx.state.b; await button(page,'More Agent actions').click();
      await page.getByRole('menuitem',{name:'Delete task',exact:true}).click();
      const dialog = page.getByRole('dialog',{name:'Delete Agent',exact:true});
      await contains(ctx,dialog,`Delete "${ctx.state.title}"`,'delete_exact_displayed_target');
      await dialog.getByRole('checkbox',{name:'I understand this Agent will be deleted.',exact:true}).check();
      const result = await capture(page,'DELETE',agentPath(ctx),() => button(dialog,'Delete Agent').click());
      ctx.assert([200,202,204].includes(result.status),'delete_accepted');
    },
    async ctx => {
      const {page} = ctx.state.b;
      await button(page,/^View Agent deletion status/).click();
      await visible(ctx,page.getByRole('heading',{name:'Agent deleted',exact:true}),'deletion_terminal_ui');
      await contains(ctx,page.getByRole('dialog'),'Agent deletion is confirmed.','deletion_confirmed_ui');
    },
    async ctx => {
      const f = ctx.state.f; await peerReload(ctx);
      const deleted = (await api(ctx,agentPath(ctx),{agentId:f.agentId})).body;
      ctx.assert(deleted.id === f.agentId && deleted.status === 'deleted' && deleted.worktree.status === 'removed',
        'deletion_requires_tombstone_and_worktree_finality');
      const sibling = (await api(ctx,'/api/v1/agents/'+enc(f.unrelatedAgentId),{agentId:f.unrelatedAgentId})).body;
      ctx.assert(JSON.stringify(sibling) === JSON.stringify(ctx.state.sibling),'unrelated_agent_unchanged');
    },
  ],{scope:'agent'}),
];
