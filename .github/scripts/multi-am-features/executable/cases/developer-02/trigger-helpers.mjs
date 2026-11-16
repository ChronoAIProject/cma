import {setup,button,visible,contains,api,enc,need} from './helpers.mjs';
export const triggerPath = ctx => '/api/v1/triggers/' + enc(ctx.state.f.trigger.id);
export const readTrigger = async ctx => (await api(ctx,triggerPath(ctx),{am:ctx.am})).body;
export async function wizard(ctx,scenario='trigger-draft') {
  const b = await setup(ctx,scenario);
  need(b.f.trigger?.id && b.f.trigger.public_id,'owned_trigger_fixture_missing');
  await b.page.goto(b.origin + '/triggers/manage/' + enc(b.f.trigger.id));
  await visible(ctx,b.page.getByRole('heading',{name:'What it opens',exact:true}),'trigger_target_step');
  return b;
}
export async function next(ctx,label) {
  await button(ctx.state.b.page,'Next').click();
  await visible(ctx,ctx.state.b.page.getByRole('heading',{name:label,exact:true}),'trigger_step_advanced');
}
export async function review(ctx) {
  for (const label of ['When someone clicks again','What it says','Where it lives','Publish']) await next(ctx,label);
}
export async function pick(page,label,value) {
  await button(page,label).click(); await page.getByRole('menuitemradio',{name:value,exact:true}).click();
}
export async function unavailable(ctx,placement) {
  const {page,origin}=ctx.state.b,f=ctx.state.f;
  await page.goto(origin+'/t/'+enc(f.trigger.public_id)+(placement?'/'+enc(placement):''));
  await visible(ctx,page.getByText('This agent is no longer available from this page.',{exact:true}),'public_trigger_unavailable');
  const buttonNode=button(page,f.buttonLabel ?? 'Open in CMA');
  ctx.assert(await buttonNode.count()===0 || await buttonNode.isDisabled(),'unavailable_launch_not_enabled');
}
export async function exactDraft(ctx,doc) {
  ctx.assert(doc.id===ctx.state.f.trigger.id && doc.availability==='draft' && doc.revision_count===0,'draft_not_published');
  await contains(ctx,ctx.state.b.page.locator('main'),doc.draft.presentation.title,'draft_title_visible');
}
