import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {cases} from './index.mjs';
import {hash, readOtherAM, request, rows, take} from './common.mjs';
import {assertAgui, assertResponses, sseFrames} from './compatibility.mjs';
import {assertMetadata} from './metadata.mjs';
import {assertCapability} from './capabilities.mjs';
import {assertNoCredentialFields} from './delegation.mjs';
import {assertTotals, totalsFor} from './usage.mjs';
import {Blocked, executeCase} from '../../runner.mjs';

const assertionContext = {assert(value, code) { assert.ok(value, code); }};
const binding = {phase: 'before_restart', iteration: 1, am: 'a',
  image: 'synthetic@sha256:' + 'a'.repeat(64), source: 'b'.repeat(40)};
const assigned = JSON.parse(readFileSync(new URL('../../assignments/prod-verifier-04.json', import.meta.url))).case_ids;
const fixtureSpec = JSON.parse(readFileSync(new URL('./fixtures.json', import.meta.url)));
const definition = number => cases.find(value => value.id.endsWith('-' + String(number).padStart(3,'0')));
const rowFor = value => rows.find(row => row.case_id === value.id);
const sse = events => ({headers: {'content-type': 'text/event-stream; charset=utf-8'},
  body: events.map(event => (event.id === undefined ? '' : 'id: ' + event.id + '\n')
    + (event.event === undefined ? '' : 'event: ' + event.event + '\n')
    + 'data: ' + JSON.stringify(event.body ?? event) + '\n\n').join('')});

test('the contribution exactly covers API87 without adding C275 or weakening steps', () => {
  assert.equal(cases.length, 87);
  assert.equal(new Set(cases.map(value => value.id)).size, 87);
  assert.deepEqual(cases.map(value => value.id).sort(), [...assigned].sort());
  for (const value of cases) {
    assert.deepEqual(value.steps, rowFor(value).required_steps);
    assert.equal(typeof value.run, 'function');
    assert.ok(['global','agent'].includes(value.scope));
  }
});
test('every fixture-dependent row blocks with no installed adapter', async () => {
  for (const value of cases.filter(value => value.fixtures.length)) {
    const result = await executeCase(value, rowFor(value), binding);
    assert.equal(result.status, 'BLOCKED', value.id);
    assert.equal(result.requests.length, 0);
  }
});
test('named setup omission and incomplete methods are BLOCKED after cleanup registration', async () => {
  for (const name of Object.keys(fixtureSpec.setups)) {
    let cleanups = 0, registered;
    const ctx = {fixture: () => ({take: async () => ({cleanup: async () => { cleanups++; }})}),
      defer(label, operation) { registered = operation; }};
    await assert.rejects(take(ctx, name), Blocked);
    assert.equal(typeof registered, 'function');
    await registered(); assert.equal(cleanups, 1);
    await assert.rejects(take({...ctx, fixture: () => ({take: async () => null})}, name), Blocked);
  }
});
test('Agent writes omit ingress, and cross-AM reads use actual preceding attribution', async () => {
  const calls = [];
  const ctx = {...assertionContext, am: 'a',
    async request(value) { calls.push(value); return {status:200, timing:{am:value.am, request_id:'safe-receipt'}}; },
    async observe(label, operation) { return operation(); }};
  await request(ctx,'POST','/api/v1/agents/agt_fixture/input',{text:'synthetic'},{agentId:'agt_fixture'});
  assert.equal(calls[0].am, undefined);
  await readOtherAM(ctx, {}, {method:'GET',path:'/api/v1/agents/agt_fixture',agentId:'agt_fixture'}, {timing:{am:'b'}});
  assert.equal(calls[1].am, 'a');
  assert.equal(calls[1].agentId, 'agt_fixture');
  await assert.rejects(readOtherAM({...ctx,request:async()=>({timing:{am:'b'}})}, {},
    {method:'GET',path:'/api/v1/agents/agt_fixture'}, {timing:{am:'b'}}), /cross_am_read_distinct_ingress/);
});

