import test from 'node:test';
import assert from 'node:assert/strict';
import {execute, pass} from './test-helpers.mjs';

const stream = frames => ({status:200,headers:{'content-type':'text/event-stream'},body:frames.map(frame=>
  (frame.cursor===undefined?'':'id: '+frame.cursor+'\n')+'data: '+JSON.stringify(frame.body??frame)+'\n\n').join('')});
function responseStream(response) {
  return stream([{type:'response.created',sequence_number:0,response:{...response,status:'in_progress',output:[],incomplete_details:null}},
    {type:'response.'+response.status,sequence_number:1,response}]);
}
function compatibilityModel(fault) {
  let inputEffects=0,interruptEffects=0,active=0,hold=false,pendingResolve,startedResolve;
  let started=new Promise(resolve=>{startedResolve=resolve;});
  const responses=new Map(),receipts=new Map(),children=[],cleaned=[];
  const fixture={agentId:'agt_compat',sandboxId:'sbx',workspaceId:'wks',worktreeId:'wkt',launchId:'launch',
    launchEvents:[{seq:1,type:'created'}],conversationRequest:{metadata:{sandbox:'sbx',workspace:'wks'}},
    responseInput:'response input',responseText:'response output',aguiInput:'agui input',aguiText:'agui output',interruptInput:'held input',
    async readNativeFacts(){return {input_effects:inputEffects,interrupt_effects:interruptEffects,active_turns:active,agent_ids:['agt_compat',...children.map(v=>v.id)]};},
    async waitForOwner(){},async cleanupConversation(ctx,id){cleaned.push(id);},async holdNextTurn(){hold=true;},
    async waitForHeldTurnStarted(){await started;},async releaseHeldTurn(){hold=false;}};
  function aguiReply() {
    const event=(type,extra={})=>({type,threadId:'agt_compat',runId:'2',...extra});
    return stream([{cursor:10,body:event('RUN_STARTED')},
      {body:event('TEXT_MESSAGE_START',{messageId:'msg',role:'assistant'})},
      {body:event('TEXT_MESSAGE_CONTENT',{messageId:'msg',delta:'agui output'})},
      {cursor:11,body:event('TEXT_MESSAGE_END',{messageId:'msg'})},
      {cursor:12,body:event('RUN_FINISHED',{outcome:{type:'complete'}})}]);
  }
  async function handler(input) {
    const url=new URL(input.path,'http://localhost'),path=url.pathname;
    if(path.startsWith('/api/v1/responses/')&&(input.method==='DELETE'||path.endsWith('/cancel')))
      return {status:422,body:{code:'responses_field_unsupported',field:input.method==='DELETE'?'delete':'background'}};
    if(input.actor==='outsider') return {status:404,body:{code:'resource_not_found'}};
    if(path==='/api/v1/agui/capabilities') return {status:200,body:{protocol:'ag-ui',transports:['sse'],features:{transport:{streaming:true,resumable:true},custom:{'cma:terminal':false}}}};
    if(path.endsWith('/sandbox')) return {status:200,body:{threadId:'agt_compat',sandbox:{id:'sbx'},workspace:{id:'wks'},worktree:{id:'wkt'}}};
    if(path.includes('/agui/launches/')) return stream(fixture.launchEvents.map(body=>({cursor:body.seq,body})));
    if(path==='/api/v1/conversations'&&input.method==='POST') {
      const child={id:'agt_child_'+children.length,object:'conversation',metadata:input.body.metadata};children.push(child);
      return {status:200,body:child};
    }
    if(path.includes('/conversations/')) {
      if(path.endsWith('/items')) {
        const page=url.searchParams.has('after')?[{id:'msg-output',role:'assistant',content:[{text:'response output'}]}]:[{id:'msg-input',role:'user',content:[{text:'response input'}]}];
        return {status:200,body:{object:'list',data:page,first_id:page[0].id,last_id:page.at(-1).id,has_more:!url.searchParams.has('after')}};
      }
      return {status:200,body:children.find(value=>path.endsWith('/'+value.id))??{id:'agt_compat',object:'conversation'}};
    }
    if(path==='/api/v1/responses') {
      if(receipts.has(input.key)) {
        if(fault==='replay_effect') inputEffects++;
        return responseStream(receipts.get(input.key));
      }
      inputEffects++;
      const response={id:'resp_agt_compat_'+inputEffects,status:hold?'incomplete':'completed',object:'response',
        incomplete_details:hold?{reason:'interrupted'}:null,
        output:hold?[]:[{type:'message',role:'assistant',status:'completed',content:[{text:'response output'}]}]};
      responses.set(response.id,response);receipts.set(input.key,response);
      if(hold) {active=1;startedResolve();return new Promise(resolve=>{pendingResolve=()=>resolve(responseStream(response));});}
      return responseStream(response);
    }
    if(path.startsWith('/api/v1/responses/')) return {status:200,body:responses.get(path.split('/').at(-1))};
    if(path.endsWith('/interrupt')) {interruptEffects++;active=0;pendingResolve();return {status:202,body:{}};}
    if(path==='/api/v1/agui/run') {
      if(!receipts.has(input.key)) {inputEffects++;receipts.set(input.key,true);}
      return aguiReply();
    }
    if(path==='/api/v1/agui/events') return aguiReply();
  }
  return {fixture,handler,children,cleaned};
}

test('compatibility flows retain raw SSE, paged output, exact replay and settled interruption',async()=>{
  for(const number of [24,25,26,27,28,30,31,32,49,50,51,52,84,85]) {
    const model=compatibilityModel();
    const result=await execute(number,'compatibility',model.fixture,model.handler);
    pass(result);assert.deepEqual(model.cleaned,[...model.children].reverse().map(value=>value.id));
    assert.ok(result.result.owner_proofs.some(proof=>proof.owner!==proof.ingress));
    assert.ok(result.result.observations.some(value=>value.label==='cross_am_read_attribution'));
    const launchReads=result.events.filter(event=>event.kind==='response' && event.request.path.includes('/agui/launches/'));
    assert.deepEqual([...new Set(launchReads.filter(event=>event.request.actor==='user').map(event=>event.response.timing.am))].sort(),['a','b']);
    for(const event of launchReads) assert.equal(result.result.owner_proofs.some(proof=>proof.request_id===event.response.timing.request_id),false);
    if(number===49) {
      const writes=result.calls.filter(value=>value.path==='/api/v1/responses'&&value.body.input==='response input'&&value.actor==='user');
      assert.deepEqual(writes.map(value=>value.am),['b','a']);
    }
  }
  const model=compatibilityModel('replay_effect');
  const bad=await execute(49,'compatibility',model.fixture,model.handler);
  assert.equal(bad.result.status,'FAIL');assert.equal(bad.result.primary_failure,'compatibility_three_intents_no_replay');
  assert.equal(bad.cleanups.length,1);
});
