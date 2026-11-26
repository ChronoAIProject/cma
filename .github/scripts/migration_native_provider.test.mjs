import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {randomBytes} from 'node:crypto';
import {once} from 'node:events';
import {fileURLToPath} from 'node:url';
import test from 'node:test';
import {LocalProviderControl} from './migration_agent_adapter.mjs';

test('actual Python provider shares control state with the adapter and SSE model route', {timeout:15000}, async () => {
  const token=randomBytes(32).toString('hex');
  const child=spawn('python3',['-u','-B','-c',[
    'from http.server import ThreadingHTTPServer',
    'from migration_native_provider import Handler',
    "server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)",
    'print(server.server_port, flush=True)',
    'server.serve_forever()',
  ].join('\n')], {cwd:fileURLToPath(new URL('.',import.meta.url)),
    env:{...process.env,WS4_PROVIDER_CONTROL_TOKEN:token,WS4_PROVIDER_BEARER:'',WS4_PROVIDER_ALLOW_BOOTSTRAP:'false'},
    stdio:['ignore','pipe','pipe']});
  const closed=once(child,'close');
  let stderr='';child.stderr.on('data',chunk=>{stderr+=chunk.toString();});
  let reader;
  try {
    const port=await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(Error('provider_start_timeout')),3000);
      child.once('error',error=>{clearTimeout(timer);reject(error);});
      child.stdout.once('data',chunk=>{
        clearTimeout(timer);
        const value=chunk.toString().trim();
        if(!/^\d{1,5}$/.test(value)) reject(Error('provider_port_invalid'));
        else resolve(Number(value));
      });
    });
    const origin='http://127.0.0.1:'+port;
    const adapter=new LocalProviderControl({origin,token});
    const caseId='migration-provider-contract',marker='MIGRATION_'+randomBytes(16).toString('hex');
    const initial=await adapter.prepareTurn({caseId,marker});
    assert.equal(initial.request_count,0);
    const response=await fetch(origin+'/v1/responses',{method:'POST',
      headers:{'Content-Type':'application/json'},body:JSON.stringify({input:marker,stream:true}),
      signal:AbortSignal.timeout(5000)});
    assert.equal(response.status,200);
    reader=response.body.getReader();
    const first=await reader.read();
    assert.equal(first.done,false);
    const held=await adapter.oracle({caseId});
    assert.equal(held.request_count,1);
    assert.equal(held.active_streams,1);
    assert.equal(held.completed_effect_count,0);
    await adapter.releaseTurn({caseId});
    let bytes=Buffer.from(first.value);
    for(;;) {
      const chunk=await reader.read();
      if(chunk.done) break;
      bytes=Buffer.concat([bytes,chunk.value]);
      assert.ok(bytes.length<65536);
    }
    const events=bytes.toString().split('\n').filter(line=>line.startsWith('data: '))
      .map(line=>JSON.parse(line.slice(6)));
    assert.equal(events.at(-1).type,'response.completed');
    assert.ok(events.at(-1).response.output[0].content[0].text.includes(marker));
    const completed=await adapter.oracle({caseId});
    assert.equal(completed.completed_effect_count,1);
    assert.equal(completed.active_streams,0);
    assert.equal(completed.disconnect_count,0);
    assert.equal('request_hashes' in completed,false);
    const wrong=new LocalProviderControl({origin,token:randomBytes(32).toString('hex')});
    await assert.rejects(wrong.oracle({caseId}),/provider_control_status_401/);
    await assert.rejects(adapter.prepareTurn({caseId,marker}),/provider_control_status_409/);
    assert.equal(stderr,'');
  } finally {
    try {
      if(reader) await reader.cancel();
    } finally {
      child.kill('SIGTERM');
      const timer=setTimeout(()=>child.kill('SIGKILL'),2000);
      try {await closed;} finally {clearTimeout(timer);}
    }
  }
});
