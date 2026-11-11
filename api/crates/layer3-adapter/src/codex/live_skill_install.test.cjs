const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, renameSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { controlledBinary } = require("./native_control_fixture.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
async function modelFixture() {
  const calls = [];
  let error;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.method, "POST");
      assert.equal(request.url, "/v1/responses");
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        assert(body.length < 2 * 1024 * 1024);
      }
      calls.push(JSON.parse(body));
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
      const id = "resp_settings_" + calls.length;
      const item = { id: "msg_settings_" + calls.length, type: "message", role: "assistant",
        status: "completed", content: [{ type: "output_text", text: "Settings fixture.", annotations: [] }] };
      const send = (event) => response.write("data: " + JSON.stringify(event) + "\n\n");
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (failure) {
      error = failure;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { calls, origin: "http://127.0.0.1:" + server.address().port,
    check() { if (error) throw error; },
    async close() { server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); } };
}

function config(directory, provider, origin) {
  mkdirSync(directory);
  writeFileSync(join(directory, "config.toml"), [
    'model = "gpt-6"', 'model_reasoning_effort = "high"',
    'model_provider = "' + provider + '"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.' + provider + ']', 'name = "Offline settings fixture"',
    'base_url = "' + origin + '/v1"', 'wire_api = "responses"', 'requires_openai_auth = false',
  ].join("\n"));
}


test("pinned running Codex discovers an added skill and refreshes its latest catalog after replacement", { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-live-skill-test-"));
  const home = join(directory, "agent-home");
  const fixture = await modelFixture();
  config(home, "cma_test", fixture.origin);
  mkdirSync(join(home, "skills"));
  const bridge = processRpc(process.execPath, [process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath], {
    cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: controlledBinary(directory, binary) },
  });
  const name = "cma-live-added-skill";
  const description = "Unique live-install discovery marker 08b72e31";
  try {
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    const prompt = async () => {
      await bridge.call("session/prompt", { sessionId: session.sessionId,
        prompt: [{ type: "text", text: "Reply briefly without using tools." }] });
      fixture.check();
      return JSON.stringify(fixture.calls.at(-1));
    };
    const before = await prompt();
    const nativePid = readFileSync(join(directory, "native-control.pid"), "utf8");
    assert(!before.includes(description));
    const staging = join(home, "skills", ".stage-live-install");
    mkdirSync(staging);
    writeFileSync(join(staging, "SKILL.md"), "---\nname: " + name + "\ndescription: " + description + "\n---\nUse this skill when specifically requested.\n");
    renameSync(staging, join(home, "skills", "research-guid"));
    const after = await prompt();
    assert(after.includes(description), "same running Codex process and conversation must advertise the newly added skill to the next model turn");
    assert(after.includes(name));
    assert.equal(fixture.calls.length, 2);
    const upgradedDescription = "Unique live-upgrade discovery marker c63e1f4a";
    mkdirSync(staging);
    writeFileSync(join(staging, "SKILL.md"), "---\nname: " + name + "\ndescription: " + upgradedDescription + "\n---\nUse the upgraded skill when specifically requested.\n");
    // Exercise the atomic directory exchange required for a runtime upgrade.
    execFileSync("python3", ["-c", [
      "import ctypes, os, sys",
      "lib = ctypes.CDLL(None, use_errno=True)",
      "a, b = (os.fsencode(p) for p in sys.argv[1:])",
      // The Dockerfile runs this test on Debian/glibc, which exports renameat2.
      "result = lib.renamex_np(a, b, 2) if sys.platform == 'darwin' else lib.renameat2(-100, a, -100, b, 2)",
      "assert result == 0, os.strerror(ctypes.get_errno())",
    ].join("\n"), staging, join(home, "skills", "research-guid")]);
    rmSync(staging, { recursive: true });
    const upgraded = await prompt();
    assert(upgraded.includes(upgradedDescription), "next turn must advertise the replacement in the same conversation");
    assert(upgraded.includes(name));
    assert.equal(fixture.calls.length, 3);
    const catalogs = fixture.calls.at(-1).input.filter(item =>
      item.role === "developer" && JSON.stringify(item).includes("<skills_instructions>"));
    const latestCatalog = JSON.stringify(catalogs.at(-1));
    assert(latestCatalog.includes(upgradedDescription));
    assert(!latestCatalog.includes(description));
    const replacementIndex = fixture.calls.at(-1).input.findLastIndex(item =>
      item.role === "developer" && JSON.stringify(item).includes("<skills_instructions>"));
    for (const [index, item] of fixture.calls.at(-1).input.entries()) {
      if (JSON.stringify(item).includes(description)) assert(index < replacementIndex);
    }
    assert.equal(readFileSync(join(home, "skills", "research-guid", "SKILL.md"), "utf8"),
      "---\nname: " + name + "\ndescription: " + upgradedDescription + "\n---\nUse the upgraded skill when specifically requested.\n");
    // The same bridge and session handle served all three turns; no reopen.
    process.kill(bridge.pid, 0);
    assert.equal(readFileSync(join(directory, "native-control.pid"), "utf8"), nativePid);
    process.kill(Number(nativePid), 0);

  } finally {
    await bridge.close(); await fixture.close(); rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
