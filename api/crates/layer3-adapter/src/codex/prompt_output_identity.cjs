const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { withoutNativeBoundDispatch } = require("./native_bound_dispatch.cjs");
const { bridgePath } = require("./persist_conversations.cjs");
const { createPromptOutputBuffer } = require("./prompt_output_buffer.cjs");

const sourceSha256 = "e26bce27dbdcf4d665e081b4a81762542795e2af6453da2a2f050fab5110f484";

const state = `    const activePrompt = this.trackActivePrompt(params.sessionId);`;
const guardedState = state + `
    const output = (${createPromptOutputBuffer.toString()})(
      operation => this.codexAcpClient.enqueueSessionNotification(params.sessionId, operation),
      extractTurnRouting,
      () => this.activePrompts.get(params.sessionId) === activePrompt,
      () => logger.error("Native prompt output exceeded its pending-start buffer")
    );
    activePrompt.signal.addEventListener("abort", output.cancelPending, { once: true });`;
const callback = `        async (event) => {
          if (!promptNotificationsActive) {`;
const callbackEnd = `        },
        approvalHandler,
        elicitationHandler
      );`;
const replaceTurn = `          activePrompt.currentTurn = null;
          sessionState.currentTurnId = null;`;
const start = `          pendingTurnStart?.resolve(turnId);
          onTurnStarted?.();`;
const modelStart = `            pendingTurnStart?.resolve(turnId);
            onTurnStarted?.();`;
const planStart = `                recoverableSessionFailure = sessionState.sessionFailure;
                promptNotificationsActive = true;`;
const finish = `    } finally {
      promptNotificationsActive = false;`;
const commandSuccess = `        if (commandResult.turnCompleted?.turn.status === "completed") {`;
const modelSuccess = `      await clearRecoveredSessionFailure(eventHandler);
      await this.publishFallbackSessionTitle(`;
const overflow = `if (output.hasLostOutput()) throw RequestError.internalError(undefined, "Native prompt output buffer overflow");`;
const replacements = [
  [state, guardedState],
  [callback, callback.replace("async (event) => {", "output.route(async (event) => {")],
  [callbackEnd, callbackEnd.replace("},", "}),")],
  [start, "          output.bind(turn);\n" + start],
  [modelStart, "            output.bind(turn);\n" + modelStart],
  [replaceTurn, "          output.nextTurn();\n" + replaceTurn],
  [planStart, planStart + "\n                output.bind(turn);"],
  [commandSuccess, commandSuccess + "\n          " + overflow],
  [modelSuccess, "      " + overflow + "\n" + modelSuccess],
  [finish, finish + '\n      output.close();\n      activePrompt.signal.removeEventListener("abort", output.cancelPending);'],
];

function applyEdits(source, edits) {
  for (const [before, after] of edits) {
    assert.equal(source.split(before).length, 2, "Unexpected prompt output identity insertion point");
    source = source.replace(before, after);
  }
  return source;
}

function promptOutputIdentity(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP source; review the prompt output identity patch");
  return applyEdits(source, replacements);
}

function withoutPromptOutputIdentity(source) {
  source = withoutNativeBoundDispatch(source);
  if (!source.includes(guardedState)) {
    assert(!source.includes("output.route(async (event)"), "Unexpected prompt output identity patch");
    return source;
  }
  source = applyEdits(source, replacements.map(([before, after]) => [after, before]));
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected pre-patch ACP composition");
  return source;
}

module.exports = { promptOutputIdentity, withoutPromptOutputIdentity };
if (require.main === module) writeFileSync(bridgePath, promptOutputIdentity(readFileSync(bridgePath, "utf8")));
