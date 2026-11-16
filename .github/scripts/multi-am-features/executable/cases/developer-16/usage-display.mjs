import {button,contains,pick,visible,waitFor,hash,api,enc} from './helpers.mjs';
const paths='/api/v1/usage/metrics';
const quantity=value=>value==null?'Unavailable':typeof value==='string'?BigInt(value).toLocaleString('en-SG'):new Intl.NumberFormat('en-SG',{maximumFractionDigits:3}).format(value);
const fields={CPU:'cpu_core_hours',Memory:'memory_gib_hours',Disk:'disk_gib_hours'};
export async function summary(ctx){
 const {page}=ctx.state.b,report=ctx.state.report;
 const summary=page.getByRole('region',{name:'Allocated resource-hours',exact:true}).first();
 await visible(ctx,summary,'allocated_summary');
 for(const field of Object.values(fields))await contains(ctx,summary,quantity(report.summary.allocated_period[field]),'summary_value_from_report');
 const disclosure=summary.locator('summary');await disclosure.click();
 await contains(ctx,summary,'Missing history is excluded','missing_history_not_zero');
 const measured=page.getByText(/^Measured through /);
 if(await measured.count()){
  ctx.assert(report.measurement_cutoff!=null&&typeof report.source_health==='string','measurement_metadata_retained');
  await contains(ctx,measured,report.source_health,'measurement_source_health_displayed');
 }else ctx.assert(await page.getByRole('table',{name:'Sandboxes and allocated usage',exact:true}).count()===1,'allocation_only_view_explicit');
}
export async function charts(ctx){
 const {page}=ctx.state.b,items=ctx.state.selected.page.body.items;
 ctx.assert(items.length>0&&new Set(items.map(i=>i.sandbox_id)).size===items.length,'chart_rows_unique');
 for(const [resource,field] of Object.entries(fields)){
  await pick(page,'Allocation chart resource',resource);
  const disclosure=page.locator('summary').filter({hasText:'View allocation chart data'});await disclosure.click();
  const table=page.getByRole('table',{name:'Allocation chart data',exact:true});await visible(ctx,table,'allocation_chart_table');
  ctx.assert(await table.locator('tbody tr').count()===items.length,'chart_contains_every_page_row');
  for(const item of items){
   const row=table.locator('tr').filter({has:page.locator(`span[title="${item.sandbox_id}"]`)});
   await contains(ctx,row,quantity(item.allocated_period[field]),'chart_value_exact_report_row');
   await contains(ctx,row,item.allocated_period.complete?'Complete':'Partial','chart_history_coverage');
  }
  await disclosure.click();
 }
}
async function currentPage(ctx,since,predicate){
 let response;await waitFor(async()=>{const rows=await ctx.state.b.trace.read({since,pathPrefix:paths+'/reports/'+enc(ctx.state.report.report_id)+'/sandboxes'});
  response=rows.filter(r=>r.status===200&&Array.isArray(r.body?.items)&&predicate(r)).at(-1);return Boolean(response);},'usage_page_response_missing');
 ctx.assert(response.body.report_id===ctx.state.report.report_id,'page_exact_report');return response.body;
}
export async function pagination(ctx){
 const {page,trace}=ctx.state.b;
 let mark=trace.mark();
 if((await button(page,'Sort usage inventory').innerText()).includes('Sandbox name')){
  await pick(page,'Sort usage inventory','Last activity');await currentPage(ctx,mark,r=>new URLSearchParams(r.query).get('sort')==='last_activity_at');mark=trace.mark();
 }
 await pick(page,'Sort usage inventory','Sandbox name');
 const first=await currentPage(ctx,mark,r=>new URLSearchParams(r.query).get('sort')==='name');
 ctx.assert(first.total_count>50&&typeof first.next==='string','actual_second_page_available');
 const inventory=page.getByRole('table',{name:/Sandboxes and (allocated|measured) usage/});
 await waitFor(()=>inventory.locator('tbody tr button.usage-inventory-name__button').count().then(n=>n===first.items.length),'usage_rows_not_rendered');
 const names=await inventory.locator('tbody tr button.usage-inventory-name__button').allTextContents();
 ctx.assert(names.length===first.items.length&&names.every((name,i)=>name.trim()===(first.items[i].name||'Unnamed Sandbox')),'rendered_inventory_order_matches_report');
 mark=trace.mark();await button(page,'Next usage page').click();const second=await currentPage(ctx,mark,r=>new URLSearchParams(r.query).get('cursor')===first.next);
 await contains(ctx,page.getByRole('region',{name:'Usage inventory',exact:true}),'page 2','usage_pagination');
 ctx.assert(second.items.length>0&&second.items.every(i=>!first.items.some(j=>j.sandbox_id===i.sandbox_id)),'pages_do_not_repeat_rows');
 mark=trace.mark();await button(page,'Previous usage page').click();await currentPage(ctx,mark,r=>!new URLSearchParams(r.query).has('cursor'));
 await button(page,ctx.state.f.sandboxName).click();await visible(ctx,page.getByRole('region',{name:'Sandbox usage details',exact:true}),'usage_detail');
 ctx.state.pages=[first,second];
}
export async function displayIdentity(ctx){
 const f=ctx.state.f,r=ctx.state.report,rows=ctx.state.pages.flatMap(p=>p.items),page=ctx.state.b.page;
 const partial=rows.filter(row=>row.allocated_period?.complete===false);
 ctx.assert(partial.length>0,'actual_partial_allocation_history_required');
 const details=await api(ctx,`${paths}/reports/${enc(r.report_id)}/sandboxes/${enc(f.sandboxId)}`);
 ctx.assert(details.body.sandbox_id===f.sandboxId,'detail_selected_sandbox');
 const measured=await page.getByRole('table',{name:'Sandboxes and measured usage',exact:true}).count()>0;
 if(measured){
  ctx.assert(rows.some(row=>row.measured?.cpu?.mean_cores===null),'actual_missing_measurements_required');
  await contains(ctx,page.getByRole('region',{name:'Usage inventory',exact:true}),'Measured: Unavailable','missing_measurements_not_zero');
 }else ctx.assert(await page.getByText('Measured: 0',{exact:true}).count()===0,'allocation_view_does_not_invent_measurements');
 await ctx.observe('usage_chart_identity',async()=>({report_sha256:hash(r.report_id),row_ids:rows.map(row=>hash(row.sandbox_id)),partial_count:partial.length,measurements_shown:measured}));
}
export async function historyPage(ctx,next=false){
 const {page,trace}=ctx.state.b,path=`/api/v1/usage/sandboxes/${enc(ctx.state.f.sandboxId)}/events`,mark=trace.mark();
 await button(page,next?'Next history page':'Lifecycle history').click();
 let got;await waitFor(async()=>{got=(await trace.read({since:mark,pathPrefix:path})).filter(r=>r.status===200&&Array.isArray(r.body?.items)).at(-1);return Boolean(got);},'history_page_missing');
 ctx.assert(got.body.items.length>0&&got.body.items.every(e=>e.ended_at&&e.transaction_id&&e.allocation),'actual_history_fields');
 const table=page.getByRole('table',{name:'Usage intervals',exact:true});await visible(ctx,table,'history_intervals_visible');
 await waitFor(()=>table.locator('tbody tr').count().then(n=>n===got.body.items.length),'history_rows_not_rendered');
 ctx.assert(await table.locator('tbody tr').count()===got.body.items.length,'history_page_rows_exact');
 for(const event of got.body.items){
  const when=new Date(Number(event.ended_at)*1000).toLocaleString('en-SG',{timeZone:'Asia/Singapore'});
  await contains(ctx,table,when,'history_actual_time');
 }
 if(next){ctx.assert(new URLSearchParams(got.query).get('cursor')===ctx.state.firstEvents.next,'history_cursor_exact');
  ctx.assert(got.body.items.every(e=>!ctx.state.firstEvents.items.some(p=>p.transaction_id===e.transaction_id)),'history_pages_distinct');
 }else{ctx.assert(Boolean(got.body.next),'history_second_page_available');ctx.state.firstEvents=got.body;}
 ctx.state.lastEvents=got.body;
}
