const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

const sourceSha256 = "108600341d392534b2ef4a90fc6857f4fadedb5ee6c365b46484102294996c27";
const method = `  async cmaSessionWithModels(open) {
    // Discovery reads process configuration, not the returned thread. Keep it
    // request-local and behind authorization and required skill refresh.
    // Settle both branches: a failed read must not abandon a native mutation,
    // and open/persistence/history failures retain their original precedence.
    const [session, models] = await Promise.allSettled([
      open(), this.fetchAvailableModels()
    ]);
    if (session.status === "rejected") throw session.reason;
    if (models.status === "rejected") throw models.reason;
    return [session.value, models.value];
  }
`;

function replacements(source) {
  const edits = [["  async newSession(request) {", method + "  async newSession(request) {"]];
  for (const name of ["resumeSession", "loadSession", "newSession"]) {
    const start = source.indexOf(`  async ${name}(request`);
    const open = source.indexOf("    const response = await this.codexClient.thread", start);
    const models = source.indexOf("    const codexModels = await this.fetchAvailableModels();", open);
    assert(start > 0 && open > start && models > open && models - open < 1500,
      "Unexpected native session opening: " + name);
    const before = source.slice(open, models + "    const codexModels = await this.fetchAvailableModels();".length);
    const native = source.slice(open, models).trimEnd();
    const value = name === "loadSession" ? "{ response, historyResponse }" : "response";
    edits.push([before, `    const [${value}, codexModels] = await this.cmaSessionWithModels(async () => {
${native.split("\n").map(line => "  " + line).join("\n")}
      return ${value};
    });`]);
  }
  return edits;
}

function sessionOpenOverlap(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP source; review session overlap before upgrading");
  const edits = replacements(source);
  for (const [before, after] of edits) {
    assert.equal(source.split(before).length, 2, "Expected one session overlap insertion point");
    source = source.replace(before, after);
  }
  return source;
}

function withoutSessionOpenOverlap(source) {
  source = require("./prompt_output_identity.cjs").withoutPromptOutputIdentity(source);
  if (!source.includes(method)) {
    assert(!source.includes("cmaSessionWithModels"), "Unexpected session overlap implementation");
    return source;
  }
  source = source.replace(method, "");
  const pattern = /    const \[(?:response|\{ response, historyResponse \}), codexModels\] = await this\.cmaSessionWithModels\(async \(\) => \{\n([\s\S]*?)\n      return (?:response|\{ response, historyResponse \});\n    \}\);/g;
  let count = 0;
  source = source.replace(pattern, (_, native) => {
    count++;
    return native.split("\n").map(line => line.slice(2)).join("\n") +
      "\n    const codexModels = await this.fetchAvailableModels();";
  });
  assert.equal(count, 3, "Expected three native session overlap paths");
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected pre-session-overlap ACP composition");
  return source;
}

module.exports = { sessionOpenOverlap, withoutSessionOpenOverlap, sourceSha256 };
if (require.main === module) writeFileSync(bridgePath, sessionOpenOverlap(readFileSync(bridgePath, "utf8")));
