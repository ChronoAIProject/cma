const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

const sourceSha256 = "37a48cf95b5e58b175fd3e9b89d4761d8af1ba23f256d08edfecd087407dda18";
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
