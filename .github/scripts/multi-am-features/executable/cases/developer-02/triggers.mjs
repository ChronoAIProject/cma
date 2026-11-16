import {define,button,visible,contains,api,enc,peerReload,need,capture,waitFor,hash} from './helpers.mjs';
import {wizard,next,review,pick,triggerPath,readTrigger,exactDraft,unavailable} from './trigger-helpers.mjs';
import {cases as launches} from './trigger-launches.mjs';

export const cases = [
  define('triggers.draft',[
    async ctx => {await wizard(ctx); await exactDraft(ctx,await readTrigger(ctx));},
    async ctx => {
      const {page}=ctx.state.b,f=ctx.state.f;
      need(f.inlineTarget?.imageLabel && f.inlineTarget.model && f.inlineTarget.skill,'inline_target_choices_missing');
      await pick(page,'Image',f.inlineTarget.imageLabel);
      await page.getByLabel('Model',{exact:true}).fill(f.inlineTarget.model);
      await pick(page,'Reasoning effort',f.inlineTarget.reasoning);
      await pick(page,'Permission level',f.inlineTarget.permission);
      await page.getByLabel('Skills (comma separated)',{exact:true}).fill(f.inlineTarget.skill);
      await page.getByLabel('Agent key scope services (comma separated)',{exact:true}).fill(f.inlineTarget.services.join(', '));
      await page.getByLabel('System prompt',{exact:true}).fill(f.inlineTarget.systemPrompt);
      // An unsafe repository choice must stop progression, not silently vanish.
      await page.getByLabel('GitHub repository (optional)',{exact:true}).fill('http://127.0.0.1/unsafe');
      await button(page,'Next').click();
      await visible(ctx,page.getByRole('alert').filter({hasText:'Enter an HTTPS GitHub repository URL.'}),'unsafe_repository_refused');
      await page.getByLabel('GitHub repository (optional)',{exact:true}).fill(f.inlineTarget.repository);
      await page.getByLabel('GitHub repository ref',{exact:true}).fill(f.inlineTarget.ref);
      await next(ctx,'When someone clicks again');
    },
    async ctx => {
      const {page}=ctx.state.b,f=ctx.state.f;
      await page.getByRole('radiogroup',{name:'sandbox reuse',exact:true}).getByRole('radio',{name:'Use their existing one',exact:true}).check();
      await page.getByRole('radiogroup',{name:'workspace reuse',exact:true}).getByRole('radio',{name:'Use their existing one',exact:true}).check();
      await page.getByRole('radiogroup',{name:'Agent reuse',exact:true}).getByRole('radio',{name:'Ask them',exact:true}).check();
      await next(ctx,'What it says');
      for(const [label,value] of [['title',f.presentation.title],['description',f.presentation.description],
        ['First message',f.presentation.firstMessage],['Sign-up hint (optional)',f.presentation.signupHint]])
        await page.getByLabel(label,{exact:true}).fill(value);
      await button(page,'Add quick message').click();
      const quick=page.getByRole('group',{name:'Quick message 1'});
      for(const [label,value] of [['Name',f.quickMessage.name],['Description',f.quickMessage.description],['Message text',f.quickMessage.text]])
        await quick.getByLabel(label,{exact:true}).fill(value);
      await pick(page,'Theme','Dark');
      ctx.assert(await page.getByLabel('title',{exact:true}).inputValue()===f.presentation.title,'draft_content_selected');
    },
    async ctx => {
      const {page}=ctx.state.b;
      await button(page,'Save draft').click();
      await visible(ctx,page.getByRole('status').filter({hasText:/Draft version \d+ saved/}),'draft_save_confirmed');
      ctx.state.saved=await readTrigger(ctx);await peerReload(ctx);
      await review(ctx);await contains(ctx,page.locator('.trigger-review'),ctx.state.f.presentation.title,'draft_reload_exact');
    },
    async ctx => {
      const f=ctx.state.f,doc=await readTrigger(ctx);await exactDraft(ctx,doc);
      ctx.assert(JSON.stringify(doc.draft)===JSON.stringify(ctx.state.saved.draft)
        &&doc.draft_version===ctx.state.saved.draft_version,'draft_revision_and_content_preserved');
      ctx.assert(doc.draft.first_message===f.presentation.firstMessage
        &&doc.draft.quick_messages.some(q=>q.name===f.quickMessage.name&&q.text===f.quickMessage.text)
        &&doc.draft.target.inline.agent.model===f.inlineTarget.model,'draft_selections_persisted');
    },
  ]),
  define('triggers.publish',[
    async ctx => {
      await wizard(ctx);await review(ctx); const {page}=ctx.state.b,f=ctx.state.f;
      await contains(ctx,page.locator('.trigger-review'),f.trigger.draft.first_message,'publish_first_message_disclosed');
      await contains(ctx,page.locator('.trigger-review'),f.reuseDisclosure,'publish_reuse_disclosed');
      for(const disclosure of f.trustDisclosures ?? []) {
        const details=page.locator('details.trigger-trust').filter({hasText:disclosure.profileName});
        await details.locator('summary').click();await contains(ctx,details,disclosure.command,'publish_trust_disclosed');
      }
      need(f.targetDisclosure,'target_disclosure_fixture_missing');
      await contains(ctx,page.locator('.trigger-review'),f.targetDisclosure,'publish_target_disclosed');
    },
    async ctx => {
      const {page}=ctx.state.b,f=ctx.state.f;need(f.faults?.holdPublicationResponse,'publication_uncertainty_control_missing');
      const hold=await f.faults.holdPublicationResponse({triggerId:f.trigger.id});ctx.defer('release_publication_hold',()=>hold.restore());
      await page.getByRole('checkbox',{name:/^Publish revision /}).check();
      const pending=capture(page,'POST',triggerPath(ctx)+'/publish',()=>button(page,/^Publish revision /).click());
      const settled=pending.then(value=>({value}),error=>({error}));
      await hold.waitCaptured();
      ctx.assert(!new URL(page.url()).pathname.endsWith('/embed'),'uncertain_publication_not_presented_as_complete');
      await hold.release();const result=await settled;if(result.error)throw result.error;
      ctx.assert([200,201].includes(result.value.status),'publication_response_accepted');
      ctx.state.revision=result.value.body;
    },
    async ctx => {
      const {page}=ctx.state.b;await visible(ctx,page.getByRole('heading',{name:/^Embed revision /}),'published_embed_visible');
      const doc=await readTrigger(ctx);ctx.state.published=doc;
      ctx.assert(doc.availability==='published'&&doc.revision_count===ctx.state.revision.revision,'published_revision_exact');
      await peerReload(ctx);await visible(ctx,page.getByRole('heading',{name:'Embed revision '+doc.revision_count,exact:true}),'published_read_other_am');
    },
    async ctx => {
      const doc=await readTrigger(ctx),page=await api(ctx,triggerPath(ctx)+'/revisions',{am:ctx.am});
      ctx.assert(doc.revision_count===ctx.state.published.revision_count&&page.body.length===1,'one_immutable_publication');
      ctx.assert(JSON.stringify(page.body[0].content)===JSON.stringify(ctx.state.published.draft),'published_content_matches_review');
    },
  ]),
  define('triggers.placements',[
    async ctx => {
      const {page,f}=await wizard(ctx,'trigger-published');
      await next(ctx,'When someone clicks again');await next(ctx,'What it says');await next(ctx,'Where it lives');
      await page.getByLabel('Source label',{exact:true}).fill(f.placement.label);await page.getByLabel('Source URL',{exact:true}).fill(f.placement.url);
      const result=await capture(page,'POST',triggerPath(ctx)+'/placements',()=>button(page,'Add source URL').click());
      ctx.assert([200,201].includes(result.status),'placement_created');ctx.state.placement=result.body;
    },
    async ctx => {
      const f=ctx.state.f,p=ctx.state.placement;
      ctx.assert(p.source_url===f.placement.url&&p.label===f.placement.label&&!p.retired_at,'placement_exact_source');
      await contains(ctx,ctx.state.b.page.getByRole('region',{name:'Source URLs'}),p.id,'placement_id_visible');
    },
    async ctx => {
      const {page}=ctx.state.b,f=ctx.state.f,p=ctx.state.placement;
      await button(page,'Remove '+f.placement.label).click();await button(page,'Remove placement').click();
      await waitFor(async()=>Boolean((await readTrigger(ctx)).placements.find(x=>x.id===p.id)?.retired_at),'placement_retired',30000,2000);
      await peerReload(ctx);await unavailable(ctx,p.id);
    },
    async ctx => {
      const doc=await readTrigger(ctx),f=ctx.state.f;
      ctx.assert(doc.availability==='published'&&doc.placements.some(p=>p.id===f.otherPlacement.id&&!p.retired_at),'other_placement_still_live');
      const journal=await f.journal.launches({triggerId:f.trigger.id,placementId:ctx.state.placement.id});
      ctx.assert(journal.length===0,'retired_placement_no_new_launch');
    },
  ]),
  define('triggers.embed-stats',[
    async ctx => {
      const {page,origin,f}=await wizard(ctx,'trigger-published');
      await page.goto(origin+'/triggers/manage/'+enc(f.trigger.id)+'/embed');
      await pick(page,'Placement',f.placement.label);
      await visible(ctx,page.getByRole('heading',{name:'Embed revision '+f.trigger.revision_count,exact:true}),'exact_embed_revision');
    },
    async ctx => {
      const images=ctx.state.b.page.locator('.trigger-badge-previews img');
      ctx.assert(await images.count()===2,'both_badge_themes');
      for(const image of await images.all()) {
        await waitFor(()=>image.evaluate(n=>n.complete&&n.naturalWidth>0),'badge_decoded');
        const url=new URL(await image.getAttribute('src'));
        ctx.assert(url.pathname.includes(enc(ctx.state.f.trigger.public_id)),'badge_exact_trigger');
      }
    },
    async ctx => {
      const {page}=ctx.state.b;
      for(const label of ['README or Markdown','Web page','Page with JavaScript','React app','Just the link']) {
        await page.getByRole('tab',{name:label,exact:true}).click();
        for(const copy of await page.getByRole('tabpanel').getByRole('button',{name:/^Copy /}).all()) {
          const name=(await copy.getAttribute('aria-label')).slice(5);
          const expected=await page.locator('pre').filter({has:page.locator('code')}).evaluateAll((nodes,label)=>nodes.find(n=>n.getAttribute('aria-label')===label)?.textContent,name);
          await copy.click();const actual=await page.evaluate(()=>navigator.clipboard.readText());
          ctx.assert(actual===expected&&actual.length>0,'clipboard_matches_selected_descriptor');
          await ctx.observe('snippet_digest',async()=>({format:name,sha256:hash(actual)}));
        }
      }
    },
    async ctx => {
      const f=ctx.state.f,{page}=ctx.state.b;need(f.faults?.failStats,'stats_fault_control_missing');
      const fault=await f.faults.failStats({triggerId:f.trigger.id});ctx.defer('restore_statistics',()=>fault.restore());
      await peerReload(ctx);await contains(ctx,page.locator('[aria-label="Trigger statistics"]'),'Unavailable','statistics_failure_visible');
      await fault.restore();await button(page,'Retry statistics').click();
      await waitFor(async()=>!(await page.locator('[aria-label="Trigger statistics"]').innerText()).includes('Unavailable'),'statistics_recovered');
      ctx.assert(!(await page.locator('[aria-label="Trigger statistics"]').innerText()).includes('Loading'),'statistics_completed');
    },
    async ctx => {
      const doc=await readTrigger(ctx);ctx.assert(doc.id===ctx.state.f.trigger.id&&doc.revision_count===ctx.state.f.trigger.revision_count,'embed_revision_unchanged');
      const stats=(await api(ctx,triggerPath(ctx)+'/stats',{am:ctx.am})).body;
      await contains(ctx,ctx.state.b.page.locator('[aria-label="Trigger statistics"]'),String(stats.totals.launches_total),'statistics_exact_projection');
    },
  ]),
  ...launches,
];
