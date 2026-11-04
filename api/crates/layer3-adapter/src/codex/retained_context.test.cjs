const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, renameSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");

async function fixture() {
  const state = { enabled:false, calls:[], model:[], error:null, round:0 };
  const names = ["publish_document", "update_document"];
  const server = createServer(async (request,response) => {
    try {
      if (request.method === "GET") {response.writeHead(405).end();return;}
      assert.equal(request.method,"POST");
      let body=""; for await(const part of request){body+=part;assert(body.length<2*1024*1024);}
      const value=JSON.parse(body);
      if(request.url==="/mcp"){
        if(!Object.hasOwn(value,"id")){response.writeHead(202).end();return;}
        let result;
        if(value.method==="initialize") result={protocolVersion:value.params.protocolVersion,serverInfo:{name:"retained-tools",version:"1"},capabilities:{tools:{}}};
        else if(value.method==="tools/list") result={tools:(state.enabled?names:[]).map(name=>({name,description:"Retained document fixture",inputSchema:{type:"object",properties:{},additionalProperties:false},annotations:{readOnlyHint:true,destructiveHint:false}}))};
        else if(value.method==="tools/call"){
          assert(names.includes(value.params.name));state.calls.push(value.params.name);
          result={content:[{type:"text",text:"Confirmed document fixture"}]};
        }else assert.fail("unexpected MCP method "+value.method);
        response.writeHead(200,{"content-type":"application/json"});response.end(JSON.stringify({jsonrpc:"2.0",id:value.id,result}));return;
      }
      assert.equal(request.url,"/v1/responses");state.model.push(value);
      const declared=value.tools??value.input.filter(i=>i.type==="additional_tools").flatMap(i=>i.tools);
      const exec=declared.find(t=>t.name==="functions")?.tools?.find(t=>t.name==="exec");
      const output=state.enabled&&state.round++===0?names.map((name,index)=>{
        const exact="mcp__cma_access__"+name;
        if(exec){const names=JSON.parse(value.client_metadata["x-codex-turn-metadata"]).code_mode_tool_names;if(names)assert(Object.hasOwn(names,exact));}
        else assert(declared.some(tool=>tool.name===exact) || declared.find(tool=>tool.name==="mcp__cma_access")?.tools?.some(tool=>tool.name===name), "document tool was not advertised");
        return {id:"fc_"+state.model.length+"_"+index,call_id:"call_"+state.model.length+"_"+index,status:"completed",
          ...(exec?{type:"custom_tool_call",name:"exec",namespace:"functions",input:"const entry=ALL_TOOLS.find(t=>t.name==="+JSON.stringify(exact)+");text(await tools[entry.name]({}));"}
          :(declared.some(tool=>tool.name===exact)?{type:"function_call",name:exact,arguments:"{}"}:{type:"function_call",name,namespace:"mcp__cma_access",arguments:"{}"}))};
      }):[{id:"message_"+state.model.length,type:"message",role:"assistant",status:"completed",content:[{type:"output_text",text:"Retained history marker.",annotations:[]}]}];
      response.writeHead(200,{"content-type":"text/event-stream"});
      const send=event=>response.write("data: "+JSON.stringify(event)+"\n\n");
      const id="resp_retained_"+state.model.length;send({type:"response.created",response:{id,status:"in_progress",output:[]}});
      output.forEach((item,output_index)=>{send({type:"response.output_item.added",output_index,item:{...item,status:"in_progress"}});send({type:"response.output_item.done",output_index,item});});
      send({type:"response.completed",response:{id,status:"completed",output,usage:{input_tokens:2,output_tokens:1,total_tokens:3}}});response.end();
    }catch(error){state.error=error;if(!response.headersSent)response.writeHead(500);response.end();}
  });
  await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
  return {state,origin:"http://127.0.0.1:"+server.address().port,async close(){server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}};
}

