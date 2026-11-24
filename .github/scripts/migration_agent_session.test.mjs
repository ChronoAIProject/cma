import assert from 'node:assert/strict';
import {mkdtemp,chmod,readFile,rm,symlink,stat,realpath,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import test from 'node:test';
import {AgentSession,bootBinding,createSession,exactKeys} from './migration_agent_session.mjs';
import {PrivateAgentJournal} from './migration_agent_journal.mjs';
import {STAGES} from './migration_agent_journey.mjs';
import {hash} from './migration_agent_observations.mjs';
import {artifacts} from './migration_agent_test_fixture.mjs';
import {sessionFixture,stageCommand} from './migration_agent_session_fixture.mjs';

async function directory(t) {
  const path=await realpath(await mkdtemp(join(tmpdir(),'migration-session-test-')));
  await chmod(path,0o700);t.after(()=>rm(path,{recursive:true,force:true}));return path;
}

test('one real journey retains both Agents and all replies through old/new/old',async()=>{
  const f=sessionFixture();
  for(const stage of STAGES) {
    const result=await f.session.stage(stageCommand(stage));
    assert.equal(result.stage,stage);
    assert.equal(result.result.reader,undefined);
    assert.match(result.result.reader_sha256,/^[a-f0-9]{64}$/);
  }
  const records=f.session.resources();
  assert.equal(records.length,2);
  assert.deepEqual(records.map(r=>r.responses.length),[4,2]);
  const result=await f.session.finish();
  assert.equal(result.outcome,'SESSION_OBSERVED');
  assert.equal(result.full_reader_writer_gate,'NOT_PROVEN');
  assert.equal(result.persisted_collection_coverage,'NOT_PROVEN');
  assert.equal(result.stages.length,4);
  assert.equal(f.cleanups.length,1);assert.deepEqual(f.cleanups[0],records);
  assert.equal(f.journal.closed,true);
  assert.equal(f.api.rebinds.length,4);
  assert.ok(f.journal.events.some(e=>e.resources.length===2));
  assert.equal(f.state.calls.filter(c=>c.method==='POST').length,2);
  await assert.rejects(f.session.finish(),/finish_once/);
  await assert.rejects(f.session.stage(stageCommand('old-write')),/stage_refused/);
});

test('failed/lost admission poisons stages and preserves custody for cleanup',async()=>{
  for(const failure of ['lost_create','lost_send','request','release','stream']) {
    const f=sessionFixture();f.state.failure=failure;
    await assert.rejects(f.session.stage(stageCommand('old-write')));
    await assert.rejects(f.session.stage(stageCommand('old-write')),/stage_refused/);
    const resources=f.session.resources();
    const result=await f.session.finish('session_failed');
    assert.equal(result.outcome,'INCOMPLETE');
    assert.equal(f.cleanups.length,1);
    assert.deepEqual(f.cleanups[0],resources);
    if(failure==='lost_create') {
      assert.equal(resources.length,1);assert.equal(resources[0].operation_id,null);
    }
  }
});

test('stage misuse, evidence failure and concurrent commands fail before mutation',async()=>{
  for(const mutate of [c=>c.stage='new-write',c=>c.extra=true,c=>c.reader.artifact=artifacts.new,
    c=>c.origin='https://production.invalid',c=>c.boot.failed_boots=1,c=>c.boot=null]) {
    const f=sessionFixture(),command=stageCommand('old-write');mutate(command);
    await assert.rejects(f.session.stage(command));
    assert.equal(f.state.calls.length,0);
    assert.equal((await f.session.finish()).outcome,'INCOMPLETE');
  }
  const f=sessionFixture();f.journal.fail='session_stage_binding';
  await assert.rejects(f.session.stage(stageCommand('old-write')),/journal_injected/);
  assert.equal(f.state.calls.length,0);await f.session.finish();
  const g=sessionFixture();let release;
  const original=g.journal.append.bind(g.journal);
  g.journal.append=async(event,resources)=>{
    if(event.type==='session_stage_binding')await new Promise(resolve=>{release=resolve;});
    return original(event,resources);
  };
  const running=g.session.stage(stageCommand('old-write'));
  await assert.rejects(g.session.stage(stageCommand('old-write')),/stage_refused/);
  await assert.rejects(g.session.finish(),/finish_once/);release();await running;await g.session.finish();
});

test('interruption rejects future writes but permits finite cleanup',async()=>{
  const f=sessionFixture();await f.session.interrupt();
  await assert.rejects(f.session.stage(stageCommand('old-write')),/stage_refused/);
  assert.equal((await f.session.finish('session_interrupted')).outcome,'INCOMPLETE');
  const g=sessionFixture();
  const append=g.journal.append.bind(g.journal);
  g.journal.append=async(event,resources)=>{
    await append(event,resources);
    if(event.type==='create_intent')await g.session.interrupt();
  };
  await assert.rejects(g.session.stage(stageCommand('old-write')),/journey_interrupted/);
  assert.equal(g.state.calls.filter(c=>c.method==='POST').length,0);
  await g.session.finish('session_interrupted');
});

test('new writer keeps the same reader and boot must retain every attempt',async()=>{
  const valid=stageCommand('old-write');
  bootBinding(valid.stage,valid.reader,valid.boot,artifacts.old,null);
  const mutations=[
    c=>c.reader.podUid='other',c=>c.reader.podName='foreign',c=>c.reader.extra=1,
    c=>c.reader.localImageAlias='arbitrary:tag',c=>c.boot.start_resource_version='',
    c=>c.boot.attempted_uids=[],c=>c.boot.ready_uids=[],c=>c.boot.observations=[],
    c=>c.boot.observations[0].restart_count=1,c=>c.boot.observations[0].image=artifacts.new.image,
    c=>c.boot.observations[0].phase='Failed',c=>c.boot.observations[0].type='MODIFIED',
    c=>c.boot.observations[0].phase='Pending',c=>c.boot.observations[0].name='foreign',
    c=>{c.boot.observations[0].ready=false;c.boot.observations.push({...c.boot.observations[0],type:'DELETED'});},
    c=>{c.boot.attempted_uids.push('pending');c.boot.observations.push({...c.boot.observations[0],
      uid:'pending',name:'api-pending',phase:'Pending',ready:false});},
  ];
  for(const mutate of mutations) {
    const c=structuredClone(valid);mutate(c);
    assert.throws(()=>bootBinding(c.stage,c.reader,c.boot,artifacts.old,null));
  }
  assert.throws(()=>bootBinding(valid.stage,valid.reader,valid.boot,artifacts.old,valid.reader));
  const second=stageCommand('new-read-old-write'),third=stageCommand('new-write');
  bootBinding(third.stage,third.reader,third.boot,artifacts.new,second.reader);
  assert.throws(()=>bootBinding(third.stage,third.reader,{},artifacts.new,second.reader));
  assert.throws(()=>bootBinding(third.stage,{...third.reader,podUid:'other'},null,artifacts.new,second.reader));
});

test('cleanup and stream/provider failures cannot produce session PASS',async()=>{
  const f=sessionFixture();for(const stage of STAGES)await f.session.stage(stageCommand(stage));
  f.api.close=async()=>{throw Error('close_failed');};
  f.provider.oracle=async()=>{throw Error('provider_failed');};
  const result=await f.session.finish();assert.equal(result.outcome,'INCOMPLETE');
  assert.ok(result.failures.includes('close_failed'));
  assert.equal(result.failures.filter(v=>v==='provider_failed').length,6);
  const g=sessionFixture();g.options.cleanupFactory=()=>({async run(){return {outcome:'UNRESOLVED'};}});
  const s=new AgentSession(g.options);assert.equal((await s.finish()).outcome,'INCOMPLETE');
  const h=sessionFixture();h.options.cleanupFactory=()=>({async run(){throw Error('cleanup_failed');}});
  const bad=new AgentSession(h.options);await assert.rejects(bad.finish(),/cleanup_failed/);
  assert.equal(h.journal.closed,true);
  const leftover=sessionFixture();for(const stage of STAGES)await leftover.session.stage(stageCommand(stage));
  leftover.api.close=async()=>({joined:1,failed:1});
  const joined=await leftover.session.finish();assert.equal(joined.outcome,'INCOMPLETE');
  assert.ok(joined.failures.includes('session_stream_finality_failed'));
});

test('journal is exclusive, mode0600, hash chained, redacted and fsynced',async t=>{
  const dir=await directory(t),secret='secret-with-"quotes"-and-\\-slash';
  const journal=await PrivateAgentJournal.create(dir,[secret]);
  await journal.append({type:'one',secret,prefix:'Bearer '+secret},[{agent_id:'synthetic-agent'}]);
  await journal.append({type:'two'},[]);
  assert.equal(journal.summary().records,2);await journal.close();await journal.close();
  const path=join(dir,'agent-session.private.jsonl');
  assert.equal((await stat(path)).mode&0o777,0o600);
  const text=await readFile(path,'utf8');assert.equal(text.includes(JSON.stringify(secret).slice(1,-1)),false);
  const rows=text.trim().split('\n').map(JSON.parse);
  let previous=null;
  for(const row of rows) {const {sha256,...content}=row;assert.equal(hash(content),sha256);
    assert.equal(row.previous,previous);previous=sha256;}
  assert.equal(rows[0].event.secret,'[REDACTED]');
  await assert.rejects(journal.append({}),/not_writable/);
  await assert.rejects(PrivateAgentJournal.create(dir,[]),/EEXIST/);
});

test('journal rejects directory exposure, symlink, oversized and concurrent writes',async t=>{
  const dir=await directory(t);
  await chmod(dir,0o755);await assert.rejects(PrivateAgentJournal.create(dir,[]),/private_campaign/);
  await chmod(dir,0o700);
  const alias=dir+'-link';await symlink(dir,alias);t.after(()=>rm(alias,{force:true}));
  await assert.rejects(PrivateAgentJournal.create(alias,[]),/private_campaign/);
  await assert.rejects(PrivateAgentJournal.create(dir,['short']),/redaction_contract/);
  const journal=await PrivateAgentJournal.create(dir,[]);
  await assert.rejects(journal.append({large:'x'.repeat(4*1024*1024)}),/event_bound/);
  const write=journal.append({ok:true});
  await assert.rejects(journal.append({second:true}),/not_writable/);
  await assert.rejects(journal.close(),/write_in_progress/);await write;await journal.close();
});

test('journal failure poisons later writes and cannot manufacture a complete chain',async()=>{
  for(const failure of ['write','sync']) {
    let writes=0,closed=false;
    const journal=new PrivateAgentJournal({async writeFile(){writes++;if(failure==='write')throw Error('disk_failed');},
      async sync(){if(failure==='sync')throw Error('sync_failed');},async close(){closed=true;}},[]);
    await assert.rejects(journal.append({type:'intent'}),/failed/);
    await assert.rejects(journal.append({type:'retry'}),/not_writable/);
    assert.equal(writes,1);assert.equal(journal.summary().records,0);await journal.close();assert.equal(closed,true);
  }
});

test('real initialization rejects foreign kubeconfig and closes journal',async t=>{
  const dir=await directory(t);
  const config={artifacts,directory:dir,origin:'http://127.0.0.1:14000',bearer:'x'.repeat(32),
    owner:'fixture-owner',providerOrigin:'http://127.0.0.1:14001',providerToken:'y'.repeat(32),
    kubeconfig:join(dir,'missing'),context:'production',server:'https://127.0.0.1:14002',namespaceUid:'uid'};
  await assert.rejects(createSession({...config,extra:true}),/command_shape/);
  await assert.rejects(createSession(config),/owned_kind_context/);
  const events=await readFile(join(dir,'agent-session.private.jsonl'),'utf8');assert.equal(events,'');
  for(const value of [null,[],{}, {a:1,b:2}])assert.throws(()=>exactKeys(value,['a']));
});

test('default kubectl subprocess initializes from a scripted local cluster then closes cleanly',async t=>{
  const dir=await directory(t),kubeconfig=join(dir,'kubeconfig.private');
  const config={artifacts,directory:dir,origin:'http://127.0.0.1:14000',bearer:'x'.repeat(32),
    owner:'fixture-owner',providerOrigin:'http://127.0.0.1:14001',providerToken:'y'.repeat(32),
    kubeconfig,context:'kind-cma-matrix-0123456789',server:'https://127.0.0.1:14002',namespaceUid:'fixture-uid'};
  const raw={'current-context':config.context,contexts:[{name:config.context,context:{cluster:'fixture',user:'fixture'}}],
    clusters:[{name:'fixture',cluster:{server:config.server,'certificate-authority-data':'offline-test-placeholder'}}],
    users:[{name:'fixture',user:{'client-certificate-data':'offline-test-placeholder','client-key-data':'offline-test-placeholder'}}]};
  await writeFile(kubeconfig,JSON.stringify(raw),{mode:0o600});
  const script='#!/usr/bin/env node\n'+[
    'const args=process.argv.slice(2);',
    'if(args.includes("config")) process.stdout.write('+JSON.stringify(JSON.stringify(raw))+');',
    'else if(args.includes("namespace")) process.stdout.write('+JSON.stringify(JSON.stringify({kind:'Namespace',metadata:{
      name:'cma-migration',uid:'fixture-uid'}}))+');',
    'else { process.stderr.write("unexpected fixture command");process.exitCode=1; }',
  ].join('\n');
  await writeFile(join(dir,'kubectl'),script,{mode:0o700});
  const original=process.env.PATH;process.env.PATH=dir+':'+original;
  try {
    const session=await createSession(config);
    const result=await session.finish();
    assert.equal(result.outcome,'INCOMPLETE');assert.equal(result.cleanup.resources.length,0);
  } finally {process.env.PATH=original;}
});
