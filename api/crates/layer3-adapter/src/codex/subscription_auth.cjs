const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

// Apply after the independently verified empty-conversation persistence patch.
const sourceSha256 = "c3cb7e54fb6a48f3091b97bed9f5243eaf7c3f3293fbd1b021e0faf41067cd2c";
const method = `  cmaWarm = false;
  async cmaAuthenticate() {
    const native = this.codexAcpClient.codexClient;
    const retrieve = async (forceRefresh, previousAccountId) => {
      const result = await this.connection.request("cma/codex/credentials", {
        forceRefresh, previousAccountId
      });
      if (result?.mode === "warm" && !forceRefresh && !previousAccountId) return result;
      if (result?.mode !== "subscription" ||
          typeof result.tokens?.accessToken !== "string" || !result.tokens.accessToken ||
          typeof result.tokens?.chatgptAccountId !== "string" || !result.tokens.chatgptAccountId ||
          (previousAccountId && result.tokens.chatgptAccountId !== previousAccountId)) {
        throw RequestError.authRequired();
      }
      return result;
    };
    native.connection.onRequest("account/chatgptAuthTokens/refresh", async (params) => {
      const result = await retrieve(true, params.previousAccountId);
      return result.tokens;
    });
    const result = await retrieve(false, null);
    if (result.mode === "warm") {
      this.cmaWarm = true;
      return {};
    }
    // Bypass diagnostic event callbacks: credentials must never appear in an
    // agent event, stderr log, transcript or persistent runtime control receipt.
    await native.connection.sendRequest("account/login/start", {
      type: "chatgptAuthTokens", ...result.tokens
    });
    this.cmaWarm = false;
    return {};
  }
`;

function subscriptionAuth(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected Codex ACP source; review the subscription patch before upgrading");
  const replacements = [
    ["  async extMethod(method, params) {", method + "  async extMethod(method, params) {"],
    ["  async checkAuthorization() {", "  async checkAuthorization() {\n    if (this.cmaWarm) return;"],
    ["  async prompt(params, signal, onTurnStarted) {", "  async prompt(params, signal, onTurnStarted) {\n    if (this.cmaWarm) throw RequestError.authRequired();"],
    ['.onRequest("authentication/status", emptyExtensionParamsParser,', '.onRequest("cma/codex/authenticate", emptyExtensionParamsParser, () => getAgent().cmaAuthenticate()).onRequest("authentication/status", emptyExtensionParamsParser,'],
  ];
  for (const [before, after] of replacements) {
    assert.equal(source.split(before).length, 2, "Expected one reviewed subscription insertion point");
    source = source.replace(before, after);
  }
  return source;
}

module.exports = { subscriptionAuth };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(bridgePath, subscriptionAuth(readFileSync(bridgePath, "utf8")));
}
