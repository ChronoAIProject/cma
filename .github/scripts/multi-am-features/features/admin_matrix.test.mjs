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
