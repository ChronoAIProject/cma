import {define,enter,api,button,visible,contains,enc,peerReload,need,capture,poll,waitFor} from './helpers.mjs';
const path = ctx => '/api/v1/agents/' + enc(ctx.state.f.agentId);
async function details(ctx,name) {
  const b = await enter(ctx,name); await button(b.page,'Show Agent details').click();
  await visible(ctx,b.page.getByRole('complementary',{name:'Agent details'}),'details_open'); return b;
}
const article = ctx => ctx.state.b.page.getByRole('article').filter({hasText:ctx.state.f.feedbackText});
const feedbackPath = ctx => path(ctx) + '/messages/' + enc(ctx.state.f.feedbackMessageId) + '/feedback';
async function exactFeedback(ctx, vote) {
  const r = await api(ctx,feedbackPath(ctx),{agentId:ctx.state.f.agentId});
  ctx.assert(r.body.viewer_id === ctx.state.f.principal && r.body.agent_id === ctx.state.f.agentId
    && r.body.message_id === ctx.state.f.feedbackMessageId && r.body.vote === vote,'feedback_exact_identity');
  return r.body;
}
export const cases = [
  define('agent.feedback',[
    async ctx => {
      await enter(ctx,'agent-feedback'); need(ctx.state.f.feedbackMessageId && ctx.state.f.feedbackText,'feedback_message_missing');
      await button(article(ctx),'Helpful').click(); await exactFeedback(ctx,'up');
      await button(article(ctx),'Helpful').click(); await exactFeedback(ctx,null);
      await contains(ctx,article(ctx).getByRole('status',{name:'Vote status'}),'Vote removed','vote_removed_ui');
    },
    async ctx => {await button(article(ctx),'Needs improvement').click(); await exactFeedback(ctx,'down');},
    async ctx => {
      const {page,f} = {...ctx.state.b,f:ctx.state.f};
      const reasons = page.getByRole('region',{name:'Reasons for improvement'});
      await reasons.getByRole('checkbox',{name:'Inaccurate content',exact:true}).check();
      await reasons.getByRole('checkbox',{name:'Incomplete answer',exact:true}).check();
      await reasons.getByRole('textbox',{name:'Additional details (optional)',exact:true}).fill('Coverage fixture feedback.');
      need(f.faults?.loseNextResponse,'feedback_lost_ack_control_missing');
      const fault = await f.faults.loseNextResponse({method:'PUT',path:feedbackPath(ctx)+'/reasons'});
      ctx.defer('restore_feedback_ack',() => fault.restore());
      await button(reasons,'Save reasons').click();
      await visible(ctx,button(reasons,'Retry reasons'),'feedback_retry_visible');
      await fault.restore();
      const retry = await capture(page,'PUT',feedbackPath(ctx)+'/reasons',() => button(reasons,'Retry reasons').click());
      const original = await fault.receipt();
      ctx.assert(retry.status === 200 && JSON.stringify(original.request_body) === JSON.stringify(retry.request)
        && original.request_body.request_id,'feedback_retry_preserves_intent');
      ctx.state.saved = retry.body; await contains(ctx,reasons,'Reasons saved','reasons_saved_ui');
    },
    async ctx => {
      await peerReload(ctx); await button(article(ctx),'Edit reasons').click();
      const reasons = ctx.state.b.page.getByRole('region',{name:'Reasons for improvement'});
      ctx.assert(await reasons.getByRole('checkbox',{name:'Inaccurate content',exact:true}).isChecked()
        && await reasons.getByRole('checkbox',{name:'Incomplete answer',exact:true}).isChecked(),'reasons_persisted_ui');
      ctx.assert(await reasons.getByRole('textbox',{name:'Additional details (optional)'}).inputValue()
        === 'Coverage fixture feedback.','feedback_comment_persisted');
    },
    async ctx => {
      const saved = await exactFeedback(ctx,'down');
      ctx.assert(saved.version === ctx.state.saved.version && saved.request_id === ctx.state.saved.request_id,
        'feedback_one_durable_update');
      const f = ctx.state.f;
      need(f.otherMessageId && f.otherMessageBefore,'feedback_isolation_fixture_missing');
      const other = await api(ctx,path(ctx)+'/messages/'+enc(f.otherMessageId)+'/feedback',{agentId:f.agentId});
      ctx.assert(JSON.stringify(other.body) === JSON.stringify(f.otherMessageBefore),'other_message_unchanged');
    },
  ],{scope:'agent'}),
  define('agent.details-resources',[
    async ctx => {await details(ctx,'agent-details');},
    async ctx => {
      const f = ctx.state.f, detail = (await api(ctx,path(ctx),{agentId:f.agentId})).body;
      const repository = ctx.state.b.page.getByRole('region',{name:'Repository context'});
      need(detail.workspace.source?.type === 'repository_snapshot','repository_fixture_missing');
      await contains(ctx,repository,detail.workspace.source.url,'repository_identity');
      await contains(ctx,repository,detail.workspace.source.reference ?? 'Default branch','repository_ref');
      const machine = ctx.state.b.page.getByRole('region',{name:'Sandbox machine'});
      await contains(ctx,machine,detail.sandbox.id,'details_sandbox_identity');
      for (const scope of detail.reach) await contains(ctx,machine,scope,'details_key_scope');
      if (!detail.reach.length) await contains(ctx,machine,'No services are enabled.','empty_key_scope_explicit');
    },
    async ctx => {
      const f = ctx.state.f, panel = ctx.state.b.page.getByRole('region',{name:'Sandbox resources'});
      const sample = await api(ctx,path(ctx)+'/resources',{agentId:f.agentId});
      ctx.assert(sample.body.agent_id === f.agentId && sample.body.sandbox_id === f.sandboxId,'resource_sample_identity');
      await contains(ctx,panel,'RAM','ram_label'); await contains(ctx,panel,'CPU','cpu_label');
      need(f.resources?.missingSample && f.resources?.configuredSample,'resource_variants_missing');
      const missing = await f.resources.missingSample({agentId:f.agentId}); ctx.defer('restore_missing_resource_sample',()=>missing.restore());
      await peerReload(ctx); await button(ctx.state.b.page,'Show Agent details').click();
      await contains(ctx,panel,'not reported','missing_resources_not_zero'); await missing.restore();
      const configured = await f.resources.configuredSample({agentId:f.agentId}); ctx.defer('restore_configured_resource_sample',()=>configured.restore());
      await peerReload(ctx); await button(ctx.state.b.page,'Show Agent details').click();
      await contains(ctx,panel,'configured','capacity_distinguished_from_usage');
    },
    async ctx => {
      await button(ctx.state.b.page,'Sandbox operation progress').click();
      ctx.assert(new URL(ctx.state.b.page.url()).pathname.includes(enc(ctx.state.f.sandboxId)),'progress_exact_sandbox');
      await visible(ctx,ctx.state.b.page.getByRole('tab',{name:'Activity',exact:true}),'sandbox_activity_tab');
    },
    async ctx => {
      const f = ctx.state.f, sandbox = await api(ctx,'/api/v1/sandboxes/'+enc(f.sandboxId),{agentId:f.agentId});
      ctx.assert(sandbox.body.id === f.sandboxId && sandbox.body.owner === f.principal,'progress_current_resource_owner');
    },
  ],{scope:'agent'}),
  define('agent.turn-usage',[
    async ctx => {
      const {page,f} = await enter(ctx,'agent-usage');
      need(f.provider?.prepareTurn && f.turnMarker,'usage_provider_fixture_missing');
      await f.provider.prepareTurn({agentId:f.agentId,marker:f.turnMarker,contextWindow:null});
      await page.getByRole('textbox',{name:'Message Agent',exact:true}).fill('Reply only '+f.turnMarker);
      await button(page,'Send message').click();
      await visible(ctx,page.getByRole('article').filter({hasText:f.turnMarker}),'harmless_turn_completed');
      ctx.state.usage = await poll(ctx,path(ctx)+'/usage',b => b.current_turn && b.output_tokens > 0,{agentId:f.agentId});
    },
    async ctx => {
      const {page} = ctx.state.b, u = ctx.state.usage.current_turn;
      await visible(ctx,page.locator(`[aria-label="Turn input tokens: ${u.input_tokens.toLocaleString('en')}"]`).last(),'turn_input_count');
      await visible(ctx,page.locator(`[aria-label="Turn output tokens: ${u.output_tokens.toLocaleString('en')}"]`).last(),'turn_output_count');
      await button(page,'Context usage').click(); await visible(ctx,page.getByRole('tooltip'),'context_usage_visible');
    },
    async ctx => {
      const u = ctx.state.usage; ctx.assert(u.agent_id === ctx.state.f.agentId,'usage_exact_agent');
      await contains(ctx,ctx.state.b.page.getByRole('tooltip'),u.current_turn.input_tokens.toLocaleString('en'),'usage_projection_matches_ui');
      const transcript = await api(ctx,path(ctx)+'/transcript',{agentId:ctx.state.f.agentId});
      ctx.assert(transcript.body.turns.some(t => t.turn_sequence === u.current_turn.turn_sequence
        && t.outcome.type === 'completed' && t.messages.some(m => m.role === 'assistant' && m.content.includes(ctx.state.f.turnMarker))),
      'usage_exact_completed_turn');
    },
    async ctx => {
      ctx.assert(ctx.state.usage.context_window === undefined,'missing_context_limit_fixture');
      ctx.assert(!(await ctx.state.b.page.getByRole('tooltip').innerText()).includes('%'),'missing_limit_no_invented_capacity');
      await peerReload(ctx);
      const u = await api(ctx,path(ctx)+'/usage',{agentId:ctx.state.f.agentId});
      ctx.assert(u.body.current_turn.turn_sequence === ctx.state.usage.current_turn.turn_sequence,'usage_turn_survives_reload');
    },
  ],{scope:'agent'}),
  define('agent.skills',[
    async ctx => {
      await details(ctx,'agent-skills'); const f = ctx.state.f;
      const saved = (await api(ctx,path(ctx)+'/skills',{agentId:f.agentId})).body;
      ctx.assert(['installed','pending','failed'].every(s => saved.items.some(i => i.status === s)),'skill_status_fixture_variants');
      const list = ctx.state.b.page.getByRole('list',{name:'Agent skill status'});
      for (const label of ['Installed','Installing…','Installation failed']) await contains(ctx,list,label,'skill_status_explicit');
    },
    async ctx => {
      const {page} = ctx.state.b, f = ctx.state.f; need(f.skill?.id && f.skill.name && f.skill.version,'catalog_skill_fixture_missing');
      await page.locator('summary').filter({hasText:'Add skills'}).click();
      await page.getByRole('textbox',{name:'Search available Ornn skills'}).fill(f.skill.search);
      await button(page,'More Ornn skills').click();
      await visible(ctx,button(page,'Install '+f.skill.name),'catalog_page_skill_visible');
    },
    async ctx => {
      const {page} = ctx.state.b, f = ctx.state.f;
      await f.provider.settlePendingSkills({agentId:f.agentId});
      await waitFor(() => button(page,'Install '+f.skill.name).isEnabled(),'skill_install_ready');
      await button(page,'Version for '+f.skill.name).click();
      await page.getByRole('menuitemradio',{name:f.skill.version,exact:true}).click();
      const response = await capture(page,'POST',path(ctx)+'/skills',() => button(page,'Install '+f.skill.name).click());
      ctx.assert([200,202].includes(response.status) && response.request.id === f.skill.id
        && response.request.version === f.skill.version,'skill_exact_install_request');
      ctx.state.install = response;
    },
    async ctx => {
      const f = ctx.state.f;
      const saved = await poll(ctx,path(ctx)+'/skills',b => b.items.some(i => i.id === f.skill.id && i.status !== 'pending'),{agentId:f.agentId});
      const item = saved.items.find(i => i.id === f.skill.id); ctx.state.skillResult = item;
      ctx.assert(item.version === f.skill.version && ['installed','failed'].includes(item.status),'skill_terminal_exact_version');
      await peerReload(ctx); await button(ctx.state.b.page,'Show Agent details').click();
      await contains(ctx,ctx.state.b.page.getByRole('list',{name:'Agent skill status'}),item.status === 'installed'?'Installed':'Installation failed','skill_terminal_visible');
    },
    async ctx => {
      const f = ctx.state.f; need(f.provider?.skillJournal,'skill_effect_journal_missing');
      const rows = await f.provider.skillJournal({agentId:f.agentId,skillId:f.skill.id});
      ctx.assert(rows.length === 1 && rows[0].version === f.skill.version,'skill_installed_once');
      if (ctx.state.skillResult.status === 'failed') ctx.assert(Boolean(ctx.state.skillResult.error),'skill_concrete_error');
    },
  ],{scope:'agent'}),
];
