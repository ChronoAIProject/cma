const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

const sourceSha256 = "bcbfa2156e69b630fb3777b275ff739efd6c030159348e9cbae79a56ffa663c5";
function sessionCommands(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP source; review session commands before upgrading");
  const replace = (before, after) => {
    assert.equal(source.split(before).length, 2, "Unexpected session-command insertion point");
    source = source.replace(before, after);
  };
  replace('      const availableCommands = this.buildAvailableCommands(skillsResponse?.data ?? []);',
    `      const availableCommands = this.buildAvailableCommands(skillsResponse?.data ?? [])
        .filter((command) => command.name !== "logout" || sessionState.account?.type === "chatgpt");`);
  replace('      for (const skill of entry.skills) {',
    `      for (const skill of entry.skills) {
        if (skill.enabled === false) continue;`);
  // Native config writes clear the cache but do not emit skills/changed in
  // 0.147.0. The native /skills command is also an explicit inventory refresh.
  replace('        const skills = (response?.data ?? []).flatMap((entry) => entry.skills);',
    `        await this.publish(sessionState);
        const skills = (response?.data ?? []).flatMap((entry) => entry.skills).filter((skill) => skill.enabled !== false);`);
  replace('      () => this.refreshSessionsAuthState(null)\n    );',
    `      () => this.refreshSessionsAuthState(null)
    );
    // Codex invalidates its skill cache before this native notification. Publish
    // the updated inventory for each actual session without starting a turn.
    this.codexAcpClient.codexClient.onClientTransportEvent((event) => {
      if (event.eventType === "notification" && event.method === "skills/changed") {
        for (const session of this.sessions.values()) this.publishAvailableCommandsAsync(session);
      }
    });`);
  return source;
}

module.exports = { sessionCommands, sourceSha256 };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(bridgePath, sessionCommands(readFileSync(bridgePath, "utf8")));
}
