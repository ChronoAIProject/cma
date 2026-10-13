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
  async runSessionListDiagnostics() {`;

function managedModelCatalog(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected Codex ACP source; review the managed-model patch before upgrading");
  assert.equal(source.split(before).length, 2,
    "Expected exactly one reviewed model catalogue insertion point");
  return source.replace(before, after);
}

module.exports = { managedModelCatalog, sourceSha256, before, after };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(bridgePath, managedModelCatalog(readFileSync(bridgePath, "utf8")));
}
