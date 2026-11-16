import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';
import {totalsFor} from './usage.mjs';

test('usage flows traverse both pages and reject an omitted allocation',async()=>{
  const ledger = ['sbx_first','sbx_second'].map((id,index)=>({sandbox_id:id,owner:'owner',name:id,status:'retired',
    allocation:{running:false,cpu_millis:0,memory_mib:0,disk_gib:0},created_at:1,tracking_started_at:1,
    complete_lifecycle:true,intervals:[{running:true,started_at:1,ended_at:3601,cpu_millis:1000*(index+1),memory_mib:1024,disk_gib:3}]}));
  const views = ledger.map(seed=>({...seed,as_of:4000,totals:totalsFor(seed.intervals)}));
  const summary = {measurement:'allocated_resource_time',sandbox_count:2,running_count:0,incomplete_lifecycle_count:0,
    totals:totalsFor(ledger.flatMap(seed=>seed.intervals))};
  async function run(number,omit=false) {
    return execute(number,'usage',{ledger},async input=>{
      const url = new URL(input.path,'http://localhost');
      if(url.pathname==='/api/v1/usage') return {status:200,body:summary};
      if(url.pathname==='/api/v1/usage/sandboxes') {
        if(input.actor==='outsider') return {status:200,body:{items:[],next:null}};
        const index = url.searchParams.has('cursor')?1:0;
        if(!url.searchParams.has('limit')) return {status:200,body:{items:views,next:null}};
        return {status:200,body:{items:omit&&index===1?[]:[views[index]],next:index===0?'second':null}};
      }
      if(input.actor==='outsider') return {status:404,body:{code:'sandbox_not_found'}};
      return {status:200,body:views.find(value=>input.path.endsWith('/'+value.sandbox_id))};
    });
  }
  for(const number of [69,70,71]) pass(await run(number));
  const missing = await run(70,true);
  assert.equal(missing.result.status,'FAIL');
  assert.equal(missing.result.primary_failure,'usage_no_missing_sandbox');
  assert.equal(missing.cleanups.length,1);
});
