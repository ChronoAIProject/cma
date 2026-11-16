import test from 'node:test';
import assert from 'node:assert/strict';
import {cases} from './usage.mjs';
import {matchesRender} from './usage-render.mjs';
const labels={'1h':'Last hour','24h':'Last 24 hours','7d':'Last 7 days','30d':'Last 30 days'};
const allocation={cpu_core_hours:1,memory_gib_hours:2,disk_gib_hours:3};
function fixture(suffix,{clickError,renderError,renderGate,stale=false,priorCaptureOnly=false,retainedPage=false}={}){
  const events=[],timings=[];let measuring=false,reportRead=0,ctx;
  const custom=suffix==='custom-filter',range=custom?'custom':suffix==='refresh'?'24h':suffix;
  const duration={'1h':3600,'24h':86400,'7d':604800,'30d':2592000,custom:86400}[range];
  const f={principal:'fixture-user',sandboxId:'sandbox-fixture',agentId:'agent-fixture',
    from:new Date(100000).toISOString(),to:new Date(86500000).toISOString()};
  const report={report_id:'report-fixture',status:'ready',report_complete:true,expires_at:Date.now()/1000+600,
    scope:{principal:f.principal,admin:false},summary:{allocated_period:allocation},
    query:{from:100,to:100+duration,filters:{owner:null,runtime_version:null,statuses:[],
      sandbox_id:custom?f.sandboxId:null,agent_id:custom?f.agentId:null}}};
  const items=[{sandbox_id:f.sandboxId,name:'Fixture Sandbox',owner:{subject:f.principal},allocated_period:allocation}];
  const selected={report,page:{body:{items}}};
  const snapshot={state:'committed',busy:false,summary:['1','2','3'],rows:[{name:'Fixture Sandbox',values:['1','2','3']}],empty:false};
  const observations=[{status:200,sequence:10,path:'/api/v1/usage/metrics/view',request_id:'view-fixture',elapsed_ms:3,
    body:{report,stale,generated_at:Date.now()/1000}},
    {status:200,sequence:11,path:'/api/v1/usage/metrics/reports/report-fixture/sandboxes',request_id:'rows-fixture',elapsed_ms:4,body:{report_id:report.report_id,items}}];
  const locator=name=>({
    click:async()=>{
      const intent=['Apply usage filters','Refresh usage'].includes(name);
      events.push({kind:intent?'intent':'selection',measuring});
      if(intent){assert.equal(measuring,true);if(clickError)throw clickError;}
    },
    waitFor:async()=>{},isVisible:async()=>true,isEnabled:async()=>true,
    innerText:async()=>labels[range]??'',
    getAttribute:async name=>name==='data-usage-requested-range'?range:'committed',
    evaluate:async()=>{
      events.push({kind:'render_wait',measuring});assert.equal(measuring,true);
      if(renderGate)await renderGate;if(renderError)throw renderError;
      events.push({kind:'render_complete',measuring});return snapshot;
    },
  });
  const page={getByRole:(_role,{name})=>locator(name),getByLabel:name=>locator(name)};
  let traceReads=0;
  const trace={mark:()=>9,read:async()=>{events.push({kind:'response_read',measuring});
    if(priorCaptureOnly){if(traceReads++)throw Error('no_post_intent_report');
      return observations.map(row=>({...row,sequence:8}));}
    return retainedPage?observations.map(row=>row.path.endsWith('/sandboxes')?{...row,sequence:8}:row):observations;}};
  const runSteps=new Set(custom?['step_3']:['step_2','step_3']);
  ctx={am:'a',state:{f,b:{page,trace},range,filters:custom?{from:100,to:86500,sandboxId:f.sandboxId,agentId:f.agentId}:{},sessions:[]},
    step:async(id,operation)=>{if(runSteps.has(id))return operation();events.push({kind:'unmeasured_step',id,measuring});},
    assert:(ok,code)=>assert.ok(ok,code),
    request:async options=>{assert.equal(measuring,false,'cross-AM verification excluded');assert.equal(options.am,'b');reportRead++;
      events.push({kind:'other_am',measuring});return {timing:{am:'b'},body:report};},
    measureAction:async(boundary,operation)=>{
      assert.equal(measuring,false);assert.equal(boundary,'usage_to_complete_render');measuring=true;
      try{const result=await operation();timings.push({boundary,status:'PASS'});return result;}
      catch(error){timings.push({boundary,status:'FAIL'});throw error;}
      finally{measuring=false;events.push({kind:'timing_closed',measuring});}
    },
  };
  return {ctx,events,timings,selected,snapshot,reads:()=>reportRead};
}
for(const suffix of ['1h','24h','7d','30d','custom-filter','refresh']){
  test(`${suffix}: actual case measures intent through render and excludes other-AM verification`,async()=>{
    const f=fixture(suffix);await cases.find(c=>c.id===`C275:cma-user.usage.${suffix}`).run(f.ctx);
    assert.deepEqual(f.timings,[{boundary:'usage_to_complete_render',status:'PASS'}]);
    assert.equal(f.events.filter(e=>e.kind==='intent').length,1);assert.equal(f.reads(),1);
    assert.ok(f.events.filter(e=>['selection','unmeasured_step','other_am'].includes(e.kind)).every(e=>!e.measuring));
    const order=f.events.map(e=>e.kind);
    assert.ok(order.indexOf('intent')<order.indexOf('render_complete'));
    assert.ok(order.indexOf('render_complete')<order.indexOf('timing_closed'));
    assert.ok(order.indexOf('timing_closed')<order.indexOf('other_am'));
  });
}
test('completed HTTP bodies do not end the timer before the actual DOM render',async()=>{
  let release;const gate=new Promise(resolve=>{release=resolve;});const f=fixture('24h',{renderGate:gate});
  const running=cases.find(c=>c.id==='C275:cma-user.usage.24h').run(f.ctx);
  while(!f.events.some(e=>e.kind==='render_wait'))await new Promise(resolve=>setImmediate(resolve));
  assert.equal(f.timings.length,0);assert.equal(f.reads(),0);
  release();await running;assert.equal(f.timings[0].status,'PASS');
});
for(const [name,options] of [['click',{clickError:Error('intent_failed')}],['render',{renderError:Error('render_failed')}],['stale',{stale:true}]]){
  test(`${name} failure stays inside the measured attempt and is not retried`,async()=>{
    const f=fixture('refresh',options);
    await assert.rejects(cases.find(c=>c.id==='C275:cma-user.usage.refresh').run(f.ctx));
    assert.deepEqual(f.timings,[{boundary:'usage_to_complete_render',status:'FAIL'}]);
    assert.equal(f.events.filter(e=>e.kind==='intent').length,1);assert.equal(f.reads(),0);
  });
}
test('loading, previous summary, missing rows and preview cannot satisfy complete render',()=>{
  const {selected,snapshot}=fixture('24h');assert.equal(matchesRender(snapshot,selected),true);
  for(const delta of [{busy:true},{state:'preview'},{summary:['0','0','0']},{rows:[]},
    {rows:[{name:'Other Sandbox',values:['1','2','3']}]},{rows:[{name:'Fixture Sandbox',values:['9','2','3']}]}])
    assert.equal(matchesRender({...snapshot,...delta},selected),false);
  selected.page.body.items=[];
  assert.equal(matchesRender({...snapshot,rows:[],empty:false},selected),false);
  assert.equal(matchesRender({...snapshot,rows:[],empty:true},selected),true);
});

