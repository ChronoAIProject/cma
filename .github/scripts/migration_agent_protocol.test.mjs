import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {PassThrough,Readable} from 'node:stream';
import {fileURLToPath} from 'node:url';
import test from 'node:test';
import {commands,runProtocol} from './migration_agent_protocol.mjs';
import {STAGES} from './migration_agent_journey.mjs';
import {sessionFixture,stageCommand} from './migration_agent_session_fixture.mjs';

const encode=rows=>Buffer.from(rows.map(row=>JSON.stringify(row)+'\n').join(''));
const initialization={sequence:0,op:'initialize',payload:{private:'not-echoed'}};
const complete=[initialization,...STAGES.map((stage,i)=>({sequence:i+1,op:'stage',payload:stageCommand(stage)})),
  {sequence:5,op:'finish',payload:{}}];

test('finite protocol runs the real journey once and emits no private resource IDs',async()=>{
  const f=sessionFixture(),output=[],data=encode(complete),chunks=[];
  for(let i=0;i<data.length;i+=13)chunks.push(data.subarray(i,i+13));
  let made=0;
  const result=await runProtocol({input:Readable.from(chunks),write:async value=>output.push(value),
    makeSession:async config=>{made++;assert.deepEqual(config,initialization.payload);return f.session;}});
  assert.equal(result.exitCode,0);assert.equal(made,1);assert.equal(output.length,6);
  assert.equal(output.at(-1).result.outcome,'SESSION_OBSERVED');
  const raw=JSON.stringify(output);
  for(const value of ['not-echoed','operation-1','agent-1','sandbox-1','worktree-1'])assert.equal(raw.includes(value),false,value);
  assert.equal(f.cleanups.length,1);assert.equal(f.journal.closed,true);
});

test('protocol rejects duplicate/out-of-order/extra commands and cleans once',async()=>{
  for(const rows of [
    [initialization,{sequence:0,op:'finish',payload:{}}],
    [{sequence:0,op:'stage',payload:{}}],
    [initialization,{sequence:1,op:'unknown',payload:{}}],
    [initialization,{sequence:1,op:'finish',payload:{unexpected:true}}],
    [initialization,{sequence:1,op:'finish',payload:{}},{sequence:2,op:'finish',payload:{}}],
    [initialization,{sequence:1,op:'stage',payload:stageCommand('new-write')}],
    [initialization,{sequence:6,op:'finish',payload:{}}],
    [{...initialization,unexpected:'private'}],
  ]) {
    const f=sessionFixture(),output=[];
    const result=await runProtocol({input:Readable.from([encode(rows)]),write:async v=>output.push(v),makeSession:async()=>f.session});
    assert.equal(result.exitCode,1);assert.equal(output.at(-1).op,'failed');
    assert.ok(f.cleanups.length<=1);
    if(output[0].op==='initialized')assert.equal(f.cleanups.length,1);
  }
});

test('EOF, malformed input and failed stage preserve cleanup and private errors',async()=>{
  for(const suffix of [Buffer.alloc(0),Buffer.from('{"truncated"'),Buffer.from('not-json\n'),Buffer.from([255,10])]) {
    const f=sessionFixture(),output=[];
    const result=await runProtocol({input:Readable.from([encode([initialization]),suffix]),
      write:async v=>output.push(v),makeSession:async()=>f.session});
    assert.equal(result.exitCode,1);assert.equal(f.cleanups.length,1);
    assert.equal(output.at(-1).cleanup.outcome,'PUBLIC_CLEANUP_OBSERVED');
  }
  const f=sessionFixture(),output=[];f.state.failure='lost_create';
  const result=await runProtocol({input:Readable.from([encode(complete)]),write:async v=>output.push(v),makeSession:async()=>f.session});
  assert.equal(result.exitCode,1);assert.equal(f.cleanups[0][0].operation_id,null);
  assert.equal(output.length,2);
  const denied=[];
  await runProtocol({input:Readable.from([encode([initialization])]),write:async v=>denied.push(v),
    makeSession:async()=>{throw Error('private-secret-text');}});
  assert.equal(JSON.stringify(denied).includes('private-secret-text'),false);
});

