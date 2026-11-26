import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,realpath,readFile,writeFile,rm,chmod,symlink,appendFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
import {Readable,Writable} from 'node:stream';
import {hash} from './migration_agent_observations.mjs';
import {PrivateAgentJournal} from './migration_agent_journal.mjs';
import {captureJoin,journalPrefix,joinDocuments,main} from './migration_agent_document_join.mjs';

const selected={source:'a'.repeat(40),image:'example.invalid/cma@sha256:'+'b'.repeat(64),
  config:'sha256:'+'c'.repeat(64)};
const owner='native-am-fixture-owner';
const checksum=bytes=>createHash('sha256').update(bytes).digest('hex');

function fixture(count=1) {
  const snapshot={schema:'migration-agent-documents-v1',database:'matrix',collections:{},document_hashes:{}};
  const resources=[];
  function add(name,row) {
    (snapshot.collections[name]??=[]).push(row);
    (snapshot.document_hashes[name]??=[]).push({id_sha256:hash(row._id),document_sha256:hash(row)});
  }
  function aggregate(name,id,fields={},record={}) {
    add(name,{_id:id,owner,...fields,document_schema_version:{$numberLong:'1'},
      record:{id,owner,...fields,...record}});
  }
  for(let i=0;i<count;i++) {
    const agent_id='agt_'+i,operation_id='op_'+i;
    const lineage={agent_id,sandbox_id:'sbx_'+i,workspace_id:'wsp_'+i,worktree_id:'wtr_'+i,
      worktree_path:'/home/test/'+i,created_at:'2026-11-18T00:00:00Z'};
    resources.push({agent_id,operation_id,writer:i===0?'old-write':'new-write',lineage});
    add('agent_lifecycle_operations',{_id:operation_id,owner,agent_id,sandbox_id:lineage.sandbox_id,status:'succeeded'});
    aggregate('agents',agent_id,Object.fromEntries(['sandbox_id','workspace_id','worktree_id'].map(k=>[k,lineage[k]])),
      {created_at:lineage.created_at});
    aggregate('sandboxes',lineage.sandbox_id);
    aggregate('workspaces',lineage.workspace_id,{sandbox_id:lineage.sandbox_id});
    aggregate('worktrees',lineage.worktree_id,{workspace_id:lineage.workspace_id,sandbox_id:lineage.sandbox_id},
      {path:lineage.worktree_path});
  }
  return {snapshot,resources};
}

async function files(t,count=1) {
  const directory=await realpath(await mkdtemp(join(tmpdir(),'migration-document-join-')));
  t.after(()=>rm(directory,{recursive:true,force:true}));
  const stage=count===1?'old-write':'old-read-new-write',{snapshot,resources}=fixture(count);
  const journal=await PrivateAgentJournal.create(directory,[]);
  // Exercise the original JavaScript canonicalizer, including Unicode/numbers
  // that would differ under a Python json.dumps approximation.
  await journal.append({type:'fixture_start',elapsed:1e-7,label:'準備'},[]);
  const result={stage,artifact:selected,outcome:'API_JOURNEY_OBSERVED'};
  await journal.append({type:'session_stage_result',result},resources);
  const session={stage,result,resources_sha256:hash(resources),journal:journal.summary()};
  await journal.close();
  const raw=JSON.stringify(snapshot),path=join(directory,'agent-documents-'+stage+'.private.json');
  await writeFile(path,raw,{mode:0o600});
  return {directory,stage,selected,owner,session,snapshot_sha256:checksum(raw)};
}

test('actual private journal prefix joins original and new resources without claiming compatibility',async t=>{
  for(const count of [1,2]) {
    const input=await files(t,count),result=await captureJoin(input);
    assert.equal(result.outcome,'JOURNAL_IDENTITIES_JOINED');assert.equal(result.compatibility,'NOT_PROVEN');
    assert.equal(result.resources.length,count);assert.equal(result.joined_collections.length,5);
    assert.equal(result.session_tail_sha256,input.session.journal.tail_sha256);
    assert.equal(result.snapshot_sha256,input.snapshot_sha256);
    assert.ok(!JSON.stringify(result).includes('agt_0'));assert.ok(!JSON.stringify(result).includes(owner));
  }
});

test('journal may grow after acknowledged prefix without relabeling that prefix',async t=>{
  const input=await files(t);
  await appendFile(join(input.directory,'agent-session.private.jsonl'),'unacknowledged trailing data\n');
  assert.equal((await captureJoin(input)).resources.length,1);
});

