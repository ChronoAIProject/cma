const assert = require("node:assert/strict");
const { spawn, execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { createInterface } = require("node:readline");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { runtimeObservations, usageAfter } = require("./runtime_observations.cjs");
const { stop } = require("./native_tools_fixture.cjs");
const { controlledBinary, control } = require("./native_control_fixture.cjs");

test("observation patch refuses unknown bytes and forwards real cached input without inventing counts", () => {
  assert.throws(() => runtimeObservations("unknown source"), /Unexpected ACP source/);
  const project = new Function("params", "sessionState", usageAfter.replaceAll("this.sessionState", "sessionState"));
  const params = {turnId:"t", tokenUsage:{total:{inputTokens:100,outputTokens:20,totalTokens:120},last:{inputTokens:40,outputTokens:5,totalTokens:45},modelContextWindow:1000}};
  const value = project(params,{})._meta.cma.usageSnapshot;
  assert.equal(value.inputTokens,100); assert.equal(value.lastInputTokens,40);
  params.tokenUsage.last.inputTokens = null; assert.equal(project(params,{}),null);
});

function connect(program, directory, home, binary, frames) {
  const child = spawn(process.execPath,[program],{cwd:directory,env:{PATH:process.env.PATH,CODEX_HOME:home,CODEX_PATH:binary},detached:true,stdio:["pipe","pipe","pipe"]});
  child.stderr.resume(); const pending=new Map(); let serial=0;
  const listeners=new Set();
  const lines=createInterface({input:child.stdout});
  lines.on("line",line=>{const frame=JSON.parse(line);frames.push(frame);pending.get(frame.id)?.(frame);for(const listener of listeners)listener(frame);});
  return {
    async call(method,params){const id=++serial;let timer;const promise=new Promise(resolve=>pending.set(id,resolve));
      child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");
      try {const frame=await Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error("Fixture deadline: "+method)),30000);})]);assert.equal(frame.error,undefined,JSON.stringify(frame.error));return frame.result;}
      finally{clearTimeout(timer);pending.delete(id);}},
    async waitFor(predicate){
      if(frames.some(predicate))return;
      await new Promise((resolve,reject)=>{
        const receive=frame=>{if(predicate(frame)){clearTimeout(timer);listeners.delete(receive);resolve();}};
        const timer=setTimeout(()=>{listeners.delete(receive);reject(new Error("Observation deadline"));},10000);
        listeners.add(receive);
      });
    },
    async close(){lines.close();await stop(child);}
  };
}

