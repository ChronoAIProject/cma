const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

const sourceSha256 = "99076a9022c0e09108ac1cb95186c8ddf0b64b7fb30b37158c9483c5c3e9a4d0";
const before = `        case "http":
          return {
            "url": mcpServer.url,`;
const after = `        case "http":
          return {
            // CMA's permission request capability must be usable before the
            // first turn. Codex otherwise omits optional servers after 1s.
            ...(mcpServer.name === "cma_access" ? { "required": true } : {}),
            "url": mcpServer.url,`;

function requiredAccessTool(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP source; review required access tools before upgrading");
  assert.equal(source.split(before).length, 2, "Expected one HTTP MCP configuration path");
  return source.replace(before, after);
}

module.exports = { requiredAccessTool, sourceSha256, before, after };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(bridgePath, requiredAccessTool(readFileSync(bridgePath, "utf8")));
}
