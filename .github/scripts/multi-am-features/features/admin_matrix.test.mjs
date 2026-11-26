import test from 'node:test';
import assert from 'node:assert/strict';
import {AdminMatrix,validateFixture,ADMIN_PERMISSIONS} from './admin_matrix.mjs';
const SOURCE = '1'.repeat(40), MANIFEST = 'sha256:' + '2'.repeat(64);
const fixture=()=>({scope:'LOCAL_DISPOSABLE_TWO_AM',source:SOURCE,manifest:MANIFEST,database_disposable:true,
  single_matrix_writer:true,am_origins:{a:'http://127.0.0.1:8001',b:'http://127.0.0.1:8002'},
  admin_permissions:ADMIN_PERMISSIONS,ordinary_user_has_admin:false,run_id:'offline-test'});
function settingsServer({loseAck=false,stalePeer=false,foreignValue=false}={}) {
  let state={revision:1,settings:{retention:{agent_log_days:7},limits:{visitor_max_launches_per_day:3}}};
  const original=structuredClone(state),requests=[],rows=[];
  const request=async(role,am,method,path,{body})=>{
    requests.push({role,am,method,path,body:structuredClone(body)});
    if(method==='GET') return {status:200,body:structuredClone(stalePeer&&am==='b'?original:state)};
    if(body.expected_revision!==state.revision) return {status:409,body:{code:'conflict'}};
    state={revision:state.revision+1,settings:{...state.settings,retention:structuredClone(body.retention)}};
    if(loseAck && state.revision===2) {if(foreignValue)state.settings.retention={agent_log_days:99};throw Error('lost_ack');}
    return {status:200,body:structuredClone(state)};
  };
  return {matrix:new AdminMatrix({fixture:fixture(),request,record:row=>rows.push(row)}),requests,rows,state:()=>state};
}
test('refuses nonlocal endpoints, mismatched image, and bare admin permission before any callback',()=>{
  for(const change of [x=>x.am_origins.a='https://prod.example',x=>x.manifest='sha256:wrong',x=>x.admin_permissions=['cma:admin']]) {
    const f=fixture();change(f);assert.throws(()=>validateFixture(f));
  }
});
test('cross-AM settings mutation rejects stale revision and restores original state',async()=>{
  const x=settingsServer();await x.matrix.settingsRoundtrip(1,'retention',()=>({agent_log_days:8}));
  assert.equal(x.state().settings.retention.agent_log_days,7);assert.equal(x.rows[0].verdict,'PASS');
  assert.ok(x.requests.some(r=>r.am==='b'&&r.method==='PATCH'&&r.body.expected_revision===1));
});
test('lost successful-write acknowledgement produces FAIL, one readback and restoration without replay',async()=>{
  const x=settingsServer({loseAck:true});await assert.rejects(x.matrix.settingsRoundtrip(1,'retention',()=>({agent_log_days:8})),/lost_ack/);
  assert.equal(x.requests.filter(r=>r.method==='PATCH'&&r.body.retention.agent_log_days===8).length,1);
  assert.equal(x.state().settings.retention.agent_log_days,7);assert.equal(x.rows[0].verdict,'FAIL');
  assert.equal(x.rows[0].requests[1].error_code,'lost_ack');
});
test('stale cross-AM read cannot pass as restored or verified',async()=>{
  const x=settingsServer({stalePeer:true});await assert.rejects(x.matrix.settingsRoundtrip(1,'retention',()=>({agent_log_days:8})),/settings_restore_not_visible/);
  assert.equal(x.rows[0].verdict,'FAIL');
});
test('uncertain mutation never overwrites foreign state during restoration',async()=>{
  const x=settingsServer({loseAck:true,foreignValue:true});await assert.rejects(x.matrix.settingsRoundtrip(1,'retention',()=>({agent_log_days:8})),/settings_restore_foreign_value/);
  assert.equal(x.requests.filter(r=>r.method==='PATCH').length,1);assert.equal(x.state().settings.retention.agent_log_days,99);
});
test('admin denial is a failure when the ordinary caller receives protected data',async()=>{
  const rows=[];const m=new AdminMatrix({fixture:fixture(),record:r=>rows.push(r),request:async()=>({status:200,body:{items:[],next:null}})});
  await assert.rejects(m.denials(1),/unexpected_http_200/);assert.equal(rows[0].verdict,'FAIL');
});
test('failed admin request retains the actual ingress, response status and request ID',async()=>{
  const rows=[];
  const primary=Object.assign(Error('api_status_503'),{response:{status:503,
    timing:{am:'b',request_id:'private-request-id'},body:{code:'operator_service_unavailable'}}});
  const m=new AdminMatrix({fixture:fixture(),record:row=>rows.push(row),request:async()=>{throw primary;}});
  await assert.rejects(m.runCase('failed_read',1,()=>m.call('a','GET','/api/v1/admin/sandboxes')),
    error=>error===primary);
  assert.equal(rows[0].requests[0].am,'b');
  assert.equal(rows[0].requests[0].status,503);
  assert.equal(rows[0].requests[0].request_id,'private-request-id');
  assert.equal(rows[0].verdict,'FAIL');
});
test('admission lost stop acknowledgement reconciles once and restores through peer',async()=>{
  let state={revision:1,operator_stop:null,writes_available:true,history:[]};const calls=[];
  const m=new AdminMatrix({fixture:fixture(),record:()=>{},request:async(role,am,method,path,{body})=>{
    calls.push({am,method,path,body});
    if(method==='GET')return {status:200,body:structuredClone(state)};
    if(path.endsWith('/stop')){state={...state,revision:2,operator_stop:{reason:body.reason}};throw Error('lost_ack');}
    assert.equal(am,'b');assert.equal(body.expected_revision,2);state={...state,revision:3,operator_stop:null};return {status:200,body:structuredClone(state)};
  }});
  await assert.rejects(m.admission(1),/lost_ack/);assert.equal(state.operator_stop,null);
  assert.equal(calls.filter(x=>x.path.endsWith('/stop')).length,1);
});
test('real usage status field and report identity govern completion, not state guess',async()=>{
  const rows=[],calls=[];const m=new AdminMatrix({fixture:fixture(),record:r=>rows.push(r),request:async(role,am,method,path)=>{
    calls.push({am,method,path});return {status:method==='POST'?202:200,body:method==='POST'?{report_id:'report_1',status:'ready',report_complete:true}:{report_id:'report_1'}};
  }});
  await m.usage(1);assert.equal(rows.length,3);assert.ok(rows.every(r=>r.verdict==='PASS'));
  assert.equal(calls.filter(x=>x.method==='POST').length,3);assert.ok(calls.filter(x=>x.method==='GET').every(x=>x.am==='b'));
});

