const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");

const sourceSha256 = "df41d2b8fc9d1247c035adcaf73d639c610b579ed84dbe5e6e98f51cc2940827";

function sessionCommands(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected codex-acp 2.12.1 source for session commands");
  const replace = (before, after) => {
    assert.equal(source.split(before).length, 2, "Unexpected session-command insertion point");
    source = source.replace(before, () => after);
  };

  replace(`    const commands = this.buildAvailableCommands(
      skillsResponse?.data ?? [],
      sessionState.clientCapabilities.airClient
    );`, `    const commands = this.buildAvailableCommands(
      skillsResponse?.data ?? [],
      sessionState.clientCapabilities.airClient
    ).filter((command) => command.name !== "logout" || sessionState.account?.type === "chatgpt");`);

  replace('      for (const skill of entry.skills) {\n        const name =',
    '      for (const skill of entry.skills) {\n        if (skill.enabled === false) continue;\n        const name =');

  // Refresh after explicit native skill config writes as well as watcher events.
  replace('        const skills = (response?.data ?? []).flatMap((entry) => entry.skills);',
    `        await this.publish(sessionState, options.shouldPublishCommands, true);
        const skills = (response?.data ?? []).flatMap((entry) => entry.skills)
          .filter((skill) => skill.enabled !== false);`);
  replace(`      const commandPromise = promptCommands.tryHandleCommand(params.prompt, sessionState, {
        signal: activePrompt.signal,`, `      const commandSessionGeneration = this.getSessionGeneration(sessionState.sessionId);
      const commandPromise = promptCommands.tryHandleCommand(params.prompt, sessionState, {
        signal: activePrompt.signal,
        shouldPublishCommands: () => !appServerLost.signal.aborted &&
          this.sessionPublishIsCurrent(sessionState, commandSessionGeneration),`);
  return source;
}

module.exports = { sessionCommands };
if (require.main === module) {
  assert.equal(process.argv.length, 4, "Expected input and output bridge paths");
  const [, , input, output] = process.argv;
  writeFileSync(output, sessionCommands(readFileSync(input, "utf8")), { flag: "wx" });
}
