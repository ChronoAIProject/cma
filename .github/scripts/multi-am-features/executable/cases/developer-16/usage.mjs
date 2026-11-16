import {define,setup,api,button,pick,visible,contains,absent,attribute,waitFor,enc,hash} from './helpers.mjs';
import {selectReport,refreshIdentity} from './usage-proof.mjs';
const paths = '/api/v1/usage/metrics';
const labels = {'1h':'Last hour','24h':'Last 24 hours','7d':'Last 7 days','30d':'Last 30 days'};
async function enter(ctx) {
  const {page,origin,f} = await setup(ctx,'usage');
  ctx.state.range='24h'; ctx.state.started=performance.now();
  await page.goto(origin+'/usage');
  await visible(ctx,page.getByRole('dialog',{name:'Your usage',exact:true}),'usage_dialog_visible');
  ctx.assert(typeof f.principal === 'string' && f.principal.length > 0,'usage_principal_bound');

}
async function completed(ctx) {
  const {page,trace} = ctx.state.b, f=ctx.state.f;
  const dashboard=page.getByLabel('Your usage resources',{exact:true});
  await attribute(ctx,dashboard,'data-usage-requested-range',ctx.state.range??'24h','requested_range_visible');
  await attribute(ctx,dashboard,'data-usage-state','committed','requested_report_committed');
  await visible(ctx,page.getByRole('heading',{name:'Allocation overview',exact:true}),'usage_summary_visible');
  await visible(ctx,page.getByRole('table',{name:/Sandboxes and (allocated|measured) usage/}),'usage_rows_visible');
  const wanted={principal:f.principal,range:ctx.state.range??'24h',...ctx.state.filters};
  let selected;
  await waitFor(async()=>Boolean(selected=selectReport(await trace.read({pathPrefix:paths}),wanted,ctx.state.selectionMark??0)),
    'matching_visible_report_missing',240000);
  ctx.assert(Boolean(selected),'complete_personal_report_required');
  ctx.assert(selected.page.body.items.every(item=>!item.owner||item.owner.subject===f.principal),'report_rows_authorized');
  ctx.state.report=selected.report;ctx.state.selected=selected;ctx.state.completedAt=performance.now();
  // Independently read the exact observed report through the other live ingress.
  const checked=await api(ctx,paths+'/reports/'+enc(selected.report.report_id));
  ctx.assert(checked.body.report_id===selected.report.report_id&&checked.body.scope?.principal===f.principal
    &&checked.body.report_complete===true&&checked.body.status==='ready','other_am_exact_report');
  ctx.assert(JSON.stringify(checked.body.query)===JSON.stringify(selected.report.query)
    &&JSON.stringify(checked.body.summary)===JSON.stringify(selected.report.summary),'other_am_same_report_content');
  return selected.report;
}
async function privateReceipt(ctx) {
  const r=ctx.state.report, selected=ctx.state.selected;
  const observations=await ctx.state.b.trace.read({pathPrefix:paths});
  const evidence=observations.filter(x=>(x.body?.report?.report_id??x.body?.report_id)===r.report_id);
  ctx.assert(evidence.length>0&&evidence.every(x=>Number.isFinite(x.elapsed_ms)&&x.request_id),'usage_step_timings_required');
  const view=selected.response.body?.report ? selected.response.body : null;
  if(view)ctx.assert(view.stale===false&&Number.isFinite(view.generated_at),'usage_view_not_stale');
  const completion=ctx.state.completedAt-ctx.state.started;
  await ctx.observe('usage_report',async()=>({report_sha256:hash(r.report_id),scope_sha256:hash(r.scope),
    query:r.query,effective_range:r.effective_range,as_of:r.as_of,
    freshness:view?{kind:'view',generated_at:view.generated_at,stale:view.stale,refresh:view.refresh}
      :{kind:'report',as_of:r.as_of,admitted_here:evidence.some(x=>x.method==='POST'),cache_state:'NOT_EXPOSED'},
    timings:evidence.map(x=>({request_id:x.request_id,elapsed_ms:x.elapsed_ms})),
    completion_ms:completion,target_ms:2000,performance_verdict:completion<=2000?'PASS':'FAIL'}));
}
const rangeCases = Object.entries(labels).map(([range,label])=>define('usage.'+range,[
  enter,
  async ctx=>{const {page}=ctx.state.b;ctx.state.range=range;ctx.state.selectionMark=ctx.state.b.trace.mark();ctx.state.started=performance.now();await pick(page,'Usage time range',label);await button(page,'Apply usage filters').click();
    await contains(ctx,button(page,'Usage time range'),label,'selected_usage_range');},
  completed,
  privateReceipt,
  async ctx=>{const r=ctx.state.report;const seconds={'1h':3600,'24h':86400,'7d':604800,'30d':2592000}[range];
    ctx.assert(Math.abs(r.query.to-r.query.from-seconds)<=1,'completed_requested_range');
    await absent(ctx,ctx.state.b.page.getByText(/refresh deferred|not last|allocation failed/i),'no_stale_or_failed_report');},
]));
export const cases = [...rangeCases,
 define('usage.custom-filter',[
  async ctx=>{await enter(ctx);const {page}=ctx.state.b,f=ctx.state.f;ctx.state.range='custom';ctx.state.filters={from:Date.parse(f.from)/1000,to:Date.parse(f.to)/1000};await pick(page,'Usage time range','Custom (up to 30 days)');
    await page.getByLabel('Usage range from',{exact:true}).fill(f.fromLocal);await page.getByLabel('Usage range to',{exact:true}).fill(f.toLocal);
    ctx.assert(Date.parse(f.to)-Date.parse(f.from)<=30*86400000&&Date.parse(f.to)>Date.parse(f.from),'custom_interval_valid');},
  async ctx=>{const {page}=ctx.state.b,f=ctx.state.f;await page.getByLabel('Usage Sandbox search',{exact:true}).fill(f.sandboxName);
    await button(page,/^Sandbox:/).click();await page.getByLabel('Search sandbox choices',{exact:true}).fill(f.sandboxName);
    await button(page,'Search sandbox choices').click();await pick(page,'Choose sandbox',`${f.sandboxName} (${f.sandboxId})`);
    ctx.state.filters.sandboxId=f.sandboxId;ctx.state.filters.search=f.sandboxName;await button(page,'Apply usage filters').click();await completed(ctx);
    await button(page,/^Agent:/).click();await pick(page,'Usage Agent',f.agentName);
    ctx.state.filters.agentId=f.agentId;await contains(ctx,button(page,/^Agent:/),f.agentId,'selected_owned_agent');},
  async ctx=>{await button(ctx.state.b.page,'Apply usage filters').click();const r=await completed(ctx),f=ctx.state.f;
    ctx.assert(r.query.filters.sandbox_id===f.sandboxId&&r.query.filters.agent_id===f.agentId,'usage_filters_exact');
    ctx.assert(r.query.from===Date.parse(f.from)/1000&&r.query.to===Date.parse(f.to)/1000,'custom_range_exact');},
  async ctx=>{const {page}=ctx.state.b;await page.getByLabel('Usage range to',{exact:true}).fill(ctx.state.f.invalidToLocal);
    await button(page,'Apply usage filters').click();await visible(ctx,page.getByRole('alert'),'invalid_range_visible');},
  async ctx=>{const {page}=ctx.state.b;for(const control of ['Usage runtime version','Usage Sandbox status'])
    await absent(ctx,page.getByLabel(control,{exact:true}),'personal_admin_filter_absent');
    await absent(ctx,button(page,/^Owner:/),'personal_owner_filter_absent');ctx.assert(ctx.state.report.scope.admin===false,'personal_scope_retained');},
 ]),
 define('usage.refresh',[
  async ctx=>{await enter(ctx);await completed(ctx);ctx.state.previous=ctx.state.report.report_id;},
  async ctx=>{ctx.state.refreshMark=ctx.state.b.trace.mark();ctx.state.selectionMark=ctx.state.refreshMark;ctx.state.started=performance.now();await button(ctx.state.b.page,'Refresh usage').click();
    await visible(ctx,button(ctx.state.b.page,'Refresh usage'),'refresh_action_visible');},
  async ctx=>{await waitFor(()=>button(ctx.state.b.page,'Refresh usage').isEnabled(),'refresh_deadline',240000);await completed(ctx);
    const all=await ctx.state.b.trace.read({pathPrefix:paths,since:ctx.state.refreshMark});
    ctx.assert(refreshIdentity(all,ctx.state.report.report_id),'refresh_same_single_admitted_report');},
  async ctx=>{await privateReceipt(ctx);ctx.assert(ctx.state.report.report_complete===true,'refresh_complete');},
 ]),
 define('usage.table-charts',[
  async ctx=>{await enter(ctx);await completed(ctx);},
  async ctx=>{await visible(ctx,ctx.state.b.page.getByRole('region',{name:'Allocated resource-hours',exact:true}).first(),'allocated_summary');
    await contains(ctx,ctx.state.b.page.getByLabel('Your usage resources',{exact:true}),'Missing history is excluded','missing_history_not_zero');},
  async ctx=>{const {page}=ctx.state.b;for(const resource of ['CPU','Memory','Disk']){
    await pick(page,'Allocation chart resource',resource);await page.locator('summary').filter({hasText:'View allocation chart data'}).click();
    await visible(ctx,page.getByRole('table',{name:'Allocation chart data',exact:true}),'allocation_chart_table');
    await page.locator('summary').filter({hasText:'View allocation chart data'}).click();}},
  async ctx=>{const {page}=ctx.state.b;await pick(page,'Sort usage inventory','Sandbox name');await button(page,'Next usage page').click();
    await contains(ctx,page.getByRole('region',{name:'Usage inventory',exact:true}),'page 2','usage_pagination');
    await button(page,'Previous usage page').click();await button(page,ctx.state.f.sandboxName).click();
    await visible(ctx,page.getByRole('region',{name:'Sandbox usage details',exact:true}),'usage_detail');},
  async ctx=>{const responses=await ctx.state.b.trace.read({pathPrefix:paths+'/reports/'});
    ctx.assert(responses.some(r=>r.status===200&&r.body?.report_id===ctx.state.report.report_id&&r.body?.items?.length),'charts_rows_same_report');
    const pages=responses.filter(r=>r.status===200&&r.body?.report_id===ctx.state.report.report_id&&Array.isArray(r.body.items));
    ctx.assert(pages.some(r=>r.body.total_count>50&&r.body.next),'actual_pagination_required');
    const partial=pages.flatMap(r=>r.body.items).filter(row=>row.allocated_period?.complete===false);
    ctx.assert(partial.length>0,'actual_partial_allocation_history_required');
    const rows=pages.flatMap(r=>r.body.items);
    ctx.assert(rows.some(row=>row.measured?.cpu?.mean_cores===null),'actual_missing_measurements_required');
    await ctx.observe('usage_chart_identity',async()=>({report_sha256:hash(ctx.state.report.report_id),row_ids:rows.map(row=>hash(row.sandbox_id)),partial_count:partial.length}));},
 ]),
 define('usage.event-history',[
  async ctx=>{await enter(ctx);await completed(ctx);await button(ctx.state.b.page,ctx.state.f.sandboxName).click();
    await visible(ctx,ctx.state.b.page.getByRole('region',{name:'Sandbox usage details',exact:true}),'usage_detail');},
  async ctx=>{await button(ctx.state.b.page,'Lifecycle history').click();await visible(ctx,ctx.state.b.page.getByRole('table',{name:'Usage intervals',exact:true}),'event_history');},
  async ctx=>{await button(ctx.state.b.page,'Next history page').click();await visible(ctx,button(ctx.state.b.page,'First history page'),'event_pagination');},
  async ctx=>{const f=ctx.state.f;const events=await api(ctx,`/api/v1/usage/sandboxes/${enc(f.sandboxId)}/events`);
    ctx.assert(events.body.items.length>0&&events.body.items.every(e=>e.ended_at&&e.transaction_id&&e.allocation),'actual_history_fields');
    await contains(ctx,ctx.state.b.page.getByRole('region',{name:'Sandbox usage history'}),f.sandboxId,'history_selected_sandbox');
    const denied=await api(ctx,`/api/v1/usage/sandboxes/${enc(f.foreignSandboxId)}/events`,{allowed:[403,404]});
    ctx.assert([403,404].includes(denied.status),'foreign_history_denied');},
 ]),
];
