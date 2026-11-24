const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");
const { createOpenCompletionRuntime } = require("./open_completion_runtime.cjs");
const sourceSha256 = "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c";
const method = `  cmaReadOpenCompletion(params) {
    if (params.action === "support") return { supported: true, version: 1, current: true };
    if (params.action === "current") {
      if (typeof params.challenge !== "string" || !/^[A-Za-z0-9_-]{16,128}$/.test(params.challenge))
        throw RequestError.invalidParams();
      return this.cmaOpenCompletion.current(params);
    }
    if (params.action !== "read" || typeof params.sessionId !== "string" || !params.sessionId ||
        params.sessionId.length > 256 || typeof params.bindingDigest !== "string" || !/^v1:[a-f0-9]{64}$/.test(params.bindingDigest) ||
        typeof params.challenge !== "string" || !/^[A-Za-z0-9_-]{16,128}$/.test(params.challenge))
      throw RequestError.invalidParams();
    return this.cmaOpenCompletion.read(params);
  }
`;
const route = `.onRequest("cma/open-completion", emptyExtensionParamsParser, (ctx) => getAgent().cmaReadOpenCompletion(ctx.params))`;
const originalAgent = "    return new CodexAcpServer(connection, codexClient, defaultAuthRequest, () => codexConnection.process.exitCode, () => stderr);";
const initializedAgent = `    const server = new CodexAcpServer(connection, codexClient, defaultAuthRequest, () => codexConnection.process.exitCode, () => stderr);
    const proof = (${createOpenCompletionRuntime.toString()})(require, server, () =>
      codexConnection.process.exitCode === null && codexConnection.process.signalCode === null && !codexConnection.process.killed,
      message => RequestError.invalidRequest(message));
    server.cmaOpenCompletion = proof;
    appServerClient.cmaOpenCompletion = proof;
    appServerClient.onClientTransportEvent(event => proof.observe(event));
    codexConnection.process.once("exit", () => proof.retireAll());
    return server;`;
const replacements = [
  [originalAgent, initializedAgent],
  ["  async cancel(params) {", method + "  async cancel(params) {"],
  [").connect(acpJsonStream);", ")" + route + ".connect(acpJsonStream);"],
  ["  beginSessionCloseFence(sessionId) {", "  beginSessionCloseFence(sessionId) {\n    this.cmaOpenCompletion?.retire(sessionId);"],
  ["    this.sessions.set(sessionId, sessionState);", "    this.cmaOpenCompletion?.installing(sessionId, sessionState);\n    this.sessions.set(sessionId, sessionState);"],
  ['  async threadUnsubscribe(params) {', '  async threadUnsubscribe(params) {\n    this.cmaOpenCompletion?.retire(params.threadId);'],
  ["  async cmaAgentInstructions(params) {", "  async cmaAgentInstructions(params) {\n    return this.cmaOpenCompletion.exclusive(() => this.cmaOriginalAgentInstructions(params), params.action === \"apply\" ? params.sessionId : undefined);\n  }\n  async cmaOriginalAgentInstructions(params) {"],
  ["  async prompt(params, signal, onTurnStarted) {", "  async prompt(params, signal, onTurnStarted) {\n    this.cmaOpenCompletion?.promptStarting();"],
];
for (const [name, kind] of [["newSession", "new"], ["loadSession", "load"], ["resumeSession", "resume"]]) {
  const anchor = `  async ${name}(params) {\n    logger.log(`;
  replacements.push([anchor, `  async ${name}(params) {\n    return this.cmaOpenCompletion.open("${kind}", params, request => this.cmaOriginal${name}(request));\n  }\n  async cmaOriginal${name}(params) {\n    logger.log(`]);
}
for (const [name, wire, kind] of [["threadStart", "thread/start", "new"], ["threadResume", "thread/resume", "resume"]]) {
  const send = `this.sendRequest({ method: "${wire}", params })`;
  replacements.push([`  async ${name}(params) {\n    return await ${send};\n  }`,
    `  async ${name}(params) {\n    return this.cmaOpenCompletion.nativeOpen("${kind}", params, () => ${send});\n  }`]);
}
for (const [name, wire] of [["threadFork", "thread/fork"], ["turnStart", "turn/start"], ["reviewStart", "review/start"]]) {
  const send = `this.sendRequest({ method: "${wire}", params })`;
  replacements.push([`  async ${name}(params) {\n    return await ${send};\n  }`,
    `  async ${name}(params) {\n    return this.cmaOpenCompletion.nativeWork(() => ${send});\n  }`]);
}
function openCompletion(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256, "Unexpected ACP source; review open completion patch");
  for (const [before, after] of replacements) {
    assert.equal(source.split(before).length, before.includes("this.sessions.set") ? 3 : 2, "Unexpected open completion insertion point: " + before);
    source = source.replaceAll(before, after);
  }
  return source;
}
function withoutOpenCompletion(source) {
  source = require("./deferred_session_metadata.cjs").withoutDeferredSessionMetadata(source);
  if (source.includes(initializedAgent)) {
    for (const [before, after] of [...replacements].reverse()) source = source.replaceAll(after, before);
    assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256, "Unexpected pre-open-completion ACP composition");
  }
  return source;
}
module.exports = { openCompletion, withoutOpenCompletion, sourceSha256 };
if (require.main === module) writeFileSync(bridgePath, openCompletion(readFileSync(bridgePath, "utf8")));
