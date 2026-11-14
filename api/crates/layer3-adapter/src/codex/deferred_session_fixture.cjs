const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");

async function metadataFixture() {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-deferred-metadata-"));
  const home = join(directory, "home"); mkdirSync(home);
  const control = join(directory, "control.json"), trace = join(directory, "trace.jsonl");
  writeFileSync(control, "{}"); writeFileSync(trace, "");
  const proxy = join(directory, "native-proxy.cjs");
  // Hold exact native replies, allowing unrelated native requests to proceed.
  // No native metadata, input or credential payload is retained in the trace.
  writeFileSync(proxy, `#!${process.execPath}
const {spawn}=require("node:child_process"),{createInterface}=require("node:readline");
const {readFileSync,appendFileSync}=require("node:fs");
const child=spawn(${JSON.stringify(binary)},process.argv.slice(2),{stdio:["pipe","pipe","inherit"]});
const pending=new Map();
let epoch,counts={};
createInterface({input:process.stdin}).on("line",line=>{
 const frame=JSON.parse(line), config=JSON.parse(readFileSync(${JSON.stringify(control)},"utf8"));
 if(epoch!==config.epoch){epoch=config.epoch;counts={};}
 const kind=frame.method==="skills/list"&&!frame.params?.forceReload?"commands":frame.method==="thread/goal/get"?"goal":
  ({"model/list":"models","thread/start":"open","thread/name/set":"persist","thread/read":"history"})[frame.method]??null;
 if(kind&&config[kind]&&(counts[kind]??0)<(config.limit??Infinity)){counts[kind]=(counts[kind]??0)+1;pending.set(frame.id,{kind,delay:config[kind]});}
 appendFileSync(${JSON.stringify(trace)},JSON.stringify({event:"request",method:frame.method,optional:kind==="commands"||kind==="goal"?kind:null,forced:frame.params?.forceReload===true})+"\\n");
 child.stdin.write(line+"\\n");
});
createInterface({input:child.stdout}).on("line",line=>{
 const frame=JSON.parse(line),hold=pending.get(frame.id);
 if(!hold){process.stdout.write(line+"\\n");return;}
 pending.delete(frame.id);
 appendFileSync(${JSON.stringify(trace)},JSON.stringify({event:"held",kind:hold.kind})+"\\n");
 const release=()=>{const current=JSON.parse(readFileSync(${JSON.stringify(control)},"utf8"));
  if(current[hold.kind]==="hold")return setTimeout(release,10);
  process.stdout.write(line+"\\n");appendFileSync(${JSON.stringify(trace)},JSON.stringify({event:"released",kind:hold.kind})+"\\n");};
 if(hold.delay==="hold")setTimeout(release,10);else setTimeout(release,hold.delay);
});
process.stdin.on("end",()=>child.stdin.end());
child.on("exit",code=>process.exit(code??1));
`, { mode: 0o755 });
  const state = { modelCalls: [], frames: [], error: null };
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/responses");
      let body = ""; for await (const bytes of request) { body += bytes; assert(body.length < 2 * 1024 * 1024); }
      state.modelCalls.push(JSON.parse(body));
      response.writeHead(200, { "content-type": "text/event-stream" });
      const id = "resp_metadata_" + state.modelCalls.length;
      const item = { id: "msg_metadata_" + state.modelCalls.length, type: "message", role: "assistant",
        status: "completed", content: [{ type: "output_text", text: "METADATA_HISTORY_PROOF", annotations: [] }] };
      const send = value => response.write("data: " + JSON.stringify(value) + "\n\n");
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } }); response.end();
    } catch (error) { state.error = error; if (!response.headersSent) response.writeHead(500); response.end(); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "config.toml"), ['model="gpt-6"', 'model_provider="cma_test"',
    'web_search="disabled"', 'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.cma_test]', 'name="Offline metadata fixture"',
    'base_url="http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api="responses"', 'requires_openai_auth=false'].join("\n"));
  const bridge = processRpc(process.execPath, [process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath],
    { cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: proxy } },
    frame => state.frames.push(frame), 15000);
  return { bridge, directory, home, state,
    configure: value => writeFileSync(control, JSON.stringify(value)),
    trace: () => readFileSync(trace, "utf8").trim().split("\n").filter(Boolean).map(JSON.parse),
    async waitFor(predicate) {
      const until = performance.now() + 10000;
      while (!predicate()) { assert(performance.now() < until, "fixture observation deadline"); await delay(5); }
    },
    async close() {
      await bridge.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
      rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    },
  };
}
module.exports = { metadataFixture };
