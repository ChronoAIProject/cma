import test from 'node:test';
import assert from 'node:assert/strict';
import {RetainedAgentJourney,STAGES} from './migration_agent_journey.mjs';
import {artifact,completedResponse,hash,lineage,nativeObservation,preservedTranscript,
  providerObservation,readerObservation,transcript} from './migration_agent_observations.mjs';
import {artifacts,digest,fixture,pod} from './migration_agent_test_fixture.mjs';

test('four stages retain both writer lineages, responses and native homes',async () => {
  const f = fixture(), results = [];
  for (const [stage,version,incarnation] of [[STAGES[0],'old',0],[STAGES[1],'new',1],
    [STAGES[2],'new',1],[STAGES[3],'old',2]]) {
    f.switchReader(version,incarnation);
    results.push(await f.journey.run(stage));
  }
  assert.deepEqual(results.map(r => r.agents.map(a => a.responses)),[[1],[2],[3,1],[4,2]]);
  assert.equal(f.state.calls.filter(c => c.method === 'POST').length,2);
  assert.equal(f.state.calls.filter(c => c.method === 'SEND').length,6);
  assert.equal(results[3].full_reader_writer_gate,'NOT_PROVEN');
  assert.equal(results[3].persisted_collection_coverage,'NOT_PROVEN');
  assert.equal(f.journey.status().next_stage,null);
  await assert.rejects(f.journey.run(STAGES[0]),/finite_stage_order/);
  const clone = f.journey.resources(); clone[0].agent_id = 'changed';
  assert.equal(f.journey.resources()[0].agent_id,'agent-1');
});

test('reader stage cannot skip, repeat, overlap, change artifact or process in place',async () => {
  const f = fixture();
  await assert.rejects(f.journey.run(STAGES[1]),/finite_stage_order/);
  const started = f.journey.run(STAGES[0]);
  await assert.rejects(f.journey.run(STAGES[0]),/finite_stage_order/);
  await started;
  await assert.rejects(f.journey.run(STAGES[0]),/finite_stage_order/);
  f.switchReader('new',0);
  await assert.rejects(f.journey.run(STAGES[1]),/reader_process_transition/);
  assert.equal(f.journey.status().failed,true);
});

for (const [name,change,reason] of [
  ['reader artifact',v => {v.source='f'.repeat(40);},'reader_artifact_mismatch'],
  ['reader Ready',v => {v.pod.status.conditions[0].status='False';},'pod_not_ready'],
  ['reader restart',v => {v.pod.status.containerStatuses[0].restartCount=1;},'container_not_ready'],
  ['reader mutation mid-stage',(v,f) => {if (f.state.readerCalls>1)v.pod.metadata.uid='new-uid';},'reader_changed'],
]) test(name + ' refuses stage',async () => {
  const f = fixture(); f.state.mutateReader = v => change(v,f);
  await assert.rejects(f.journey.run(STAGES[0]),new RegExp(reason));
  assert.equal(f.journey.status().failed,true);
});

for (const [name,change,reason] of [
  ['lost workspace',f => {f.state.records.get('agent-1').detail.workspace.id='other';},'retained_lineage'],
  ['rewritten terminal',f => {f.state.mutateResponse = v => {v.output[0].content[0].text='corrupt';};},'retained_response'],
  ['rewritten history',f => {f.state.mutateTranscript = v => {v.turns[0].owner_message='corrupt';};},'retained_transcript'],
  ['lost history',f => {f.state.mutateTranscript = v => {v.turns=[];};},'retained_transcript'],
  ['unexplained turn',f => {f.state.mutateTranscript = v => {v.turns.push({...v.turns[0],turn_sequence:2});};},'unexpected_retained_turn'],
  ['replaced home',f => {f.state.mutateNative = v => {v.sandbox.metadata.annotations['cma/home-identity']='new-home';};},'retained_runtime'],
  ['replaced PVC',f => {f.state.mutateNative = v => {v.pvcs[0].metadata.uid='new-pvc';};},'retained_runtime'],
]) test(name + ' fails before new-reader writes',async () => {
  const f = fixture(); await f.journey.run(STAGES[0]);
  change(f); f.switchReader('new',1);
  const writes = f.state.calls.filter(c => ['POST','SEND'].includes(c.method)).length;
  await assert.rejects(f.journey.run(STAGES[1]),new RegExp(reason));
  assert.equal(f.state.calls.filter(c => ['POST','SEND'].includes(c.method)).length,writes);
});

