import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { compareSnapshots, eventPrefix } from './cma-bridge-continuity-observer.mjs';
import { nativeIdentity } from './cma-bridge-continuity-native.mjs';
import { loadOwnedFixture, validateOwnedFixture } from './cma-bridge-continuity-owned-config.mjs';
import { hash, successorLabel, predecessorNames, successorEvidenceNames, validateRecordedHistory, resolveSuccessorFixture } from './cma-bridge-continuity-successor.mjs';

globalThis.fetch = () => { throw new Error('offline_tests_forbid_network'); };
const bytes = value => Buffer.from(JSON.stringify(value, null, 2) + '\n');
const time = minute => `2026-09-16T12:${String(minute).padStart(2,'0')}:00Z`;
const baseEvents = [{ seq: 1, appended_at: time(0), kind: { type: 'conversation_opened', conversation_id: 'retained', runtime_generation: 1 } }];

function fixture(paused = false, precedingCycles = 1, retirementEvents = false) {
  const resources = { agentIds: ['agt_trigger', 'agt_manual'], sandboxId: 'sbx_owned', workspaceId: 'wks_owned' };
  const ids = [...resources.agentIds, 'agt_bootstrap']; const first = { outcome: { type: 'completed' }, agent_messages: ['MARKER'], messages: [{ role: 'assistant', content: 'MARKER' }] };
  const values = counts => ids.map((id,index) => ({ agent: { id, status: 'ready', sandbox: { id: resources.sandboxId }, workspace: { id: resources.workspaceId }, worktree: { id: `wkt_${index}` }, profile: {}, created_at: time(0) },
    transcript: { agent_id: id, turns: Array.from({length:counts[index]},(_,turn)=>turn?{...first,owner_message:'repeat marker'}:first) }, provenance: { agent_id: id, creation_origin: ['trigger','manual','legacy_unknown'][index], details: { workspace_profile_index: index === 2 ? 0 : null } } }));
  const before = { schema: 'cma-bridge-continuity-observation-v1', status: 'observed', observedAt: time(0), scope: { agentIds: resources.agentIds },
    agents: resources.agentIds.map((id,index) => ({ id, sandboxId: resources.sandboxId, workspaceId: resources.workspaceId, worktreeId: `wkt_${index}`, profileHash: 'a'.repeat(64), createdAt: time(0), status: 'ready', events: eventPrefix(baseEvents) })),
    runtimes: [{ name: 'runtime1', uid: 'uid1', sandboxId: resources.sandboxId, generation: '1', contract: '3', containers: [{name:'runtime',image:'fixture',id:'container',restarts:0}] }] };
  const original = { schema: 'cma-public-canary-journal-v1', run: 'test', owner: 'subject', origin: 'https://fixture.invalid', commit: '1'.repeat(40), resources, bootstrapAgentId: ids[2],
    commands: { 'continue-0': { path: `/api/v1/agents/${ids[0]}/input`, body: JSON.stringify({ text: 'repeat marker' }), key: 'test-continue-0', state: 'unsettled', attempts: 1, safeCode: 'public_http_403_forbidden' } },
    baseline: { before, firstTurns: [first,first], nativeBefore: [nativeIdentity(baseEvents),nativeIdentity(baseEvents)] } };
  const originalBytes = bytes(original); const files = {};
  const state = (cycles, isPaused, counts) => {
    const events = []; const agentEvents = ids.map(() => structuredClone(baseEvents));
    const append = (minute,kind) => events.push({ seq: events.length+1, sandbox_id: resources.sandboxId, appended_at: time(minute), kind });
    for (let c=0;c<cycles+Number(isPaused);c++) {
      append(c*6+1,{ type:'status_changed', from:'active', to:'pausing' });
      append(c*6+2,{ type:'checkpoint_sealed' }); append(c*6+3,{ type:'status_changed', from:'pausing', to:'paused' });
      if (c === cycles) break;
      append(c*6+4,{ type:'status_changed', from:'paused', to:'reviving' });
      agentEvents.forEach(items => {
        if(retirementEvents)items.push({ seq:items.length+1, appended_at:time(c*6+4), kind:{ type:'runtime_generation_retired', previous_generation:c+1, runtime_generation:retirementEvents?c+2:2*(c+1)+1 } });
        items.push({ seq:items.length+1, appended_at:time(c*6+5), kind:{ type:'conversation_opened', conversation_id:'retained', runtime_generation:retirementEvents?c+2:2*(c+1)+1 } });
      });
      append(c*6+5,{ type:'status_changed', from:'reviving', to:'active' }); append(c*6+6,{ type:'revived' });
    }
    const publicValues = values(counts); publicValues.forEach((item,index)=>{item.transcript.through_seq=agentEvents[index].length;}); if (isPaused) publicValues.forEach(item => { item.agent.status='paused'; });
    const runtime = isPaused ? null : { ...structuredClone(before), observedAt: time(cycles*6+1), agents: before.agents.map((agent,index) => ({ ...agent, events:eventPrefix(agentEvents[index]) })),
      runtimes: [{ ...before.runtimes[0], name:`runtime${cycles+1}`, uid:`uid${cycles+1}`, generation:String(retirementEvents?cycles+1:cycles*2+1) }] };
    const result = { sandbox: { id:resources.sandboxId, owner:original.owner, agent_count:3, worktree_count:3, workspace_count:1, workspaces:[{id:resources.workspaceId}], status:isPaused?'paused':'active', lifecycle: { latest_snapshot: { epoch:cycles+Number(isPaused), saved_at:events.filter(e=>e.kind.type==='checkpoint_sealed').at(-1).appended_at } } }, sandboxEvents:events, agentEvents, public:publicValues, runtime };
    result.native=validateRecordedHistory(original,result).native; return result;
  };
  const predecessorObservation = state(0,true,[1,1,0]);
  const predecessorLabel='continue-0-after-nonadmission-1';
  const request = label => ({ path:original.commands['continue-0'].path, body:original.commands['continue-0'].body, key:`${original.run}-${label}`, status:202 });
  files['predecessor-observation.json']=bytes(predecessorObservation);
  const pd={ schema:'cma-confirmed-nonadmission-followup-directive-v1', recoveryId:'predecessor', originalJournalSha256:hash(originalBytes), followup:request(predecessorLabel), evidenceSha256:{'observation.json':hash(files['predecessor-observation.json'])} };
  files['predecessor-directive.json']=bytes(pd);
  const pj={ schema:'cma-nonadmission-followup-journal-v1', recoveryId:pd.recoveryId, originalJournalSha256:hash(originalBytes), directiveHash:hash(files['predecessor-directive.json']), resumedApplicationCommit:'2'.repeat(40), resources, bootstrapAgentId:ids[2], original403:{command:original.commands['continue-0']}, commands:{ [predecessorLabel]:{...request(predecessorLabel),state:'accepted',attempts:1,value:{queued:true}} } };
  files['predecessor-journal.json']=bytes(pj);
  const row={_id:'cmd_failed',agent_id:ids[0],response_id:'resp_failed',status:'failed',record:{receipt:{admitted_at:time(5)},work:{failure:{code:'authority_unavailable',at:time(6)}}}};
  const terminal={commandId:row._id,responseId:row.response_id,status:'failed',admittedAt:time(5),failure:row.record.work.failure};
  files['terminal-capture.json']=bytes({schema:'cma-owned-command-status-read-v1',status:'captured',mutatingCommands:0,readConcern:'majority',readPreference:'primary',effectiveReadConcern:'snapshot',snapshot:{majorityCommitted:true},results:{owned_followup_command_status:{complete:true,rows:[row]}}});
  const predecessorEvidenceSha256=Object.fromEntries(predecessorNames.map(name=>[name,hash(files[name])]));
  const observation={...state(precedingCycles,paused,[1,1,0]),applicationCommit:'3'.repeat(40)};
  const fresh={...observation,nonadmission:{originalCommandAbsent:true,predecessorTerminalUnchanged:true,proposedKeyUnused:true}};
  const after=state(precedingCycles+Number(paused),false,[2,2,0]);
  const root={status:'confirmed_terminal_nondispatch',applicationCommit:observation.applicationCommit,originalJournalSha256:hash(originalBytes),originalExecutionCannotAppend:true,predecessorExecutionCannotAppend:true,priorKeysWillNeverBeResubmitted:true,predecessorFailure:terminal,predecessorEvidenceSha256,
    sourceProof:{applicationCommit:pj.resumedApplicationCommit,terminalBeforeDispatch:true,blobSha256:'b'.repeat(64),path:'worker.rs'},freshKeyAuthorized:request(successorLabel).key,observationSha256:hash(observation),runtimeMode:'recorded_lifecycle_successor',allowImplicitRevivalFromMessage:paused};
  for(const [name,value] of Object.entries({'observation.json':observation,'root-reconciliation.json':root,'fresh-preflight.json':fresh,'api-receipt.json':{},'web-receipt.json':{},'after-rollout.json':{},'after.json':after,'resumption-lifecycle.json':{sandbox:after.sandbox,events:after.sandboxEvents}})) files[name]=bytes(value);
  const directive={schema:'cma-terminal-nondispatch-successor-directive-v1',recoveryId:'successor',originalJournalSha256:hash(originalBytes),startedApplicationCommit:original.commit,resumedApplicationCommit:observation.applicationCommit,predecessorEvidenceSha256,priorKeysWillNeverBeResubmitted:true,pollSeconds:15,newResourcesPermitted:0,plannedFinalTurns:[2,2,0],followup:request(successorLabel),helperSha256:{'/fixture.mjs':'a'.repeat(64)},evidenceSha256:Object.fromEntries(successorEvidenceNames.map(name=>[name,hash(files[name])])),runtimeMode:'recorded_lifecycle_successor'};
  files['directive.json']=bytes(directive);
  const result={status:'passed',resources,commit:observation.applicationCommit,startedApplicationCommit:original.commit,resumedApplicationCommit:observation.applicationCommit,turnApplicationCommits:Array(2).fill([original.commit,observation.applicationCommit]),originalFailedRunPreserved:true,
    recovery:{id:directive.recoveryId,originalJournalSha256:hash(originalBytes),directiveSha256:hash(files['directive.json']),predecessorEvidenceSha256,predecessorFailure:terminal,priorKeysRetried:false,originalForbiddenPreserved:true,exactOriginalKeyRetried:false,originalKeyDisposition:'not_resubmitted',replacementFollowupKey:request(successorLabel).key,helperSha256:directive.helperSha256,deploymentEvidenceSha256:directive.evidenceSha256,freshPreflightSha256:hash(fresh),finalEvidenceSha256:{'after.json':hash(files['after.json']),'resumption-lifecycle.json':hash(files['resumption-lifecycle.json'])}},
    runtimeMode:directive.runtimeMode,nativeIdentity:after.native,lifecycle:{before:validateRecordedHistory(original,observation),after:validateRecordedHistory(original,after)},continuity:compareSnapshots(before,after.runtime),allOwnedAgentIds:ids,totalOwnedAgents:3,completedTurns:[2,2],origins:['trigger','manual'],sameSandbox:true,separateWorktrees:true,retainedTranscriptPrefix:true,bootstrapAgent:{id:ids[2],creationOrigin:'legacy_unknown',workspaceProfileIndex:0,completedTurns:0,untouched:true}};
  const journal={schema:'cma-terminal-nondispatch-successor-journal-v1',recoveryId:directive.recoveryId,originalJournalSha256:hash(originalBytes),directiveHash:result.recovery.directiveSha256,resources,bootstrapAgentId:ids[2],predecessorEvidenceSha256,predecessorFailure:terminal,original403:{command:original.commands['continue-0'],disposition:'not_resubmitted'},startedApplicationCommit:original.commit,resumedApplicationCommit:result.commit,freshPreflightSha256:hash(fresh),commands:{[successorLabel]:{...request(successorLabel),state:'accepted',attempts:1,value:{queued:true}},'continue-1':{path:'/api/v2/responses',body:JSON.stringify({conversation:ids[1],input:'repeat marker',stream:true}),key:`${original.run}-continue-1`,state:'accepted',attempts:1,status:200}},result};
  const seal=()=>{files['result.json']=bytes(result);files['journal.json']=bytes(journal);};seal();
  return {original,result,journal,directive,files,evidence:{originalBytes,files},observation,after,seal,config:{run:original.run,origin:original.origin,expectedSubject:original.owner,markers:{manual:'MARKER'}}};
}