function responseEvents() {
  const response = {id:'resp_agt_fixture_1',object:'response',status:'in_progress',output:[],incomplete_details:null};
  return [
    {type:'response.created',sequence_number:0,response},
    {type:'response.output_text.delta',sequence_number:1,delta:'synthetic output'},
    {type:'response.completed',sequence_number:2,response:{...response,status:'completed',
      output:[{type:'message',role:'assistant',status:'completed',content:[{text:'synthetic output'}]}]}},
  ];
}
test('Responses validation accepts real frame order and rejects missing/duplicate/foreign terminal data', () => {
  const events = responseEvents();
  assert.equal(assertResponses(assertionContext,sse(events),'agt_fixture','synthetic output').id,'resp_agt_fixture_1');
  const mutations = [
    value => value.slice(0,-1),
    value => [value[0],value[1],value[1],value[2]],
    value => { value[2].response.id = 'resp_agt_foreign_1'; return value; },
    value => { value[2].response.output[0].content[0].text = 'wrong'; return value; },
    value => [...value, {type:'response.output_text.delta',sequence_number:3,delta:'late'}],
    value => { value[0].id = 'durable-cursor-not-supported'; return value.map(v => ({...v,body:v})); },
  ];
  for (const mutate of mutations) assert.throws(() => assertResponses(assertionContext,sse(mutate(structuredClone(events))),
    'agt_fixture','synthetic output'));
});
test('SSE transport must supply bounded raw frames with valid JSON and termination', () => {
  assert.throws(() => sseFrames({headers:{'content-type':'application/json'},body:'{}'}), /actual_sse/);
  assert.throws(() => sseFrames({headers:{'content-type':'text/event-stream'},body:{events:[]}}), /raw_sse/);
  assert.throws(() => sseFrames({headers:{'content-type':'text/event-stream'},body:'data: nope\n\n'}), /invalid_json/);
  assert.throws(() => sseFrames({headers:{'content-type':'text/event-stream'},body:'data: {}'}), /truncated/);
  assert.equal(sseFrames({headers:{'content-type':'text/event-stream'},body:': keepalive\r\ndata: {\r\ndata: "kind":1}\r\n\r\n'})[0].body.kind,1);
});
function aguiFrames() {
  const event = (type,body={}) => ({type,threadId:'agt_fixture',runId:'4',...body});
  return [
    {id:'10',body:event('RUN_STARTED')},
    {body:event('TEXT_MESSAGE_START',{messageId:'msg_11',role:'assistant'})},
    {body:event('TEXT_MESSAGE_CONTENT',{messageId:'msg_11',delta:'synthetic output'})},
    {id:'11',body:event('TEXT_MESSAGE_END',{messageId:'msg_11'})},
    {id:'12',body:event('RUN_FINISHED',{outcome:{type:'complete'}})},
  ];
}
test('AG-UI distinguishes durable cursors from live fragments and checks message identity/order', () => {
  assert.equal(assertAgui(assertionContext,sse(aguiFrames()),'agt_fixture','synthetic output').cursor,12);
  const mutations = [
    value => { value[1].id = '10'; return value; },
    value => { value[2].body.threadId = 'agt_foreign'; return value; },
    value => { value[2].body.messageId = 'msg_other'; return value; },
    value => { value[3].body['cma:live'] = true; return value; },
    value => value.slice(0,-1),
    value => { value[2].body.delta = 'wrong output'; return value; },
  ];
  for (const mutate of mutations) assert.throws(() => assertAgui(assertionContext,sse(mutate(aguiFrames())),
    'agt_fixture','synthetic output'));
});
test('usage totals are computed from allocation input with the correct CPU/RAM/disk units', () => {
  const expected = {uptime_seconds:3600,cpu_millis_seconds:3600000,memory_mib_seconds:7372800,
    disk_gib_seconds:10800,cpu_core_hours:1,memory_gib_hours:2,disk_gib_hours:3};
  assert.deepEqual(totalsFor([{running:true,started_at:0,ended_at:3600,cpu_millis:1000,memory_mib:2048,disk_gib:3}]), expected);
  assertTotals(assertionContext,expected,expected);
  const paused = totalsFor([{running:false,started_at:0,ended_at:3600,cpu_millis:0,memory_mib:0,disk_gib:3}]);
  assert.equal(paused.uptime_seconds,0); assert.equal(paused.disk_gib_hours,3);
  assert.throws(() => assertTotals(assertionContext,{...expected,memory_gib_hours:2048},expected), /usage_memory_gib_hours/);
  assert.throws(() => assertTotals(assertionContext,{...expected,cpu_core_hours:NaN},expected), /usage_cpu_core_hours/);
});
test('artifact metadata checks content hash and Agent lineage, not HTTP success', () => {
  const fixture = {agentId:'agt_fixture',artifact:{id:'art_fixture',content:'fixture bytes',name:'fixture.txt',media_type:'text/plain',sha256:hash('fixture bytes')}};
  const body = {agent_id:fixture.agentId,id:fixture.artifact.id,sha256:fixture.artifact.sha256,
    size_bytes:13,name:'fixture.txt',media_type:'text/plain'};
  assertMetadata(assertionContext,12,body,fixture);
  assert.throws(() => assertMetadata(assertionContext,12,{...body,agent_id:'agt_foreign'},fixture), /artifact_identity/);
  assert.throws(() => assertMetadata(assertionContext,12,{...body,sha256:'0'.repeat(64)},fixture), /artifact_identity/);
  assert.throws(() => assertMetadata(assertionContext,23,{code:'ok',ticket:'synthetic'},fixture), /terminal_documented_refusal/);
});
test('capability manifests bind the current synthetic account and reject stale account content', () => {
  const path = '/api/v2/ui/task-experience';
  const body = {object:'cma.ui.task_experience',schema_version:1,account_id:'owner',capabilities:{desktop_enable:false}};
  const fixture = {subjects:{user:'owner',outsider:'other'},manifest:{[path]:body}};
  assertCapability(assertionContext,path,body,fixture);
  assert.throws(() => assertCapability(assertionContext,path,body,fixture,'outsider'), /exact_image_manifest/);
});
test('delegation refuses nested credential-bearing response fields', () => {
  assertNoCredentialFields(assertionContext,{status:201,body:{id:'synthetic'}});
  assert.throws(() => assertNoCredentialFields(assertionContext,{body:{access_token:'synthetic'}}), /credential/);
});

test('invalid named fixture cannot manufacture a PASS from declarations', async () => {
  for (const number of [13,24,29,34,42,62,64,69]) {
    let cleaned = 0;
    const value = definition(number);
    const result = await executeCase(value,rowFor(value),{...binding,
      fixtures:{'api-fixtures':{async take(){return {cleanup(){cleaned++;}};}}}});
    assert.equal(result.status,'BLOCKED',value.id);
    assert.equal(result.requests.length,0);
    assert.equal(cleaned,1);
  }
});
