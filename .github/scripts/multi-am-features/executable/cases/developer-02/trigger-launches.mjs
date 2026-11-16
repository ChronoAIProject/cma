import {define,setup,button,visible,contains,api,enc,peerReload,need,capture,waitFor,hash} from './helpers.mjs';
import {wizard,triggerPath,readTrigger,unavailable} from './trigger-helpers.mjs';
const launchPath = id => '/api/v1/launches/'+enc(id);
async function readLaunch(ctx,id) {return (await api(ctx,launchPath(id),{actor:ctx.state.actor,am:ctx.am})).body;}
async function retainLaunch(ctx,id) {
  need(typeof ctx.state.f.retainLaunch==='function','launch_custody_registration_missing');
  await ctx.state.f.retainLaunch(id);ctx.state.launchId=id;
}
export const cases = [
  define('triggers.retire',[
    async ctx => {
      await wizard(ctx,'trigger-published');const doc=await readTrigger(ctx);
      ctx.assert(doc.id===ctx.state.f.trigger.id&&doc.availability==='published','retire_owned_published_trigger');
      ctx.state.before=doc;ctx.state.independent=await ctx.state.f.journal.resources();
    },
    async ctx => {
      const {page}=ctx.state.b;await button(page,'Retire Trigger').click();
      await contains(ctx,page.getByRole('group').filter({hasText:'New launches will be refused'}),ctx.state.before.draft.presentation.title,'retire_exact_confirmation');
      const result=await capture(page,'POST',triggerPath(ctx)+'/retire',()=>button(page,'Retire Trigger').click());
      ctx.assert(result.status===200&&result.body.id===ctx.state.before.id&&result.body.availability==='retired','retirement_confirmed');
    },
    async ctx => {await peerReload(ctx);await unavailable(ctx);},
    async ctx => {
      const doc=await readTrigger(ctx);ctx.assert(doc.id===ctx.state.before.id&&doc.availability==='retired','retirement_persists');
      const resources=await ctx.state.f.journal.resources();
      ctx.assert(JSON.stringify(resources)===JSON.stringify(ctx.state.independent),'retirement_preserves_independent_resources');
      const launches=await ctx.state.f.journal.launches({triggerId:doc.id});
      ctx.assert(launches.every(l=>l.created_at<=doc.retired_at),'no_post_retirement_launch');
    },
  ]),
  define('triggers.public-launch',[
    async ctx => {
      const {page,origin,f}=await setup(ctx,'trigger-launch','outsider');
      need(f.trigger?.public_id&&f.placement?.id&&f.visitorPrincipal,'public_visitor_fixture_missing');
      await page.goto(origin+'/t/'+enc(f.trigger.public_id)+'/'+enc(f.placement.id));
      await visible(ctx,page.getByRole('heading',{name:f.trigger.draft.presentation.title,exact:true}),'public_landing_exact_trigger');
    },
    async ctx => {
      const {page,origin}=ctx.state.b,f=ctx.state.f;
      await page.getByText('Technical details',{exact:true}).click();
      await contains(ctx,page.locator('main'),f.targetDisclosure,'public_target_disclosed');
      const session=await page.request.get(origin+'/auth/session');const identity=await session.json();
      ctx.assert(identity.authenticated===true&&identity.subject===f.visitorPrincipal,'public_signed_in_visitor');
      ctx.assert(identity.subject!==f.principal,'visitor_is_not_publisher');
      await button(page,f.buttonLabel).click();
      await visible(ctx,page.getByRole('dialog',{name:'Choose your agent',exact:true}),'explicit_repeat_choice');
    },
    async ctx => {
      const {page}=ctx.state.b,f=ctx.state.f;
      need(['new','resume'].includes(f.repeatChoice),'public_repeat_choice_fixture_missing');
      const result=await capture(page,'POST','/api/v1/triggers/public/'+enc(f.trigger.public_id)+'/launches',
        ()=>button(page,f.repeatChoice==='new'?'Start fresh':'Continue where you left off').click());
      if(result.body?.id)await retainLaunch(ctx,result.body.id);
      ctx.assert([200,201,202].includes(result.status)&&Boolean(ctx.state.launchId),'public_launch_admitted');
      const current=await readLaunch(ctx,ctx.state.launchId);
      ctx.assert(current.id===ctx.state.launchId&&current.public_id===f.trigger.public_id,'public_admission_exact_identity');
    },
    async ctx => {
      const {page}=ctx.state.b,f=ctx.state.f;
      await visible(ctx,page.getByRole('dialog',{name:'Review and continue',exact:true}),'public_review_visible');
      const trust=page.getByRole('checkbox',{name:'I understand this software will be installed in my sandbox',exact:true});
      if(await trust.count())await trust.check();
      need(f.connections?.completeLocalOAuth,'local_connection_authorization_control_missing');
      await f.connections.completeLocalOAuth({page,launchId:ctx.state.launchId,subject:f.visitorPrincipal});
      await waitFor(()=>button(page.getByRole('dialog'),'Continue').isEnabled(),'public_connections_verified');
      await button(page.getByRole('dialog'),'Continue').click();
      await visible(ctx,page.getByRole('list',{name:'Agent provisioning progress'}),'public_launch_progress');
    },
    async ctx => {
      const {page}=ctx.state.b;
      await waitFor(()=>Promise.resolve(new URL(page.url()).pathname.startsWith('/agents/')),'public_ready_navigation',240000);
      ctx.state.launchedAgent=decodeURIComponent(new URL(page.url()).pathname.split('/')[2]);
      await visible(ctx,button(page,'More Agent actions'),'launched_agent_ready_ui');
      const launch=await readLaunch(ctx,ctx.state.launchId);ctx.state.launch=launch;
      ctx.assert(launch.resolution.agent.id===ctx.state.launchedAgent,'launch_progress_exact_agent');
    },
    async ctx => {
      const f=ctx.state.f;await peerReload(ctx);
      const launch=await readLaunch(ctx,ctx.state.launchId);
      ctx.assert(launch.owner===f.visitorPrincipal&&launch.placement_id===f.placement.id,'launch_owner_placement_bound');
      const journal=await f.journal.launches({triggerId:f.trigger.id,visitor:f.visitorPrincipal});
      ctx.assert(journal.filter(r=>r.id===ctx.state.launchId).length===1,'one_launch_intent');
      ctx.assert(f.repeatChoice==='resume'?ctx.state.launchedAgent===f.existingAgentId:ctx.state.launchedAgent!==f.existingAgentId,
        'launch_repeat_choice_honored');
      await ctx.observe('launch_resource_identity',async()=>({launch_sha256:hash(launch.id),agent_sha256:hash(ctx.state.launchedAgent)}));
    },
  ]),
  define('triggers.launch-recovery',[
    async ctx => {
      const {page,origin,f}=await setup(ctx,'trigger-recovery','outsider');need(f.launchId,'admitted_launch_fixture_missing');
      await retainLaunch(ctx,f.launchId);await page.goto(origin+'/launches/'+enc(f.launchId));
      await visible(ctx,page.getByRole('heading',{name:'Preparing your agent',exact:true}),'launch_recovery_open');
      ctx.state.before=await f.journal.launches({triggerId:f.trigger.id});
    },
    async ctx => {
      const launch=await readLaunch(ctx,ctx.state.launchId);
      const requirements=await api(ctx,launchPath(launch.id)+'/requirements',{actor:'outsider',am:ctx.am});
      const events=await api(ctx,launchPath(launch.id)+'/events',{actor:'outsider',am:ctx.am});
      ctx.assert(launch.id===ctx.state.launchId&&Array.isArray(launch.steps),'launch_progress_identity');
      ctx.assert(requirements.status===200&&events.status===200,'launch_requirements_events_readable');
      await visible(ctx,ctx.state.b.page.getByRole('list',{name:'Agent provisioning progress'}),'launch_progress_steps_visible');
    },
    async ctx => {
      await peerReload(ctx);await visible(ctx,ctx.state.b.page.getByRole('heading',{name:'Preparing your agent',exact:true}),'launch_progress_reload');
      ctx.assert(new URL(ctx.state.b.page.url()).pathname==='/launches/'+enc(ctx.state.launchId),'reload_same_launch');
      const after=await ctx.state.f.journal.launches({triggerId:ctx.state.f.trigger.id});
      ctx.assert(JSON.stringify(after.map(x=>x.id))===JSON.stringify(ctx.state.before.map(x=>x.id)),'observation_no_new_intent');
    },
    async ctx => {
      const f=ctx.state.f;await f.provider.releaseTerminalFailure({launchId:ctx.state.launchId});
      await visible(ctx,button(ctx.state.b.page,'Copy support code'),'terminal_support_action_visible');
      await button(ctx.state.b.page,'Copy support code').click();
      const code=await ctx.state.b.page.evaluate(()=>navigator.clipboard.readText());
      ctx.assert(code===ctx.state.launchId,'support_code_exact_original_launch');
      await ctx.observe('support_code_digest',async()=>({sha256:hash(code)}));
    },
    async ctx => {
      const launch=await readLaunch(ctx,ctx.state.launchId);
      ctx.assert(launch.id===ctx.state.f.launchId&&Boolean(launch.failure),'terminal_failure_exact_launch');
      const after=await ctx.state.f.journal.launches({triggerId:ctx.state.f.trigger.id});
      ctx.assert(JSON.stringify(after.map(x=>x.id))===JSON.stringify(ctx.state.before.map(x=>x.id)),'support_copy_no_second_operation');
    },
  ]),
];