for (const [name,change,reason] of [
  ['lost create',f => {f.state.failure='lost_create';},'create_response_lost'],
  ['invalid create',f => {f.state.failure='invalid_create';},'lifecycle_receipt_missing'],
  ['lost send',f => {f.state.failure='lost_send';},'send_response_lost'],
  ['stream failure',f => {f.state.failure='stream';},'stream_failed'],
  ['release failure',f => {f.state.failure='release';},'release_failed'],
  ['lifecycle failure',f => {f.state.mutateLifecycle=v=>{v.status='failed';};},'create_lifecycle_failed'],
  ['lifecycle wrong identity',f => {f.state.mutateLifecycle=v=>{v.id='other';};},'lifecycle_identity'],
  ['lifecycle timeout',f => {f.state.mutateLifecycle=v=>{v.status='pending';};},'create_lifecycle_timeout'],
  ['duplicate provider effect',f => {f.state.mutateOracle=v=>{v.request_count=2;};},'provider_held_contract'],
  ['failed response',f => {f.state.mutateResponse=v=>{v.status='failed';};},'completed_marker'],
  ['wrong response identity',f => {f.state.mutateResponse=v=>{v.conversation.id='other';};},'response_identity'],
  ['missing assistant output',f => {f.state.mutateResponse=v=>{v.output=[];};},'completed_marker'],
  ['missing transcript turn',f => {f.state.mutateTranscript=v=>{v.turns=[];};},'new_transcript_turn'],
]) test(name + ' retains evidence and refuses a retry',async () => {
  const f = fixture(); change(f);
  await assert.rejects(f.journey.run(STAGES[0]),new RegExp(reason));
  const calls = f.state.calls.length;
  await assert.rejects(f.journey.run(STAGES[0]),/finite_stage_order/);
  assert.equal(f.state.calls.length,calls);
  assert.equal(f.journey.resources().length,1);
  assert.equal(f.state.events.at(-1).type,'stage_failed');
});

test('admitted identity survives an observation-sink failure',async () => {
  const f = fixture();
  const journey = new RetainedAgentJourney({...f.options,observe:async event => {
    if (event.type === 'create_receipt' || event.type === 'stage_failed') throw new Error('disk_full');
  }});
  await assert.rejects(journey.run(STAGES[0]),/stage_and_evidence_failed/);
  assert.equal(journey.resources()[0].operation_id,'operation-1');
});

test('lifecycle Agent binding cannot move between polls',async () => {
  const f = fixture(); let n=0;
  f.state.mutateLifecycle=v=>{if (!n++){v.status='pending';}else{v.agent_id='other';}};
  await assert.rejects(f.journey.run(STAGES[0]),/lifecycle_agent_changed/);
  assert.equal(f.journey.resources()[0].agent_id,'agent-1');
});

test('a completed response awaits stream finality without cancelling it',async () => {
  const f = fixture();
  await f.journey.run(STAGES[0]);
  assert.equal(f.state.calls.filter(c => c.method === 'ABORT').length,0);
});

test('failed response joins its stream even if abort throws',async () => {
  const f = fixture(), send = f.api.send; let joined=false;
  f.api.send = async (...args) => {
    const stream = await send(...args);
    return {...stream,done:Promise.resolve().then(()=>{joined=true;}),abort(){throw new Error('abort_failed');}};
  };
  f.state.mutateResponse=v=>{v.status='failed';};
  await assert.rejects(f.journey.run(STAGES[0]),/reply_and_abort_failed/);
  assert.equal(joined,true);
});

test('transport details are not copied into failure reason',async () => {
  const f = fixture();
  f.api.send = async () => {throw new Error('untrusted transport detail');};
  await assert.rejects(f.journey.run(STAGES[0]),/untrusted transport detail/);
  assert.equal(f.state.events.at(-1).reason,'adapter_or_transport_failure');
});