test('bounded parser rejects oversized frames, total bytes and nonbinary input',async()=>{
  const collect=async input=>{const values=[];for await(const value of commands(input))values.push(value);return values;};
  for(const chunks of [[Buffer.from('\n')],[Buffer.from('x'.repeat(256*1024+1))],
    [Buffer.from('x'.repeat(256*1024+1)+'\n')],[Buffer.alloc(2*1024*1024+1)],['text-not-bytes']])
    await assert.rejects(collect(Readable.from(chunks)),/session_/);
  assert.deepEqual(await collect(Readable.from([Buffer.from('{"x":"😀"}\r\n')])),[{x:'😀'}]);
});

test('interrupted idle input wakes and awaits cleanup',async()=>{
  const f=sessionFixture(),input=new PassThrough(),controller=new AbortController(),output=[];
  let initialized;
  const ready=new Promise(resolve=>{initialized=resolve;});
  const running=runProtocol({input,signal:controller.signal,makeSession:async()=>f.session,
    write:async value=>{output.push(value);if(value.op==='initialized')initialized();}});
  input.write(encode([initialization]));await ready;controller.abort();
  const result=await running;
  assert.equal(result.exitCode,1);assert.equal(f.cleanups.length,1);
  assert.equal(output.at(-1).op,'failed');assert.equal(output.at(-1).reason,'session_interrupted');
  assert.equal(f.journal.closed,true);
  const already=new AbortController();already.abort();
  assert.equal((await runProtocol({input:new PassThrough(),signal:already.signal,write:async()=>{}})).exitCode,1);
});

test('cleanup, output and interrupt failures stay failures',async()=>{
  const f=sessionFixture();f.options.cleanupFactory=()=>({async run(){throw Error('cleanup_failed');}});
  const output=[];
  const fake={async stage(){throw Error('private stage failure');},async finish(){throw Error('private finish failure');}};
  assert.equal((await runProtocol({input:Readable.from([encode(complete)]),write:async v=>output.push(v),
    makeSession:async()=>fake})).exitCode,1);
  assert.equal(JSON.stringify(output).includes('private'),false);
  const g=sessionFixture();let count=0;
  await assert.rejects(runProtocol({input:Readable.from([encode(complete)]),makeSession:async()=>g.session,
    write:async()=>{count++;throw Error('stdout_closed');}}),/stdout_closed/);
  assert.equal(g.cleanups.length,1);assert.equal(count,2);
});

test('interruption failure is observed and cleanup still runs',async()=>{
  const input=new PassThrough(),controller=new AbortController(),output=[];
  let ready;const initialized=new Promise(resolve=>{ready=resolve;});let cleaned=0;
  const fake={async interrupt(){throw Error('session_interrupt_failed');},
    async finish(){cleaned++;return {outcome:'INCOMPLETE',cleanup:{outcome:'UNRESOLVED'}};}};
  const running=runProtocol({input,signal:controller.signal,makeSession:async()=>fake,
    write:async value=>{output.push(value);if(value.op==='initialized')ready();}});
  input.write(encode([initialization]));await initialized;controller.abort();
  assert.equal((await running).exitCode,1);assert.equal(cleaned,1);
  assert.equal(output.at(-1).interrupt_failure,'session_interrupt_failed');
});

test('actual CLI rejects malformed input, exits nonzero and leaves no child', {timeout:10000},async()=>{
  const path=fileURLToPath(new URL('./migration_agent_protocol.mjs',import.meta.url));
  const child=spawn(process.execPath,[path],{stdio:['pipe','pipe','pipe']});
  const done=once(child,'close');let stdout='',stderr='';
  child.stdout.on('data',v=>{stdout+=v;});child.stderr.on('data',v=>{stderr+=v;});
  child.stdin.end('malformed\n');
  const [code]=await done;assert.equal(code,1);assert.equal(stderr,'');
  assert.equal(JSON.parse(stdout).reason,'session_json_invalid');
});

test('actual CLI SIGTERM wakes idle stdin and exits with failure finality', {timeout:10000},async()=>{
  const readyCode='const running=main(); process.stdout.write("fixture-ready\\n"); await running;';
  const code='import {main} from '+JSON.stringify(new URL('./migration_agent_protocol.mjs',import.meta.url).href)+';'+readyCode;
  const child=spawn(process.execPath,['--input-type=module','-e',code],{stdio:['pipe','pipe','pipe']});
  const done=once(child,'close');let stdout='';child.stdout.on('data',v=>{stdout+=v;});
  await once(child.stdout,'data');
  child.kill('SIGTERM');
  const [exit,signal]=await done;assert.equal(exit,1);assert.equal(signal,null);
  assert.equal(JSON.parse(stdout.trim().split('\n').at(-1)).op,'failed');
});
