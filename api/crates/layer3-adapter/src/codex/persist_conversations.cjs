const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");

const sourceSha256 =
  "ba74f54f7c89ba61858e544eb3afefbb0c1b90f2b23a136664828816a8430dbf";
const bridgePath =
  "/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js";
const opening = `    const response = await this.codexClient.threadStart({
      config: await this.createSessionConfig(request.cwd, additionalDirectories, request.mcpServers),
      modelProvider: this.getModelProvider(),
      cwd: request.cwd
    });`;

// Codex 0.147.0 defers an empty rollout until metadata is persisted. Keep this
// compatibility change tied to the reviewed ACP 1.4.0 bytes, never a loose match.
function persistConversations(source) {
  assert.equal(
    createHash("sha256").update(source).digest("hex"),
    sourceSha256,
    "Unexpected Codex ACP source; review the persistence patch before upgrading",
  );
  assert.equal(
    source.split(opening).length,
    2,
    "Expected exactly one new-conversation path",
  );
  return source.replace(
    opening,
    `${opening}
    await this.codexClient.sendRequest({
      method: "thread/name/set",
      params: { threadId: response.thread.id, name: "New Agent" }
    });`,
  );
}

module.exports = { bridgePath, persistConversations };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(
    bridgePath,
    persistConversations(readFileSync(bridgePath, "utf8")),
  );
}