function alwaysOnServer({badAck=false,unavailableAfterWrite=false,loseAck=false}={}) {
  const rows=[],requests=[];
  let state={id:'sbx_fixture',revision:1,profile:{always_on:false}};
  const matrix=new AdminMatrix({fixture:fixture(),record:row=>rows.push(row),request:async(role,am,method,path,{body})=>{
    requests.push({am,method,path,body:structuredClone(body)});
    if(method==='GET') {
      if(unavailableAfterWrite && state.revision>1)return {status:503,body:{code:'operator_service_unavailable'}};
      if(path.includes('/events'))return {status:200,body:[{sandbox_id:state.id}]};
      return {status:200,body:{sandbox:structuredClone(state)}};
    }
    if(body.expected_revision!==state.revision)return {status:409,body:{code:'revision_conflict'}};
    state={...state,revision:state.revision+1,profile:{always_on:body.always_on}};
    if(loseAck && state.revision===2)throw Error('lost_ack');
    // SandboxView, from instances_openapi_shapes.rs, differs from the detail aggregate.
    return {status:200,body:{id:state.id,revision:state.revision,
      profile:{id:'profile_fixture',revision:1},...(badAck?{}:{always_on:state.profile.always_on})}};
  }});
  return {rows,requests,state:()=>state,run:callback=>matrix.sandboxAlwaysOn(1,{
    created_by_run:fixture().run_id,sandbox_id:state.id,
    verifyBrowserRelinquishment: callback && (async revision => {
      assert.equal(state.profile.always_on, true);
      assert.equal(state.revision, revision);
      state = {...state, revision: state.revision + 1, profile: {always_on: false}};
      return callback(revision);
    })})};
}

