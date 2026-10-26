const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");
const sourceSha256 = "b2676ef9a4a57a5bedf15c0b576b96f2c67c45e53880f08b4b8ba80f2f8ce8d9";
const method = `  async cmaAgentInstructions(params) {
    if (params.action === "support") return { supported: true, version: 1 };
    if (params.action !== "apply" || typeof params.sessionId !== "string" ||
        typeof params.systemPrompt !== "string" || Buffer.byteLength(params.systemPrompt, "utf8") > 32768 ||
        params.systemPrompt.includes("\\0")) throw RequestError.invalidParams();
    const session = this.getSessionState(params.sessionId);
    const mcpServers = session.cmaInstructionMcpServers;
    if (!Array.isArray(mcpServers))
      throw RequestError.invalidRequest("Reload the Agent session before updating instructions");
    const fs = require("node:fs"), path = require("node:path"), crypto = require("node:crypto");
    const home = process.env.CODEX_HOME;
    if (!home || !path.isAbsolute(home)) throw RequestError.invalidRequest("Agent home unavailable");
    const file = path.join(home, "AGENTS.md");
    const hash = value => crypto.createHash("sha256").update(value).digest("hex");
    const desired = hash(params.systemPrompt);
    let current;
    try { current = hash(fs.readFileSync(file)); } catch (error) { if (error.code !== "ENOENT") throw error; }
    if (current === desired && this.cmaInstructionHashes?.get(params.sessionId) === desired)
      return { applied: true, sha256: desired, sessionId: params.sessionId };
    if (this.activePrompts.has(params.sessionId) || this.pendingTurnStarts.has(params.sessionId) ||
        this.sessionIsClosing(params.sessionId))
      throw RequestError.invalidRequest("Wait for the current turn to finish before updating instructions");
    this.cmaRefreshingInstructions ??= new Set();
    this.cmaRefreshingInstructions.add(params.sessionId);
    this.beginSessionCloseFence(params.sessionId);
    try {
    const snapshot = await this.codexAcpClient.codexClient.threadRead({ threadId: params.sessionId, includeTurns: false });
    if (snapshot.thread.status?.type !== "idle")
      throw RequestError.invalidRequest("Wait for the native Agent to become idle before updating instructions");
    const temporary = path.join(home, ".cma-instructions-" + crypto.randomUUID());
    try {
      fs.writeFileSync(temporary, params.systemPrompt, { flag: "wx", mode: 0o600 });
      fs.renameSync(temporary, file);
    } finally { try { fs.unlinkSync(temporary); } catch (error) { if (error.code !== "ENOENT") throw error; } }
    // Only the settled native thread is unsubscribed. The bridge process,
    // Agent home and persisted thread identity remain exactly the same.
    await this.codexAcpClient.closeSession(params.sessionId);
    const resumed = await this.codexAcpClient.resumeSession({
      sessionId: params.sessionId, cwd: snapshot.thread.cwd ?? session.cwd,
      additionalDirectories: session.additionalDirectories, mcpServers
    });
    session.cwd = snapshot.thread.cwd ?? session.cwd;
    if (resumed.sessionId !== params.sessionId) throw new Error("Instruction update changed thread identity");
    await this.codexAcpClient.setCollaborationMode(params.sessionId, session.collaborationMode, session.currentModelId);
    this.cmaInstructionHashes ??= new Map();
    this.cmaInstructionHashes.set(params.sessionId, desired);
    return { applied: true, sha256: desired, sessionId: params.sessionId };
    } finally {
      this.cmaRefreshingInstructions.delete(params.sessionId);
      this.endSessionCloseFence(params.sessionId);
    }
  }
`;
const recording = `    this.sessions.set(sessionId, sessionState);
    // ACP's sessionMcpServers contains names for display, not resume definitions.
    sessionState.cmaInstructionMcpServers = structuredClone(requestedMcpServers);
    this.cmaInstructionHashes ??= new Map();
    try {
      const file = require("node:path").join(process.env.CODEX_HOME, "AGENTS.md");
      this.cmaInstructionHashes.set(sessionId, require("node:crypto").createHash("sha256").update(require("node:fs").readFileSync(file)).digest("hex"));
    } catch { this.cmaInstructionHashes.delete(sessionId); }`;
const stateGuard = `  getSessionState(sessionId) {
    if (this.cmaRefreshingInstructions?.has(sessionId))
      throw RequestError.invalidRequest("Agent instructions are being refreshed");`;
const route = `.onRequest("cma/agent-instructions", emptyExtensionParamsParser, (ctx) => getAgent().cmaAgentInstructions(ctx.params))`;
function agentInstructions(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256, "Unexpected ACP source; review instructions patch");
  assert.equal(source.split("  async applySessionConfigOption(sessionState, params) {").length, 2);
  assert.equal(source.split("    this.sessions.set(sessionId, sessionState);").length, 3);
  assert.equal(source.split(").connect(acpJsonStream);").length, 2);
  return source.replace("  async applySessionConfigOption(sessionState, params) {", method + "  async applySessionConfigOption(sessionState, params) {")
    .replaceAll("    this.sessions.set(sessionId, sessionState);", recording)
    .replace("  getSessionState(sessionId) {", stateGuard)
    .replace(").connect(acpJsonStream);", ")" + route + ".connect(acpJsonStream);");
}
module.exports = { agentInstructions, sourceSha256, method, recording, stateGuard, route };
if (require.main === module) writeFileSync(bridgePath, agentInstructions(readFileSync(bridgePath, "utf8")));