test("pinned provider emits canonical cwd and cumulative real usage across turns and process resume", {timeout:120000}, async()=>{
  const binary=process.env.CMA_CODEX_TEST_BINARY??"/usr/local/bin/codex";
  assert.equal(execFileSync(binary,["--version"],{encoding:"utf8"}).trim(),"codex-cli 0.147.0");
  const directory=mkdtempSync(join(tmpdir(),"cma-observations-")),home=join(directory,"agent-home"),other=join(directory,"other");
  mkdirSync(home);mkdirSync(other);const frames=[];let calls=0,error;
  const server=createServer(async(req,res)=>{try{
    let body="";for await(const chunk of req)body+=chunk;assert.equal(req.url,"/v1/responses");JSON.parse(body);calls++;
    res.writeHead(200,{"content-type":"text/event-stream"});const send=e=>res.write("data: "+JSON.stringify(e)+"\n\n");
    const id="resp_observation_"+calls,item={id:"msg_observation_"+calls,type:"message",role:"assistant",status:"completed",content:[{type:"output_text",text:"Observed.",annotations:[]}]};
    send({type:"response.created",response:{id,status:"in_progress",output:[]}});
    send({type:"response.output_item.added",output_index:0,item:{...item,status:"in_progress"}});send({type:"response.output_item.done",output_index:0,item});
    send({type:"response.completed",response:{id,status:"completed",output:[item],usage:{input_tokens:10,output_tokens:3,total_tokens:13,input_tokens_details:{cached_tokens:4}}}});res.end();
  }catch(e){error=e;if(!res.headersSent)res.writeHead(500);res.end();}});
  await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
  writeFileSync(join(home,"config.toml"),['model = "gpt-5.6-sol"','model_provider = "cma_test"','web_search = "disabled"','approval_policy = "never"','sandbox_mode = "danger-full-access"','[model_providers.cma_test]','name = "Offline observation fixture"','base_url = "http://127.0.0.1:'+server.address().port+'/v1"','wire_api = "responses"','requires_openai_auth = false'].join("\n"));
  const program=process.env.CMA_ACP_TEST_BRIDGE??bridgePath;let bridge;
  const native=controlledBinary(directory,binary);
  const open=async()=>{bridge=connect(program,directory,home,native,frames);await bridge.call("initialize",{protocolVersion:1,clientCapabilities:{}});};
  const snapshots=()=>frames.map(f=>f.params?.update?._meta?.cma?.usageSnapshot).filter(Boolean);
  try{
    await open();const session=await bridge.call("session/new",{cwd:directory,mcpServers:[]});
    assert(frames.some(f=>f.params?.update?._meta?.cma?.runtimeMetadata?.workingDirectory===directory));
    // The unchanged persistence patch must also retain an empty conversation.
    await bridge.close();await open();
    await bridge.call("session/load",{sessionId:session.sessionId,cwd:directory,mcpServers:[]});
    const input="Describe the workspace\nbriefly.";
    const prompt=()=>bridge.call("session/prompt",{sessionId:session.sessionId,prompt:[{type:"text",text:input}]});
    const metadata=()=>frames.map(f=>f.params?.update?._meta?.cma?.runtimeMetadata).filter(Boolean);
    await prompt();await prompt();if(error)throw error;
    assert.equal(calls,2);assert.deepEqual(snapshots().map(v=>[v.inputTokens,v.outputTokens]),[[10,3],[20,6]]);
    assert(metadata().some(m=>m.description==="Describe the workspace briefly." &&
      m.descriptionSource==="prompt_preview" && m.descriptionInput===input && m.descriptionInputPosition==="current"));
    await bridge.close();await open();
    const previewReload=frames.length;
    await bridge.call("session/load",{sessionId:session.sessionId,cwd:directory,mcpServers:[]});
    assert(frames.slice(previewReload).some(f=>{
      const m=f.params?.update?._meta?.cma?.runtimeMetadata;
      return m?.descriptionSource==="prompt_preview" && m.descriptionInput===input && m.descriptionInputPosition==="first";
    }));
    // These are genuine native requests on the same process and thread.
    await control(directory,"thread/settings/update",{threadId:session.sessionId,cwd:other});
    await control(directory,"thread/name/set",{threadId:session.sessionId,name:"Describe the workspace briefly."});
    await bridge.waitFor(f=>f.params?.update?._meta?.cma?.runtimeMetadata?.workingDirectory===other);
    await bridge.waitFor(f=>f.params?.update?._meta?.cma?.runtimeMetadata?.descriptionSource==="provider_title");
    assert(metadata().some(m=>m.workingDirectory===other));
    assert(metadata().some(m=>m.description==="Describe the workspace briefly." && m.descriptionSource==="provider_title"));
    await bridge.close();await open();
    const resumeStart=frames.length;
    await bridge.call("session/load",{sessionId:session.sessionId,cwd:other,mcpServers:[]});
    assert(frames.slice(resumeStart).some(f=>f.params?.update?._meta?.cma?.runtimeMetadata?.descriptionSource==="provider_title"));
    assert(frames.some(f=>f.params?.update?._meta?.cma?.runtimeMetadata?.workingDirectory===other));
    await prompt();if(error)throw error;
    assert.equal(snapshots().at(-1).inputTokens,30);assert.equal(snapshots().at(-1).outputTokens,9);
  }finally{await bridge?.close();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));rmSync(directory,{recursive:true,force:true});}
});