test('always-on uses the POST SandboxView and GET aggregate contracts with peer restoration',async()=>{
  const x=alwaysOnServer();await x.run();
  assert.equal(x.rows[0].verdict,'PASS');assert.equal(x.state().profile.always_on,false);
  assert.ok(x.requests.some(r=>r.am==='b'&&r.method==='POST'&&r.body.expected_revision===1));
  assert.equal(x.requests.at(-2).body.expected_revision,2);
});

test('lifecycle browser relinquishment settles the approval without another admin write', async () => {
  const fixture = alwaysOnServer();
  const proof = {owner: 'a', ingress: 'b', revisions_advanced: 1};
  await fixture.run(revision => {
    assert.equal(revision, 2);
    return proof;
  });
  assert.deepEqual(fixture.rows[0].owner_relinquishment, proof);
  assert.equal(fixture.state().profile.always_on, false);
  assert.equal(fixture.requests.filter(request => request.method === 'POST').length, 2);
});

test('acknowledged always-on mutation followed by product 503 remains a failing read',async()=>{
  const x=alwaysOnServer({unavailableAfterWrite:true});
  await assert.rejects(x.run(),/unexpected_http_503/);
  assert.equal(x.rows[0].primary_error_code,'unexpected_http_503');
  assert.equal(x.rows[0].restoration_error_code,'unexpected_http_503');
  assert.equal(x.requests.filter(r=>r.method==='POST').length,1);
  assert.equal(x.state().profile.always_on,true);
});

test('a restoration 503 cannot replace the original always-on acknowledgement failure',async()=>{
  const x=alwaysOnServer({badAck:true,unavailableAfterWrite:true});
  await assert.rejects(x.run(),/always_on_ack/);
  assert.equal(x.rows[0].error_code,'always_on_ack');
  assert.equal(x.rows[0].primary_error_code,'always_on_ack');
  assert.equal(x.rows[0].restoration_error_code,'unexpected_http_503');
  assert.equal(x.requests.filter(r=>r.method==='POST').length,1);
});

test('uncertain always-on acknowledgement restores once through its peer without replay',async()=>{
  const x=alwaysOnServer({loseAck:true});await assert.rejects(x.run(),/lost_ack/);
  assert.equal(x.rows[0].primary_error_code,'lost_ack');
  assert.equal(x.rows[0].restoration_error_code,undefined);
  assert.equal(x.requests.filter(r=>r.method==='POST'&&r.body.always_on===true).length,1);
  assert.equal(x.requests.filter(r=>r.method==='POST'&&r.body.always_on===false).length,1);
  assert.equal(x.state().profile.always_on,false);
});

import {ExtendedAdminMatrix} from './admin_extended.mjs';
test('extended settings replacement preserves restored semantic settings after lost ACK',async()=>{
  let state={revision:1,settings:{retention:{agent_log_days:7},limits:{visitor_max_launches_per_day:3},inactivity:{auto_pause_enabled:false,pause_after_seconds:900,grace_started_at:null}}};
  const old=structuredClone(state.settings),calls=[];
  const m=new ExtendedAdminMatrix({fixture:fixture(),record:()=>{},request:async(role,am,method,path,{body})=>{
    calls.push({am,method,body});if(method==='GET')return {status:200,body:structuredClone(state)};
    if(body.expected_revision!==state.revision)return {status:409,body:{}};
    state={revision:state.revision+1,settings:structuredClone(body.settings)};
    if(state.revision===2)throw Error('lost_ack');return {status:200,body:structuredClone(state)};
  }});
  await assert.rejects(m.fullReplace(1),/lost_ack/);assert.deepEqual(state.settings,old);
  assert.equal(calls.filter(x=>x.method==='PUT'&&x.body.settings.retention.agent_log_days===90).length,1);
});
test('extended pagination detects a repeated cross-AM cursor page',async()=>{
  const m=new ExtendedAdminMatrix({fixture:fixture(),record:()=>{},request:async()=>({status:200,body:{items:[{id:'x'},{id:'y'}],next:'cursor'}})});
  await assert.rejects(m.paginate(1,{collection:'sandboxes',expected_ids:['x','y','z']}),/pagination_duplicate/);
});