test('a pre-click completed report cannot end the measured Apply action',async()=>{
  const f=fixture('24h',{priorCaptureOnly:true});
  await assert.rejects(cases.find(c=>c.id==='C275:cma-user.usage.24h').run(f.ctx),/no_post_intent_report/);
  assert.deepEqual(f.timings,[{boundary:'usage_to_complete_render',status:'FAIL'}]);
  assert.equal(f.events.filter(e=>e.kind==='intent').length,1);
  assert.equal(f.events.filter(e=>e.kind==='render_complete').length,0);
  assert.equal(f.reads(),0);
});

test('a post-click report may reuse its immutable page when the exact contents are rendered',async()=>{
  const f=fixture('24h',{retainedPage:true});
  await cases.find(c=>c.id==='C275:cma-user.usage.24h').run(f.ctx);
  assert.deepEqual(f.timings,[{boundary:'usage_to_complete_render',status:'PASS'}]);
  assert.equal(f.reads(),1);
});

test('the 24h case opens a different range before its measured Apply instead of the default no-op',async()=>{
  const paths=[];
  const f=Object.fromEntries(['principal','sandboxId','sandboxName','agentId','agentName','foreignSandboxId',
    'from','to','fromLocal','toLocal','invalidToLocal'].map(field=>[field,'unit-'+field]));
  Object.assign(f,{local:true,run_owned:true,webSource:'a'.repeat(40),cleanup:async()=>({complete:true,owned_only:true})});
  const page={on(){},off(){},goto:async path=>paths.push(path),
    getByRole:()=>({waitFor:async()=>{},isVisible:async()=>true})};
  const ctx={am:'a',state:{},defer(){},assert:(ok,code)=>assert.ok(ok,code),
    fixture:name=>name==='user-fixtures'?{take:async()=>f}
      :{open:async()=>({page,origin:'http://unit.invalid',close:async()=>{}})},
    step:async(id,operation)=>{if(id==='step_1')await operation();}};
  await cases.find(c=>c.id==='C275:cma-user.usage.24h').run(ctx);
  assert.deepEqual(paths,['http://unit.invalid/usage?range=1h']);
  assert.equal(ctx.state.range,'1h');
});