for(const [paused,cycles] of [[false,1],[true,1],[true,3]]) test(`successor retains original native history after ${cycles} prior cycles, paused=${paused}`,async()=>{
  const f=fixture(paused,cycles); const dir=await mkdtemp(join(tmpdir(),'successor-test-'));
  try {
    await writeFile(join(dir,'original.json'),f.evidence.originalBytes);
    await Promise.all(Object.entries(f.files).map(([name,value])=>writeFile(join(dir,name),value)));
    const loaded=await loadOwnedFixture({...f.config,fixtureJournal:join(dir,'original.json'),fixtureResult:join(dir,'result.json'),fixtureRecoveryDirectory:dir});
    assert.equal(loaded.journal.result,undefined);assert.equal(loaded.result.continuity.status,'failed');
    assert.equal(nativeIdentity(f.after.agentEvents[0],loaded.nativeBoundaries[0]).runtimeGeneration,2*(cycles+Number(paused))+1);
    assert.deepEqual(await readFile(join(dir,'original.json')),f.evidence.originalBytes);
  } finally {await rm(dir,{recursive:true,force:true});}
});

test('successor rejects changed predecessor bytes, pending command, missing reconciliation, reused key or manual duplicate',()=>{
  for(const mutate of [
    f=>{f.files['predecessor-journal.json']=Buffer.concat([f.files['predecessor-journal.json'],Buffer.from(' ')]);},
    f=>{const v=JSON.parse(f.files['terminal-capture.json']);v.results.owned_followup_command_status.rows[0].status='queued';f.files['terminal-capture.json']=bytes(v);},
    f=>{f.result.recovery.predecessorFailure.status='accepted';},
    f=>{f.journal.commands[successorLabel].key=f.original.commands['continue-0'].key;},
    f=>{f.journal.commands['continue-1'].attempts=2;},
    f=>{const v=JSON.parse(f.files['root-reconciliation.json']);v.predecessorExecutionCannotAppend=false;f.files['root-reconciliation.json']=bytes(v);},
    f=>{delete f.result.recovery.finalEvidenceSha256['after.json'];},
  ]){const f=fixture(true);mutate(f);f.seal();assert.throws(()=>validateOwnedFixture(f.config,f.original,f.result,f.evidence));}
});

