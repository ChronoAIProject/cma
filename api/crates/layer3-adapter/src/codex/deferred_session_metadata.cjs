const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");
const { createDeferredMetadata } = require("./deferred_metadata_runtime.cjs");

const sourceSha256 = "ed2ce5c01e53b0a991bab1fd6ccbd44d0182967b3eda44ab21538881be27dc4c";
const method = `  cmaDeferLoadMetadata(state, generation) {
    this.cmaLoadMetadata ??= (${createDeferredMetadata.toString()})({
      current: (state, generation) => this.goalPublishIsCurrent(state, generation),
      commands: (state, current) => this.availableCommands.publish(state, current),
      goal: (state, generation) => this.publishCurrentGoalBestEffort(state, generation, true),
      report: outcome => logger.error("Deferred load metadata: " + outcome),
    });
    this.cmaLoadMetadata(state, generation);
  }
`;
const replacements = [
  ["  async publish(sessionState) {\n    try {",
    "  async publish(sessionState, isCurrent = () => true) {\n    try {\n      if (!isCurrent()) return;"],
  ["      const availableCommands = this.buildAvailableCommands(skillsResponse?.data ?? [])",
    "      if (!isCurrent()) return;\n      const availableCommands = this.buildAvailableCommands(skillsResponse?.data ?? [])"],
  ["    await this.availableCommands.publish(sessionState);\n    await this.publishCurrentGoalBestEffort(sessionState, requestedSessionGeneration, true);",
    "    // Advisory inventories follow the load reply; required preparation is complete."],
  ["      modeState: sessionModeState,\n      thread\n    };\n  }\n  async streamThreadHistory",
    "      modeState: sessionModeState,\n      thread,\n      cmaMetadata: { state: sessionState, generation: requestedSessionGeneration }\n    };\n  }\n  async streamThreadHistory"],
  ["      modeState,\n      thread\n    } = await this.getOrCreateSessionWithHistory(params);",
    "      modeState,\n      thread,\n      cmaMetadata\n    } = await this.getOrCreateSessionWithHistory(params);"],
  ["    await this.streamThreadHistory(sessionId, thread);\n    logger.log(\"Session loaded\",",
    "    await this.streamThreadHistory(sessionId, thread);\n    this.cmaDeferLoadMetadata(cmaMetadata.state, cmaMetadata.generation);\n    logger.log(\"Session loaded\","],
  ["  publishAvailableCommandsAsync(sessionState) {", method + "  publishAvailableCommandsAsync(sessionState) {"],
];

function deferredSessionMetadata(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP source; review deferred session metadata before upgrading");
  for (const [before, after] of replacements) {
    assert.equal(source.split(before).length, 2,
      "Unexpected deferred metadata insertion point: " + before.slice(0, 90));
    source = source.replace(before, after);
  }
  return source;
}

function withoutDeferredSessionMetadata(source) {
  source = require("./session_open_overlap.cjs").withoutSessionOpenOverlap(source);
  if (source.includes(method)) {
    for (const [before, after] of [...replacements].reverse()) source = source.replace(after, before);
    assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
      "Unexpected pre-deferred-metadata ACP composition");
  }
  return source;
}

module.exports = { deferredSessionMetadata, withoutDeferredSessionMetadata, sourceSha256 };
if (require.main === module) writeFileSync(bridgePath, deferredSessionMetadata(readFileSync(bridgePath, "utf8")));
