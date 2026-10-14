const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { runInNewContext } = require("node:vm");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
const { managedModelCatalog, sourceSha256, before, after, beforeSelection, afterSelection } = require("./managed_model_catalog.cjs");

function sources() {
  const installed = readFileSync(process.env.CMA_ACP_MANAGED_BASE_SOURCE ?? bridgePath, "utf8");
  const base = installed.includes(after) ? installed.replace(after, before).replace(afterSelection, beforeSelection) : installed;
  assert.equal(createHash("sha256").update(base).digest("hex"), sourceSha256);
  return { base, patched: managedModelCatalog(base) };
}

test("managed model patch is tied to the reviewed package bytes", () => {
  const { patched } = sources();
  assert.throws(() => managedModelCatalog("unreviewed"), /Unexpected Codex ACP source/);
  assert.throws(() => managedModelCatalog(patched), /Unexpected Codex ACP source/);
});

test("native subscription, gateway overrides and other configured models keep their original catalogue", async () => {
  const { patched } = sources();
  const start = patched.indexOf("  async fetchAvailableModels() {");
  const end = patched.indexOf("  async runSessionListDiagnostics() {", start);
  assert(start > 0 && end > start);
  const Client = runInNewContext("(class {" + patched.slice(start, end) + "})");
  for (const scenario of [
    { provider: "openai", model: "gpt-6" },
    { provider: "cma", model: "gpt-6", override: "openai" },
    { provider: "cma", model: "gpt-6", gateway: {} },
    { provider: "cma", model: "gpt-5.6-sol" },
  ]) {
    const original = { id: "gpt-5.6-sol", supportedReasoningEfforts: [{ reasoningEffort: "high" }] };
    let reads = 0;
    const client = new Client();
    Object.assign(client, { gatewayConfig: scenario.gateway ?? null,
      modelProvider: scenario.override ?? null, config: {}, codexClient: {
        async listModels() { return { data: [original], nextCursor: null }; },
        async configRead() { reads += 1; return { config: {
          model_provider: scenario.provider, model: scenario.model,
        } }; },
      } });
    const result = await client.fetchAvailableModels();
    assert.equal(result.length, 1);
    assert.equal(result[0], original, "provider model metadata must remain untouched");
    assert.equal(reads, scenario.gateway ? 0 : 1);
  }
});

test("managed aliases require bounded current catalogue evidence from the loopback provider", async () => {
  const { patched } = sources();
  const start = patched.indexOf("  async fetchAvailableModels() {");
  const end = patched.indexOf("  async runSessionListDiagnostics() {", start);
  let requests = 0;
  let payload = JSON.stringify({ data: [{ id: "gpt-6-astra" }] });
  let status = 200;
  const Client = runInNewContext("(class {" + patched.slice(start, end) + "})", {
    URL, AbortSignal, Buffer,
    fetch: async (url, options) => {
      requests += 1;
      assert.equal(url.href, "http://127.0.0.1:9091/models");
      assert.equal(options.redirect, "error");
      return new Response(payload, { status });
    },
  });
  const configured = { model_provider: "cma", model_providers: { cma: { base_url: "http://127.0.0.1:9091" } } };
  const client = new Client();
  Object.assign(client, { config: {}, codexClient: {
    async configRead() { return { config: configured }; },
  } });
  assert.equal((await client.fetchManagedModel("gpt-6-astra")).id, "gpt-6-astra");
  for (const name of ["gpt-image-2", "bad\nmodel", "gpt-6/redirect"]) {
    assert.equal(await client.fetchManagedModel(name), null);
  }
  assert.equal(requests, 1);
  for (const url of ["https://example.invalid", "http://metadata.google.internal", "http://127.0.0.1:9091?override=1"]) {
    configured.model_providers.cma.base_url = url;
    assert.equal(await client.fetchManagedModel("gpt-6-astra"), null);
  }
  assert.equal(requests, 1);
  configured.model_providers.cma.base_url = "http://127.0.0.1:9091";
  for (const body of ["invalid", " ".repeat(1024 * 1024 + 1), JSON.stringify({ data: Array(1001).fill({ id: "gpt-6-astra" }) })]) {
    payload = body;
    assert.equal(await client.fetchManagedModel("gpt-6-astra"), null);
  }
  payload = JSON.stringify({ data: [{ id: "gpt-6-astra" }] });
  status = 403;
  assert.equal(await client.fetchManagedModel("gpt-6-astra"), null);
});