test('tampered, truncated, foreign or wrong sequence journal cannot join',async t=>{
  const input=await files(t),raw=await readFile(join(input.directory,'agent-session.private.jsonl'));
  for(const mutate of [
    v=>v.journal.records++,v=>v.journal.bytes--,v=>v.journal.tail_sha256='0'.repeat(64),
    v=>v.resources_sha256='0'.repeat(64),v=>v.stage='new-write',v=>v.result.artifact.source='f'.repeat(40),
    v=>v.journal.schema='other',v=>v.journal.bytes=65*1024**2,
  ]) {
    const session=structuredClone(input.session);mutate(session);
    assert.throws(()=>journalPrefix(raw,session));
  }
  const changed=raw.toString().replace('fixture_start','fixture_other');
  assert.throws(()=>journalPrefix(Buffer.from(changed),{...input.session,
    journal:{...input.session.journal,bytes:Buffer.byteLength(changed)}}),/chain/);
});

test('empty unrelated rows, duplicate rows and missing original hashes fail',()=>{
  for(const mutate of [
    v=>v.snapshot.collections.agents=[],v=>v.snapshot.collections.agents[0]._id='unrelated',
    v=>v.snapshot.collections.agents.push(v.snapshot.collections.agents[0]),
    v=>v.snapshot.document_hashes.agents=[],v=>v.snapshot.document_hashes.agents[0].id_sha256=hash('other'),
    v=>v.resources=[],v=>v.snapshot.database='foreign',v=>v.resources.push(v.resources[0]),
  ]) {
    const value=fixture();mutate(value);
    assert.throws(()=>joinDocuments(value.snapshot,value.resources,{stage:'old-write',selected,owner}));
  }
});

test('all projected and nested ownership/lineage fields must match the admitted resource',()=>{
  const cases=[
    ['agent_lifecycle_operations','owner'],['agent_lifecycle_operations','agent_id'],
    ['agent_lifecycle_operations','sandbox_id'],['agent_lifecycle_operations','status'],
    ['agents','owner'],['agents','record','id'],['agents','sandbox_id'],['agents','record','worktree_id'],
    ['agents','record','created_at'],['sandboxes','record','owner'],['workspaces','sandbox_id'],
    ['worktrees','record','workspace_id'],['worktrees','record','path'],['worktrees','record','sandbox_id'],
  ];
  for(const [collection,...keys] of cases) {
    const {snapshot,resources}=fixture();let row=snapshot.collections[collection][0];
    for(const key of keys.slice(0,-1))row=row[key];row[keys.at(-1)]='wrong';
    assert.throws(()=>joinDocuments(snapshot,resources,{stage:'old-write',selected,owner}));
  }
});

test('second resource must be the new writer with distinct Agent identity',()=>{
  for(const mutate of [r=>r[1].writer='old-write',r=>r[1].agent_id=r[0].agent_id,
    r=>delete r[1].lineage,r=>r[1].lineage.sandbox_id='../foreign']) {
    const {snapshot,resources}=fixture(2);mutate(resources);
    assert.throws(()=>joinDocuments(snapshot,resources,{stage:'old-read-new-write',selected,owner}));
  }
});

test('private paths, file modes, byte bounds and snapshot digest are enforced',async t=>{
  const input=await files(t),path=join(input.directory,'agent-documents-old-write.private.json');
  await assert.rejects(captureJoin({...input,directory:input.directory+'/../'+input.directory.split('/').at(-1)}));
  await assert.rejects(captureJoin({...input,snapshot_sha256:'f'.repeat(64)}),/changed/);
  await assert.rejects(captureJoin({...input,stage:'../foreign'}));
  await chmod(path,0o644);await assert.rejects(captureJoin(input),/file_invalid/);await chmod(path,0o600);
  const raw=await readFile(path);await writeFile(path,Buffer.alloc(0));
  await assert.rejects(captureJoin(input),/file_invalid/);await writeFile(path,raw);
  const link=join(input.directory,'alias');await symlink(input.directory,link);
  await assert.rejects(captureJoin({...input,directory:link}),/directory_invalid/);
  await chmod(input.directory,0o755);await assert.rejects(captureJoin(input),/directory_invalid/);
  await chmod(input.directory,0o700);
});

test('bounded CLI emits hashes only on success and no raw errors on failure',async t=>{
  const input=await files(t);
  for(const [payload,exit] of [[JSON.stringify(input),0],['{',1],['x'.repeat(1024**2+1),1]]) {
    let output='';
    const code=await main(Readable.from([Buffer.from(payload)]),new Writable({write(b,_e,done){output+=b;done();}}));
    assert.equal(code,exit);const result=JSON.parse(output);
    assert.equal(result.outcome,exit?'INCOMPLETE':'JOURNAL_IDENTITIES_JOINED');
    assert.ok(!output.includes(input.directory));assert.ok(!output.includes('agt_0'));
  }
});
