const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { dirname, join } = require("node:path");
const { createInterface } = require("node:readline");
const { createServer } = require("node:http");
const { test } = require("node:test");
const { bridgePath, persistConversations } = require("./persist_conversations.cjs");
const { subscriptionAuth } = require("./subscription_auth.cjs");
const { managedModelCatalog } = require("./managed_model_catalog.cjs");

test("native subscription handoff preserves a credential-free warm session and blocks warm prompts", { timeout: 60000 }, async () => {
  const directory = mkdtempSync(join(tmpdir(), "cma-subscription-contract-"));
  const home = join(directory, "home");
  require("node:fs").mkdirSync(home, {mode: 0o700});
  const discovery = createServer((request, response) => {
    if (!request.url.startsWith("/wham/accounts/check") && !request.url.startsWith("/api/codex/accounts/check")) {
      response.writeHead(404).end(); return;
    }
    response.writeHead(200, {"content-type": "application/json"});
    // CLI 0.165's accounts/check list carries routing; the legacy map drops it.
    response.end(JSON.stringify({accounts: [{
      id: "fixture-account", name: "Offline subscription fixture", plan_type: "plus",
      structure: "personal", workspace_backend_origin: "https://subscription.example.invalid",
      account_routing_override: "NO_CONSTRAINT",
    }], default_account_id: "fixture-account"}));
  });
  await new Promise(resolve => discovery.listen(0, "127.0.0.1", resolve));
  const executable = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  let program = bridgePath;
  if (process.env.CMA_ACP_UPSTREAM_SOURCE) {
    program = join(directory, "bridge.mjs");
    writeFileSync(program, managedModelCatalog(subscriptionAuth(persistConversations(readFileSync(process.env.CMA_ACP_UPSTREAM_SOURCE, "utf8")))));
  }
  assert.throws(() => subscriptionAuth(readFileSync(program, "utf8")), /Unexpected Codex ACP source/);
  writeFileSync(join(home, "config.toml"), 'model = "gpt-5.6-sol"\nweb_search = "disabled"\ncli_auth_credentials_store = "file"\nchatgpt_base_url = "http://127.0.0.1:' + discovery.address().port + '"\n', {mode: 0o600});
  const child = spawn(process.execPath, [program], {
    cwd: directory,
    env: {PATH: dirname(process.execPath) + ":/usr/local/bin:/usr/bin:/bin", CODEX_HOME: home, CODEX_PATH: executable},
    stdio: ["pipe", "pipe", "pipe"], detached: true,
  });
  const pending = new Map();
  let serial = 0;
  let material = {mode: "warm"};
  const diagnostics = [];
  child.stderr.on("data", bytes => diagnostics.push(bytes));
  const input = createInterface({input: child.stdout});
  input.on("line", line => {
    const frame = JSON.parse(line);
    if (frame.method === "cma/codex/credentials") {
      child.stdin.write(JSON.stringify({jsonrpc: "2.0", id: frame.id, result: material}) + "\n");
      return;
    }
    if (pending.has(frame.id)) pending.get(frame.id)(frame);
  });
  async function call(method, params, expectError = false) {
    const id = ++serial;
    let timer;
    try {
      const result = new Promise(resolve => pending.set(id, resolve));
      child.stdin.write(JSON.stringify({jsonrpc: "2.0", id, method, params}) + "\n");
      const frame = await Promise.race([result, new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("Native contract deadline: " + method)), 15000);
      })]);
      if (expectError) {assert(frame.error); return frame.error;}
      assert.equal(frame.error, undefined, "Native contract refused " + method);
      return frame.result;
    } finally {clearTimeout(timer); pending.delete(id);}
  }
  try {
    await call("initialize", {protocolVersion: 1, clientCapabilities: {}});
    await call("cma/codex/authenticate", {});
    assert.equal((await call("authentication/status", {})).type, "unauthenticated");
    const session = await call("session/new", {cwd: directory, mcpServers: []});
    assert.equal(typeof session.sessionId, "string");
    await call("session/prompt", {sessionId: session.sessionId, prompt: [{type: "text", text: "This warm prompt must never be executed."}]}, true);
    const claims = {
      exp: Math.floor(Date.now()/1000)+3600,
      email: "fixture@example.invalid",
      "https://api.openai.com/auth": {chatgpt_account_id: "fixture-account", chatgpt_plan_type: "plus"},
    };
    const accessToken = `fixture.${Buffer.from(JSON.stringify(claims)).toString("base64url")}.fixture`;
    material = {mode: "subscription", tokens: {accessToken, chatgptAccountId: "fixture-account", chatgptPlanType: "plus"}};
    await call("cma/codex/authenticate", {});
    assert.equal((await call("authentication/status", {})).type, "chat-gpt");
    await call("session/load", {sessionId: session.sessionId, cwd: directory, mcpServers: []});
    assert(!existsSync(join(home, "auth.json")), "external access must not persist a runtime auth cache");
    const logs = readdirSync(join(home, "sessions"), {recursive: true}).filter(path => /\.jsonl$/.test(path));
    assert.equal(logs.length, 1);
    const content = readFileSync(join(home, "sessions", logs[0]), "utf8");
    assert(content.includes(session.sessionId));
    assert(!content.includes("This warm prompt must never be executed."));
    assert(!content.includes(accessToken));
    assert(!Buffer.concat(diagnostics).toString().includes(accessToken), "credentials must not enter bridge diagnostics");
  } finally {
    discovery.closeAllConnections();
    await new Promise(resolve => discovery.close(resolve));
    input.close();
    if (child.exitCode === null) {
      const stopped = new Promise(resolve => child.once("exit", resolve));
      try {process.kill(-child.pid, "SIGTERM");} catch {}
      const force = setTimeout(() => {try {process.kill(-child.pid, "SIGKILL");} catch {}}, 1500);
      await stopped;
      clearTimeout(force);
    }
    rmSync(directory, {recursive: true, maxRetries: 10, retryDelay: 100});
  }
});