test("retained pinned bridge refreshes document tools on its same thread and keeps owner work and history",{timeout:120000},async()=>{
  const binary=process.env.CMA_CODEX_TEST_BINARY??"/usr/local/bin/codex";
  assert.match(execFileSync(binary,["--version"],{encoding:"utf8"}).trim(),/^codex-cli 0\.(147|165)\.0$/);
  const directory=mkdtempSync(join(tmpdir(),"cma-retained-context-"));const home=join(directory,"home");mkdirSync(home);
  const server=await fixture();
  writeFileSync(join(home,"config.toml"),['model="gpt-6"','model_reasoning_effort="high"','model_provider="cma"','web_search="disabled"','approval_policy="never"','sandbox_mode="danger-full-access"','[model_providers.cma]','name="Retained context fixture"','base_url="'+server.origin+'/v1"','wire_api="responses"','requires_openai_auth=false'].join("\n"));
  const owner="OWNER_RETAINED_CONTEXT: preserve this user's guidance.\n";
  const guidance="MANAGED_DOCUMENT_CONTEXT: publish_document and update_document retain user documents.";
  writeFileSync(join(home,"AGENTS.md"),owner);writeFileSync(join(directory,"dirty.txt"),"Uncommitted owner work");
  const bridge=processRpc(process.execPath,[process.env.CMA_ACP_TEST_BRIDGE??bridgePath],{cwd:directory,env:{PATH:process.env.PATH,CODEX_HOME:home,CODEX_PATH:binary}});
  const tools=[{type:"http",name:"cma_access",url:server.origin+"/mcp",headers:[]}];
  try{
    await bridge.call("initialize",{protocolVersion:1,clientCapabilities:{}});
    const created=await bridge.call("session/new",{cwd:directory,mcpServers:tools});
    const prompt=()=>bridge.call("session/prompt",{sessionId:created.sessionId,prompt:[{type:"text",text:"Perform the offline document fixture."}]});
    await prompt();if(server.state.error)throw server.state.error;
    assert(JSON.stringify(server.state.model[0]).includes("OWNER_RETAINED_CONTEXT"));assert.equal(server.state.calls.length,0);
    server.state.enabled=true;
    writeFileSync(join(home,"AGENTS.md.next"),guidance+"\n\n"+owner,{mode:0o600});renameSync(join(home,"AGENTS.md.next"),join(home,"AGENTS.md"));
    // Existing methods work on the old production bridge without the new
    // cma/agent-instructions extension or a runtime image/process restart.
    await bridge.call("session/close",{sessionId:created.sessionId});
    await bridge.call("session/close",{sessionId:created.sessionId}); // ambiguous close ACK replay
    await assert.rejects(bridge.call("session/set_mode",{sessionId:created.sessionId,modeId:"agent-full-access"}),
      error => {
        assert.deepEqual(error.actual,{code:-32603,message:"Internal error",data:{details:"Session "+created.sessionId+" not found"}});
        return true;
      });
    await bridge.call("session/load",{sessionId:created.sessionId,cwd:directory,mcpServers:tools});
    await bridge.call("session/set_model",{sessionId:created.sessionId,modelId:"gpt-6[high]"});
    await bridge.call("session/set_config_option",{sessionId:created.sessionId,configId:"reasoning_effort",value:"high"});
    await bridge.call("session/set_mode",{sessionId:created.sessionId,modeId:"agent-full-access"});
    await prompt();if(server.state.error)throw server.state.error;
    assert.deepEqual(server.state.calls.sort(),["publish_document","update_document"]);
    const refreshed=JSON.stringify(server.state.model[1]);assert(refreshed.includes("MANAGED_DOCUMENT_CONTEXT"));assert(refreshed.includes("OWNER_RETAINED_CONTEXT"));assert(refreshed.includes("Retained history marker."));
    assert.equal(server.state.model[1].client_metadata.session_id,created.sessionId);
    assert.equal(server.state.model[1].client_metadata.thread_id,created.sessionId);
    assert.equal(server.state.model[1].model,"gpt-6");assert.equal(server.state.model[1].reasoning.effort,"high");
    assert.equal(readFileSync(join(directory,"dirty.txt"),"utf8"),"Uncommitted owner work");
    assert.equal(readFileSync(join(home,"AGENTS.md"),"utf8"),guidance+"\n\n"+owner);
  }finally{await bridge.close();await server.close();rmSync(directory,{recursive:true,force:true,maxRetries:10,retryDelay:100});}
});