async function modelFixture() {
  const calls = [];
  const catalogue = { status: 200, calls: 0 };
  let error;
  const server = createServer(async (request, response) => {
    try {
      if (request.method === "GET" && request.url === "/v1/models") {
        catalogue.calls += 1;
        assert.equal(request.headers["user-agent"], "chrono-bot/1.0");
        assert.equal(request.headers.accept, "application/json");
        assert.equal(request.headers.authorization, undefined);
        response.writeHead(catalogue.status, { "content-type": "application/json" });
        response.end(JSON.stringify({ data: ["gpt-6-astra", "gpt-6.1-sol", "gpt-image-2"].map((id) => ({ id })) }));
        return;
      }
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
  return { calls, catalogue, origin: "http://127.0.0.1:" + server.address().port,
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

function choice(response, id) {
  return response.configOptions.find((option) => option.id === id);
}

test("pinned ACP preserves native fallback and supports managed settings across model changes and reload", { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.equal(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), "codex-cli 0.147.0");
  const directory = mkdtempSync(join(tmpdir(), "cma-managed-model-test-"));
  const programs = sources();
  const paths = Object.fromEntries(Object.entries(programs).map(([name, source]) => {
    const path = join(directory, name + ".mjs");
    writeFileSync(path, source);
    return [name, path];
  }));
  const fixture = await modelFixture();
  const homes = Object.fromEntries(["base", "patched", "other"].map((name) => [name, join(directory, name)]));
  for (const [name, path] of Object.entries(homes)) config(path, name === "other" ? "cma_test" : "cma", fixture.origin);
  let bridge;
  const connect = async (name, program = name) => {
    bridge = processRpc(process.execPath, [paths[program]], { cwd: directory,
      env: { PATH: process.env.PATH, CODEX_HOME: homes[name], CODEX_PATH: binary } });
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
  };
  const prompt = async (sessionId) => {
    await bridge.call("session/prompt", { sessionId,
      prompt: [{ type: "text", text: "Reply briefly without using tools." }] });
    fixture.check();
    return fixture.calls.at(-1);
  };
  const apply = async (sessionId, model, effort, mode) => {
    await bridge.call("session/set_model", { sessionId, modelId: model + "[" + effort + "]" });
    const configured = await bridge.call("session/set_config_option", {
      sessionId, configId: "reasoning_effort", value: effort,
    });
    assert.equal(choice(configured, "model").currentValue, model);
    assert.equal(choice(configured, "reasoning_effort").currentValue, effort);
    await bridge.call("session/set_mode", { sessionId, modeId: mode });
  };
  try {
    await connect("base");
    const original = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    assert(!original.models.availableModels.some((model) => model.modelId.startsWith("gpt-6[")));
    await assert.rejects(bridge.call("session/set_model", { sessionId: original.sessionId, modelId: "gpt-6[medium]" }));
    const native = await prompt(original.sessionId);
    assert.equal(native.model, "gpt-6");
    assert.equal(native.reasoning.effort, "high");
    await bridge.close();

    await connect("patched");
    const session = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    assert.deepEqual(session.models.availableModels.filter((model) => !model.modelId.startsWith("gpt-6[")), original.models.availableModels);
    assert.deepEqual(session.models.availableModels.filter((model) => model.modelId.startsWith("gpt-6[")).map((model) => model.modelId),
      ["gpt-6[low]", "gpt-6[medium]", "gpt-6[high]", "gpt-6[xhigh]"]);
    assert.equal(fixture.catalogue.calls, 0, "default startup must not wait on catalogue HTTP");
    await apply(session.sessionId, "gpt-6", "medium", "read-only");
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "unconfigured-model[medium]" }));
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-6[minimal]" }));
    const managed = await prompt(session.sessionId);
    assert.equal(managed.model, "gpt-6");
    assert.equal(managed.reasoning.effort, "medium");
    assert(managed.instructions === native.instructions, "native fallback instructions must stay unchanged");
    assert.deepEqual(managed.tools.map((tool) => tool.name ?? tool.type),
      native.tools.map((tool) => tool.name ?? tool.type), "native fallback tools must stay unchanged");

    for (const model of ["gpt-6-astra", "gpt-6.1-sol"]) {
      await apply(session.sessionId, model, "high", "agent-full-access");
      const selected = await prompt(session.sessionId);
      assert.equal(selected.model, model);
      assert.equal(selected.reasoning.effort, "high");
      assert.equal(selected.instructions, native.instructions);
      assert.deepEqual(selected.tools.map((tool) => tool.name ?? tool.type),
        native.tools.map((tool) => tool.name ?? tool.type));
    }
    assert.equal(fixture.catalogue.calls, 2);
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-image-2[high]" }));
    assert.equal(fixture.catalogue.calls, 2, "image models are not Agent text models");
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-9-absent[high]" }));
    fixture.catalogue.status = 403;
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-6-astra[high]" }));
    fixture.catalogue.status = 200;
    assert.equal((await prompt(session.sessionId)).model, "gpt-6.1-sol", "refused settings keep the previous model");

    const alternate = original.models.availableModels.find((model) => model.modelId === "gpt-5.6-sol[high]");
    assert(alternate, "test alternative must be advertised by the actual pinned runtime");
    await apply(session.sessionId, "gpt-5.6-sol", "high", "agent-full-access");
    assert.equal((await prompt(session.sessionId)).model, "gpt-5.6-sol");
    await bridge.close();
    await connect("patched");
    await bridge.call("session/load", { sessionId: session.sessionId, cwd: directory, mcpServers: [] });
    await apply(session.sessionId, "gpt-6-astra", "high", "agent-full-access");
    assert.equal((await prompt(session.sessionId)).model, "gpt-6-astra");
    await apply(session.sessionId, "gpt-6", "high", "agent-full-access");
    const restored = await prompt(session.sessionId);
    assert.equal(restored.model, "gpt-6");
    assert.equal(restored.reasoning.effort, "high");
    assert(restored.instructions === native.instructions, "restored native fallback instructions must stay unchanged");
    await bridge.close();

    await connect("other", "patched");
    const other = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    assert(!other.models.availableModels.some((model) => model.modelId.startsWith("gpt-6[")));
    await assert.rejects(bridge.call("session/set_model", { sessionId: other.sessionId, modelId: "gpt-6[medium]" }));
  } finally {
    await bridge?.close();
    await fixture.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
