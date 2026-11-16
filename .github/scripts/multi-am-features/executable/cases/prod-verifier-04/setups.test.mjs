import test from 'node:test';
import assert from 'node:assert/strict';
import {Blocked} from '../../runner.mjs';
import {hash} from './common.mjs';
import {prepareDocuments,prepareHierarchy,prepareProfileAuthoring,requiredPrimitives,setups} from './setups.mjs';

function environment(fault) {
  const intents=[],ids=[],cleanups=[],calls=[];
  let releases=0,revision=1;
  const primitives={identities:Object.fromEntries(['user','recipient','outsider'].map(actor=>[actor,{actor,subject:actor+'-subject'}])),
    disposableDatabase:{scope:'LOCAL_DISPOSABLE_TWO_AM',database_disposable:true,async reserve(){return {
      parent_cleanup_barrier:true,async retainIntent(value){intents.push(value);},async retainDocument(id){ids.push(id);},
      async release(){releases++;return {registered:true,parent_cleanup_barrier:true};}};}}};
  const ctx={row:{case_id:'document-setup-fixture'},am:'a',defer(label,operation){cleanups.push(operation);},
    async observe(label,operation){return operation();},async request(input){
      calls.push(input);
      if(input.path==='/api/v1/documents') {
        assert.equal(intents.length,1);assert.equal(cleanups.length,1);
        if(fault==='unknown_create') throw Error('transport_unknown');
        const body={id:'doc_one',owner_subject:'user-subject',access:'owner',sha256:hash(input.body.markdown),source_revision:1,revision};
        if(fault==='attribution') {const error=Error('request_attribution_missing');error.response={status:201,body};throw error;}
        return {status:201,body};
      }
      if(input.method==='PATCH') return {status:200,body:{id:'doc_one',sha256:hash(input.body.markdown),source_revision:2,revision:++revision}};
      if(input.method==='PUT') return {status:200,body:{id:'doc_one',source_revision:2,revision:++revision}};
      return {status:201,body:{comment:{id:'comment_'+input.actor,author_subject:input.actor+'-subject',
        body:input.body.body,document_revision:2},document:{revision:++revision}}};
    }};
  return {ctx,primitives,intents,ids,cleanups,calls,releases:()=>releases};
}

test('document setup creates changed source and two separately attributed comments through public requests',async()=>{
  const env=environment();const fixture=await prepareDocuments(env.ctx,env.primitives);
  assert.equal(fixture.documentId,'doc_one');assert.equal(fixture.revisions.length,2);
  assert.notEqual(fixture.revisions[0].markdown,fixture.revisions[1].markdown);
  assert.deepEqual(fixture.comments.map(comment=>comment.author_subject),['user-subject','recipient-subject']);
  assert.deepEqual(env.calls.map(call=>call.method),['POST','PATCH','PUT','POST','POST']);
  assert.equal(env.calls[2].body.permission,'comment');
  assert.deepEqual(env.ids,['doc_one']);
  await fixture.cleanup();await env.cleanups[0]();assert.equal(env.releases(),1);
});

test('document setup retains accepted IDs and unknown intents without replay, including partial failures',async()=>{
  for(const fault of ['attribution','unknown_create']) {
    const env=environment(fault);
    await assert.rejects(prepareDocuments(env.ctx,env.primitives));
    assert.equal(env.intents.length,1);assert.equal(env.calls.length,1);
    assert.deepEqual(env.ids,fault==='attribution'?['doc_one']:[]);
    await env.cleanups[0]();assert.equal(env.releases(),1);
  }
});

test('setup exports block before HTTP without local custody and independent configuration',async()=>{
  assert.deepEqual(Object.keys(setups).sort(),Object.keys(requiredPrimitives).sort());
  for(const prepare of [prepareDocuments,prepareProfileAuthoring,prepareHierarchy]) {
    const env=environment();await assert.rejects(prepare(env.ctx,{}),Blocked);assert.equal(env.calls.length,0);
  }
  const env=environment();env.primitives.identities.recipient.subject='user-subject';
  await assert.rejects(prepareDocuments(env.ctx,env.primitives),Blocked);assert.equal(env.calls.length,0);
  await assert.rejects(prepareProfileAuthoring(env.ctx,env.primitives),Blocked);
  const hierarchyEvents=[]; let hierarchyReleased=0;
  const hierarchyPrimitives={scope:'LOCAL_DISPOSABLE_TWO_AM',database_disposable:true,single_matrix_writer:true,
    defaults:{sandbox_profile:{id:'sbp',revision:1},workspace_profile:{id:'wkp',revision:1},agent_profile:{id:'agp',revision:1}},
    sandbox_profile:{id:'sbp',revision:1},workspace_profile:{id:'wkp',revision:1},agent_profile:{id:'agp',revision:1},
    async reserve(){return {parent_cleanup_barrier:true,async retainIntent(v){hierarchyEvents.push(['intent',v.kind]);},
      async retainResource(v){hierarchyEvents.push(['resource',v.kind]);},async release(){hierarchyReleased++;return {released:true};}};},
    async finality(){return {sandbox_id:'sbx',children_remaining:0,pending_operations:0,cleanup_complete:true};},
    async readNativeFacts(){return {};},async waitForOwner(){}};
  const hierarchy=await prepareHierarchy(env.ctx,{userResources:hierarchyPrimitives});
  assert.deepEqual(hierarchyEvents,[]); assert.equal(typeof hierarchy.cleanupResource,'function');
  await hierarchy.cleanup(); assert.equal(hierarchyReleased,1);
  env.primitives.profileAuthoring={source:'PARENT_PINNED_SYNTHETIC_CONFIGURATION',repositoryRequest:{url:'https://example.invalid/synthetic.git',reference:'main'},
    repositoryCommit:'c'.repeat(40),services:[]};
  const fixture=await prepareProfileAuthoring(env.ctx,env.primitives);
  assert.equal(fixture.repositoryCommit,'c'.repeat(40));assert.equal(env.calls.length,0);
  env.primitives.profileAuthoring.repositoryRequest.url='https://user:synthetic@example.invalid/repo';
  await assert.rejects(prepareProfileAuthoring(env.ctx,env.primitives),/must_not_contain_credentials/);
});
