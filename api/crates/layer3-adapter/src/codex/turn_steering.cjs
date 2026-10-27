const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");
const sourceSha256 = "4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888";
const method = `  async cmaSteer(params) {
    if (params.action === "support") return { supported: true, version: 1 };
    const input = this.parseSessionSteerParams(params);
    const session = this.getSessionState(input.sessionId);
    this.assertSteerInputSupported(input, session);
    const turnId = await this.getSteerableTurnId(session);
    if (!turnId || this.sessionIsClosing(input.sessionId))
      throw RequestError.invalidRequest("The target turn is no longer active");
    // Injection is fenced by the native turn id. Never fall back to turn/start:
    // another prompt replaces ACP's cancellation and completion ownership.
    await this.runWithProcessCheck(() => this.codexAcpClient.steerTurn({
      threadId: input.sessionId, turnId, prompt: input.prompt
    }));
    return { injected: true, sessionId: input.sessionId };
  }
`;
const route = `.onRequest("cma/steer", emptyExtensionParamsParser, (ctx) => getAgent().cmaSteer(ctx.params))`;
function turnSteering(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256, "Unexpected ACP source; review steering patch");
  assert.equal(source.split("  async cancel(params) {").length, 2);
  assert.equal(source.split(").connect(acpJsonStream);").length, 2);
  return source.replace("  async cancel(params) {", method + "  async cancel(params) {")
    .replace(").connect(acpJsonStream);", ")" + route + ".connect(acpJsonStream);");
}
module.exports = { turnSteering, sourceSha256 };
if (require.main === module) writeFileSync(bridgePath, turnSteering(readFileSync(bridgePath, "utf8")));
