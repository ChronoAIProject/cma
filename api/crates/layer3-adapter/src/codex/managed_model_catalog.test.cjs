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

function toolNames(request) {
  // Newer CLI requests carry native declarations in additional_tools items.
  const tools = request.tools ?? request.input.filter((item) => item.type === "additional_tools").flatMap((item) => item.tools);
  assert(tools.length > 0, "native tool inventory must not disappear");
  return tools.map((tool) => tool.name ?? tool.type);
}

function sources() {
  const installed = readFileSync(process.env.CMA_ACP_MANAGED_BASE_SOURCE ?? bridgePath, "utf8");
  if (installed.includes(after)) {
    assert.equal(installed.split(after).length, 2, "Expected exactly one managed catalogue patch");
    assert.equal(installed.split(afterSelection).length, 2, "Expected exactly one managed selection patch");
  }
  const base = installed.includes(after) ? installed.replace(after, before).replace(afterSelection, beforeSelection) : installed;
  const digest = (source) => createHash("sha256").update(source).digest("hex");
  const finalCompositions = {
    "11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4": "cfd3e1f0c011801525452a6897073921cf6d9c96c4d1d74f10e4865613db19fc",
    "ddb625f66a5a14d94df3512a525c4a674a359e564602c09085769c8b61e2bca1": "c2d9dd0441dc8f8f04cb7196d930df03c19cd1daf912fff1edb62fe48a68d9ca",
    e26bce27dbdcf4d665e081b4a81762542795e2af6453da2a2f050fab5110f484: "237032456955b9d9c2a3c583556f79ba8b8c986cf55e1a9c70131925881cfb1c",
    "8f0c023269496f9ad21aa2958491772fefe65011b9aebaf674c67d2aaa9fbcf4": "228381e866789e298b15a41c2506ce5db0824d7663da831eadf3fcd9c43ecdd4",
    "fc9f74358c6ce5aeda4ac164c1daf83c513fb0f18df4b75f3d20bd0816fc6fa7": "a5d96df5a0642b422311c603fee978274dab31f7751c16d81a5758c058f4d648",
    "ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66": "d24ec76d5ff372f7ab972c15e45ea0d227ede1d8aff4e3888406ef399cfdffc5",
    "186537078bb1a13292c927a7da1290f345fd76c7918025fb603a5c1eb89c8203": "600a57f3e6f8c17a4a4a76715fc16acf76272238e54c77276d295fe5dbe6259e",
    "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c": "e487e915dd3a00c9816871b346b7911b39173225ecbda098c9ad2f7e88da568b",
    "4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888": "e4bb8df279fc7b63dde04057646686e411b46cccef753594b7e272096b54b83f",
    "113762e08aaa46cbea8ec3710e334024d13f1cc026a1c264130d5e642e0748c4": "1beae76099ee89f15793c9ea2638934bc6cf10347220be362b29e624e27499ed",
    bcbfa2156e69b630fb3777b275ff739efd6c030159348e9cbae79a56ffa663c5: "0f751c0500accfbd0bd5e8c11aac6f2d9e2ade3139b8c9fe355116e590879cf6",
    b2676ef9a4a57a5bedf15c0b576b96f2c67c45e53880f08b4b8ba80f2f8ce8d9: "c4158cf17031d559adfc261104775ae04d6c688942fc3b23f6bdc5f1b267ada0",
  };
  if (Object.hasOwn(finalCompositions, digest(installed))) {
    // The image includes the reviewed observation and access patches. Compare
    // that exact installed composition with only the model patch removed, so
    // the native regression also covers the final image's other behavior.
    // Title provenance changed the composition, not either model-patch site.
    assert.equal(digest(base), finalCompositions[digest(installed)]);
    assert.equal(base.replace(before, after).replace(beforeSelection, afterSelection), installed);
    return { base, patched: installed };
  }
  assert.equal(digest(base), sourceSha256);
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
  let payload = JSON.stringify({ data: [{ id: "gpt-6.2-sol" }, { id: "gpt-6.2-astra" }] });
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
  for (const model of ["gpt-6.2-sol", "gpt-6.2-astra"]) {
    assert.equal((await client.fetchManagedModel(model)).id, model);
  }
  for (const name of ["gpt-image-2", "bad\nmodel", "gpt-6/redirect", "gpt-" + "6".repeat(253)]) {
    assert.equal(await client.fetchManagedModel(name), null);
  }
  assert.equal(requests, 2);
  for (const url of ["https://example.invalid", "http://metadata.google.internal", "http://127.0.0.1:9091?override=1"]) {
    configured.model_providers.cma.base_url = url;
    assert.equal(await client.fetchManagedModel("gpt-6.2-astra"), null);
  }
  assert.equal(requests, 2);
  configured.model_providers.cma.base_url = "http://127.0.0.1:9091";
  for (const body of ["invalid", " ".repeat(1024 * 1024 + 1), JSON.stringify({ data: Array(1001).fill({ id: "gpt-6.2-astra" }) })]) {
    payload = body;
    assert.equal(await client.fetchManagedModel("gpt-6.2-astra"), null);
  }
  payload = JSON.stringify({ data: [{ id: "gpt-6.2-astra" }] });
  status = 403;
  assert.equal(await client.fetchManagedModel("gpt-6.2-astra"), null);
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
        response.end(JSON.stringify({ data: ["gpt-6.2-sol", "gpt-6.2-astra", "gpt-image-2"].map((id) => ({ id })) }));
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
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
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
    assert.deepEqual(toolNames(managed), toolNames(native), "native fallback tools must stay unchanged");

    for (const model of ["gpt-6.2-sol", "gpt-6.2-astra"]) {
      await apply(session.sessionId, model, "high", "agent-full-access");
      const selected = await prompt(session.sessionId);
      assert.equal(selected.model, model);
      assert.equal(selected.reasoning.effort, "high");
      assert.equal(selected.instructions, native.instructions);
      // CLI tool packaging can change after a model switch. Native fallback
      // identity is proved above and again after restoration; each selected
      // model must retain an executable shell/tool namespace.
      assert(toolNames(selected).some(name => name === "functions" || name === "exec_command"));
    }
    assert.equal(fixture.catalogue.calls, 2);
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-image-2[high]" }));
    assert.equal(fixture.catalogue.calls, 2, "image models are not Agent text models");
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-9-absent[high]" }));
    fixture.catalogue.status = 403;
    await assert.rejects(bridge.call("session/set_model", { sessionId: session.sessionId, modelId: "gpt-6.2-sol[high]" }));
    fixture.catalogue.status = 200;
    assert.equal((await prompt(session.sessionId)).model, "gpt-6.2-astra", "refused settings keep the previous model");

    const alternate = original.models.availableModels.find((model) => model.modelId === "gpt-5.6-sol[high]");
    assert(alternate, "test alternative must be advertised by the actual pinned runtime");
    await apply(session.sessionId, "gpt-5.6-sol", "high", "agent-full-access");
    assert.equal((await prompt(session.sessionId)).model, "gpt-5.6-sol");
    await bridge.close();
    await connect("patched");
    await bridge.call("session/load", { sessionId: session.sessionId, cwd: directory, mcpServers: [] });
    await apply(session.sessionId, "gpt-6.2-sol", "high", "agent-full-access");
    assert.equal((await prompt(session.sessionId)).model, "gpt-6.2-sol");
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
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
