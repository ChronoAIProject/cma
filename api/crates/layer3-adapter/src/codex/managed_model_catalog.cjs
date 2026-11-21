const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

// Apply after persistence and subscription authentication. This changes ACP's
// selectable model metadata only, not Codex's native model catalogue, fallback
// instructions, tool configuration, or the provider's inference request.
const sourceSha256 = "d43c6cbda951f3f8f08a1ef1063803e703bf20c4167b2b2cd876c6f5a5a4cb0a";
const before = `    return models;
  }
  async runSessionListDiagnostics() {`;
const after = `    // CMA's managed gpt-6 name is a configured provider alias. Codex can
    // execute it using its native unknown-model fallback, but ACP's legacy
    // set_model method only accepts entries returned by this model list.
    // Keep native metadata untouched and add only the explicitly configured
    // CMA alias. Reading config on every fetch also covers session reload and
    // switching from an advertised built-in model back to the configured alias.
    if (!this.gatewayConfig) {
      const read = await this.codexClient.configRead({ includeLayers: false });
      const configured = { ...read?.config, ...this.config };
      const provider = this.modelProvider ?? configured.model_provider;
      if (provider === "cma" && configured.model === "gpt-6" &&
          !models.some((model) => model.id === "gpt-6")) {
        models.push({
          id: "gpt-6", model: "gpt-6", displayName: "gpt-6",
          description: "Configured CMA managed model",
          isDefault: false, defaultReasoningEffort: "high",
          supportedReasoningEfforts: ["low", "medium", "high", "xhigh"].map(
            (reasoningEffort) => ({ reasoningEffort, description: reasoningEffort })
          ),
          // These are the existing ACP unknown-model fallback modalities. Do
          // not inherit a built-in model's context window, tools or fast mode.
          inputModalities: ["text", "image"], additionalSpeedTiers: []
        });
      }
    }
    return models;
  }
  async fetchManagedModel(requested) {
    // Resolve a new managed alias only when it is selected. Default startup and
    // native model changes do not depend on another network catalogue request.
    if (this.gatewayConfig || typeof requested !== "string" || requested.length > 256 ||
        !(/^(?:gpt-[0-9][A-Za-z0-9._-]*|codex-auto-review)$/.test(requested))) return null;
    const read = await this.codexClient.configRead({ includeLayers: false });
    const configured = { ...read?.config, ...this.config };
    if ((this.modelProvider ?? configured.model_provider) !== "cma") return null;
    try {
      const base = new URL(configured.model_providers?.cma?.base_url);
      if (base.protocol !== "http:" || base.hostname !== "127.0.0.1" ||
          base.username || base.password || base.search || base.hash) return null;
      base.pathname = base.pathname.replace(/\\/$/, "") + "/models";
      const response = await fetch(base, {
        headers: { "User-Agent": "chrono-bot/1.0", Accept: "application/json" },
        redirect: "error", signal: AbortSignal.timeout(5000)
      });
      if (response.status !== 200) { await response.body?.cancel(); return null; }
      const chunks = [];
      let size = 0;
      for await (const bytes of response.body) {
        size += bytes.length;
        if (size > 1024 * 1024) throw new Error("Managed model catalogue is too large");
        chunks.push(bytes);
      }
      const catalogue = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      const entries = Array.isArray(catalogue) ? catalogue : catalogue.data ?? catalogue.models;
      if (!Array.isArray(entries) || entries.length > 1000 ||
          !entries.some((entry) => entry?.id === requested)) return null;
      return {
        id: requested, model: requested, displayName: requested,
        description: "Chrono LLM managed model",
        isDefault: false, defaultReasoningEffort: "high",
        supportedReasoningEfforts: ["low", "medium", "high", "xhigh"].map(
          (reasoningEffort) => ({ reasoningEffort, description: reasoningEffort })
        ),
        inputModalities: ["text", "image"], additionalSpeedTiers: []
      };
    } catch {
      // Never include upstream response bodies, addresses or credential material
      // in ACP diagnostics. The caller retains its existing Unknown model error.
      return null;
    }
  }
  async runSessionListDiagnostics() {`;
const beforeSelection = `    const models = await this.codexAcpClient.fetchAvailableModels();
    const model = models.find((m) => m.id === requestedModelName);
    if (!model) throw new Error(\`Unknown model \${params.modelId}\`);`;
const afterSelection = `    const models = await this.codexAcpClient.fetchAvailableModels();
    let model = models.find((m) => m.id === requestedModelName);
    if (!model) {
      model = await this.codexAcpClient.fetchManagedModel(requestedModelName);
      if (model) models.push(model);
    }
    if (!model) throw new Error(\`Unknown model \${params.modelId}\`);`;

function managedModelCatalog(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected Codex ACP source; review the managed-model patch before upgrading");
  assert.equal(source.split(before).length, 2,
    "Expected exactly one reviewed model catalogue insertion point");
  assert.equal(source.split(beforeSelection).length, 2,
    "Expected exactly one reviewed model selection insertion point");
  return source.replace(before, after).replace(beforeSelection, afterSelection);
}

module.exports = { managedModelCatalog, sourceSha256, before, after, beforeSelection, afterSelection };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(bridgePath, managedModelCatalog(readFileSync(bridgePath, "utf8")));
}