test('full recorded history rejects copied conversation, undocumented generation, edited event prefix, bootstrap turn and extra lifecycle',()=>{
  for(const mutate of [
    f=>{f.after.agentEvents[0].at(-1).kind.conversation_id='copy';},
    f=>{f.after.agentEvents[0].at(-1).kind.runtime_generation=4;},
    f=>{f.after.agentEvents[0][0].kind.conversation_id='copy';},
    f=>{f.after.agentEvents[0][1].kind.previous_generation=0;},
    f=>{f.after.sandbox.lifecycle.latest_snapshot.epoch=1;},
    f=>{f.after.public[2].transcript.turns=[f.after.public[0].transcript.turns[0]];},
    f=>{f.after.public[0].agent.worktree.id='wkt_copied';},
  ]){
    const f=fixture(true,1,true);mutate(f);f.files['after.json']=bytes(f.after);f.files['resumption-lifecycle.json']=bytes({sandbox:f.after.sandbox,events:f.after.sandboxEvents});
    for(const name of ['after.json','resumption-lifecycle.json'])f.result.recovery.finalEvidenceSha256[name]=hash(f.files[name]);
    f.seal();assert.throws(()=>resolveSuccessorFixture(f.original,f.result,f.evidence));
  }
});

test('recorded retirement markers remain valid alongside lifecycle reopenings',()=>{const f=fixture(true,1,true);assert.equal(resolveSuccessorFixture(f.original,f.result,f.evidence)[0].identity.recordedRetirements.length,2);});
