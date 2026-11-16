import {waitFor} from './helpers.mjs';
const fields=['cpu_core_hours','memory_gib_hours','disk_gib_hours'];
const quantity=value=>value==null?'Unavailable':typeof value==='string'?BigInt(value).toLocaleString('en-SG'):
  new Intl.NumberFormat('en-SG',{maximumFractionDigits:3}).format(value);
const values=allocation=>fields.map(field=>quantity(allocation[field]));
export function matchesRender(snapshot,selected){
  if(!snapshot||snapshot.busy||snapshot.state!=='committed')return false;
  if(JSON.stringify(snapshot.summary)!==JSON.stringify(values(selected.report.summary.allocated_period)))return false;
  const expected=selected.page.body.items.map(item=>({name:item.name||'Unnamed Sandbox',values:values(item.allocated_period)}));
  if(JSON.stringify(snapshot.rows)!==JSON.stringify(expected))return false;
  return expected.length>0||snapshot.empty===true;
}
export async function completeRender(ctx,selected){
  const dashboard=ctx.state.b.page.getByLabel('Your usage resources',{exact:true});
  // A completed response is not a completed render. Read the committed summary,
  // actual table rows and loading state in one DOM snapshot without UI actions.
  let snapshot;
  await waitFor(async()=>matchesRender(snapshot=await dashboard.evaluate(root=>{
    const table=root.querySelector('table[aria-label="Sandboxes and allocated usage"],table[aria-label="Sandboxes and measured usage"]');
    const summary=root.querySelector('[aria-label="Allocated resource-hours"]');
    if(!table||!summary)return null;
    return {state:root.getAttribute('data-usage-state'),busy:table.getAttribute('aria-busy')==='true',
      summary:[...summary.querySelectorAll('.usage-metric-totals dd')].map(node=>node.textContent.trim()),
      rows:[...table.querySelectorAll('tbody tr')].filter(row=>row.querySelector('.usage-inventory-name__button')).map(row=>({
        name:row.querySelector('.usage-inventory-name__button').textContent.trim(),
        values:[...row.querySelectorAll('.usage-inventory-quantity')].map(node=>node.textContent.trim())})),
      empty:table.textContent.includes('No Sandboxes match this selection.')};
  }),selected),'usage_complete_render_missing',240000);
  ctx.assert(matchesRender(snapshot,selected),'requested_summary_and_rows_rendered');
}