test('native runtime validation uses Pod status and immutable homes',async () => {
  const f = fixture(); await f.journey.run(STAGES[0]);
  const record = f.journey.resources()[0], raw = await f.adapter.runtimeFor({agentId:record.agent_id});
  for (const [change,reason] of [
    [v=>{v.agent_id='wrong';},'runtime_lineage'],
    [v=>{v.sandbox.metadata.annotations['cma/sandbox']='wrong';},'sandbox_join'],
    [v=>{delete v.sandbox.metadata.annotations['cma/home-identity'];},'home_missing'],
    [v=>{v.pod.metadata.ownerReferences=[];},'pod_owner'],
    [v=>{v.pod.metadata.deletionTimestamp='now';},'live_resource'],
    [v=>{v.pod.status.conditions[0].status='False';},'pod_not_ready'],
    [v=>{v.pod.status.containerStatuses[0].ready=false;},'container_not_ready'],
    [v=>{v.pod.status.containerStatuses[0].restartCount=1;},'container_not_ready'],
    [v=>{v.pod.status.containerStatuses[0].imageID=digest('f');},'image_id'],
    [v=>{v.pod.spec.containers[0].image='tag:latest';},'container_not_ready'],
    [v=>{v.pod.status.containerStatuses=[];},'container_binding'],
    [v=>{v.pod.spec.volumes=[];},'pvc_set'],
    [v=>{v.pvcs=[];},'pvc_set'],
    [v=>{v.pvcs[0].status.phase='Pending';},'pvc_not_bound'],
    [v=>{v.pvcs[0].metadata.namespace='foreign';},'pvc_not_bound'],
    [v=>{v.pod.spec.volumes.push({persistentVolumeClaim:{claimName:'another'}});v.pvcs.push(v.pvcs[0]);},'pvc_duplicate'],
  ]) {
    const value = structuredClone(raw); change(value);
    assert.throws(()=>nativeObservation(value,record.lineage,artifacts.runtime),new RegExp(reason));
  }
  raw.pod.status.containerStatuses[0].imageID='docker-pullable://' + artifacts.runtime.image;
  assert.equal(nativeObservation(raw,record.lineage,artifacts.runtime).pod.image_id,
    'docker-pullable://' + artifacts.runtime.image);
});

test('constructor and validators reject missing contracts',() => {
  const f = fixture();
  for (const changes of [{api:{}},{adapter:{}},{observe:null},{timeout:Infinity},
    {artifacts:{...artifacts,new:artifacts.old}},{artifacts:{}}]) {
    assert.throws(()=>new RetainedAgentJourney({...f.options,...changes}));
  }
  assert.throws(()=>artifact({...artifacts.old,image:'tag:latest'}),/immutable/);
  assert.throws(()=>readerObservation({...artifacts.new,pod:pod('api','uid','api',artifacts.new,'1')},artifacts.old),/reader_artifact/);
  assert.throws(()=>lineage({id:'other'},'agent'),/agent_identity/);
  assert.throws(()=>lineage({id:'agent',status:'starting'},'agent'),/agent_not_ready/);
  assert.throws(()=>transcript({agent_id:'agent',through_seq:1,turns:[],next:{}},'agent'),/complete_transcript/);
  assert.throws(()=>transcript({agent_id:'agent',through_seq:1,turns:[{}]},'agent'),/transcript_turn/);
  assert.throws(()=>preservedTranscript({through_seq:2,turns:[]},{through_seq:1,turns:[]}),/retained_transcript/);
  assert.throws(()=>providerObservation({},'case',1),/provider_effect/);
  assert.throws(()=>completedResponse({},'agent','response','marker'),/response_identity/);
  assert.equal(hash({b:2,a:1}),hash({a:1,b:2}));
});

test('kind reader aliases require the exact observed manifest or config',() => {
  const p = pod('api','uid','api',artifacts.old,'1'), alias='cma-matrix-old-abcdef123456';
  p.spec.containers[0].image=alias;
  const value={...artifacts.old,local_image_alias:alias,pod:p};
  assert.equal(readerObservation(value,artifacts.old).pod.selected_image,alias);
  p.status.containerStatuses[0].imageID=digest('f');
  assert.throws(()=>readerObservation(value,artifacts.old),/image_id/);
  value.local_image_alias='unrelated:latest';
  assert.throws(()=>readerObservation(value,artifacts.old),/reader_alias/);
});
