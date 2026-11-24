const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");

const sourceSha256 = "ddb625f66a5a14d94df3512a525c4a674a359e564602c09085769c8b61e2bca1";
const stopMarker = 'const cmaAdministrativeStop = Symbol("native administrative stop");';

function createNativePromptStarter(enabled, onBound) {
  // The native listener requires increasing sequences and permits gaps. Keep the
  // counter for this bridge process so reloading an ACP session cannot reset it.
  let sequence = 0;
  return async function startNativePrompt(client, turn, dispatchKey, shouldCancel) {
    if (!enabled || typeof dispatchKey !== "string" ||
        !/^cma-prompt-v1-[0-9a-fA-F]{64}$/.test(dispatchKey)) {
      return await client.turnStart(turn);
    }
    let capability;
    try {
      capability = await client.sendRequest({
        method: "turn/stopCapability",
        params: { threadId: turn.threadId },
      });
    } catch {
      // Discovery is read-only. There has been no native start at this point.
    }
    if (shouldCancel?.()) return null;
    const incarnation = capability?.listenerIncarnation;
    if (capability?.bindingVersion !== 1 || typeof incarnation !== "string" ||
        incarnation.length === 0 || incarnation.length > 128) {
      return await client.turnStart(turn);
    }
    if (sequence === 0xffffffff) throw new Error("Native dispatch sequence exhausted");
    const dispatchSequence = ++sequence;
    // A refused or lost bound-start response may represent an accepted dispatch.
    // Leave the original request to the recorder and never retry with turn/start.
    const target = {
      threadId: turn.threadId,
      listenerIncarnation: incarnation,
      dispatchSequence,
      dispatchKey,
    };
    const response = await client.cmaOpenCompletion.nativeWork(() => client.sendRequest({
      method: "turn/startBound",
      params: {
        turn,
        listenerIncarnation: target.listenerIncarnation,
        dispatchSequence: target.dispatchSequence,
        dispatchKey: target.dispatchKey,
      },
    }));
    if (capability.sealVersion === 1 && typeof response?.turn?.id === "string") {
      onBound?.(client, { ...target, turnId: response.turn.id });
    }
    return response;
  };
}

function createNativeStopCoordinator(enabled) {
  const clients = new WeakMap();

  function bind(client, target) {
    if (!enabled) return;
    let turns = clients.get(client);
    if (!turns) {
      turns = new Map();
      clients.set(client, turns);
    }
    turns.set(target.threadId, { target, receipt: null, fenced: false });
  }

  function release(client, threadId, turnId) {
    const turns = clients.get(client);
    if (turns?.get(threadId)?.target.turnId === turnId) turns.delete(threadId);
  }

  async function stop(client, threadId, turnId, dispatchKey, receipt) {
    const entry = clients.get(client)?.get(threadId);
    if (!entry || entry.target.turnId !== turnId || entry.target.dispatchKey !== dispatchKey || entry.fenced) {
      return { status: "hold" };
    }
    if (typeof receipt !== "string" || !/^[a-f0-9]{32}$/.test(receipt)) return { status: "hold" };
    if (entry.receipt) return { status: entry.receipt === receipt ? "pending" : "hold" };
    entry.receipt = receipt;
    try {
      const response = await client.sendRequest({ method: "turn/stopAndSeal", params: entry.target });
      // A response may acknowledge admission. Only the FIFO notification can release the prompt.
      return { status: response?.status === "pending" || response?.status === "canonical"
        ? response.status : "hold" };
    } catch {
      return { status: "hold" };
    }
  }

  function acceptFence(client, fence) {
    if (fence?.status !== "stoppedWithoutCanonical") return null;
    const entry = clients.get(client)?.get(fence.threadId);
    if (!entry?.receipt || entry.fenced || fence.turnId !== entry.target.turnId ||
        fence.listenerIncarnation !== entry.target.listenerIncarnation ||
        fence.dispatchSequence !== entry.target.dispatchSequence) return null;
    entry.fenced = true;
    return { ...entry.target, receipt: entry.receipt };
  }

  return { bind, stop, acceptFence, release };
}

function nativeBoundDispatchEdits() {
  return [
    ["getAgent().prompt(ctx.params, ctx.signal)",
      "getAgent().prompt(ctx.params, ctx.signal, void 0, ctx.requestId, " +
      "fence => ctx.client.notify(\"cma/codex/turn-stop-fence\", fence))"],
    ["async prompt(params, signal, onTurnStarted) {",
      "async prompt(params, signal, onTurnStarted, dispatchKey, publishStopFence) {"],
    [
      "          () => this.promptShouldStop(params.sessionId, activePrompt)\n        )",
      "          () => this.promptShouldStop(params.sessionId, activePrompt),\n" +
      "          dispatchKey\n        )",
    ],
    [
      "async sendPrompt(request, agentMode, modelId, serviceTier, disableSummary, cwd, additionalDirectories, onTurnStarted, shouldCancel) {",
      "async sendPrompt(request, agentMode, modelId, serviceTier, disableSummary, cwd, additionalDirectories, onTurnStarted, shouldCancel, exactDispatch) {",
    ],
    ["      serviceTier\n    }, onTurnStarted);",
      "      serviceTier\n    }, onTurnStarted, exactDispatch, shouldCancel);"],
    ["async runTurn(params, onTurnStarted) {",
      "async runTurn(params, onTurnStarted, exactDispatch, shouldCancel) {\n    let boundTurnId = null;"],
    ["      const turnStarted = await this.turnStart(params);",
      "      const turnStarted = await cmaStartNativePrompt(this, params, exactDispatch, shouldCancel);\n" +
      "      if (turnStarted === null) return null;\n      boundTurnId = turnStarted.turn.id;"],
    ["      return await this.awaitTurnCompleted(params.threadId, turnStarted.turn.id);\n    } finally {\n      releaseCapture();",
      "      return await this.awaitTurnCompleted(params.threadId, turnStarted.turn.id);\n    } finally {\n" +
      "      if (boundTurnId !== null) cmaNativeStop.release(this, params.threadId, boundTurnId);\n      releaseCapture();"],
    [
      "function extractTurnRouting(notification) {",
      `${stopMarker}
const cmaNativeStop = (${createNativeStopCoordinator.toString()})(process.env.CHRONO_CODEX_EXACT_TURN_SEAL_V1 === "1");
const cmaStartNativePrompt = (${createNativePromptStarter.toString()})(process.env.CHRONO_CODEX_EXACT_TURN_STOP_V1 === "1", cmaNativeStop.bind);
function extractTurnRouting(notification) {`,
    ],
    ["    const response = await handler(context(params, cx, responder.signal, responder.id));\n    await responder.respond(",
      "    const response = await handler(context(params, cx, responder.signal, responder.id));\n" +
      "    if (response === cmaAdministrativeStop) {\n" +
      "      responder.didRespond = true;\n      responder.finishRequest?.();\n      return;\n    }\n" +
      "    await responder.respond("],
    ["      const serverNotification = data;\n      if (isMcpServerStatusUpdatedNotification",
      "      const serverNotification = data;\n      if (serverNotification.method === \"turn/stopFence\") {\n" +
      "        const target = cmaNativeStop.acceptFence(this, serverNotification.params);\n" +
      "        if (target) this.recordTurnCompleted({ threadId: target.threadId, turn: { id: target.turnId }, [cmaAdministrativeStop]: target });\n" +
      "        return;\n      }\n      if (isMcpServerStatusUpdatedNotification"],
    ["    let promptNotificationsActive = true;",
      "    let promptNotificationsActive = true;\n    let administrativeStop = null;\n    let cmaOutputProjectionFailed = false;"],
    ["          await elicitationHandler.handleNotification(event);\n          await promptEventHandler.handleNotification(event);",
      "          try {\n            await elicitationHandler.handleNotification(event);\n" +
      "            await promptEventHandler.handleNotification(event);\n          } catch (error) {\n" +
      "            cmaOutputProjectionFailed = true;\n            throw error;\n          }"],
    ["            output.bind(turn);\n            pendingTurnStart?.resolve(turnId);",
      "            output.bind(turn);\n            activePrompt.cmaCanSeal = Boolean(publishStopFence) &&\n" +
      "              agentFileChangeReportRequest === null && sessionState.collaborationMode !== PLAN_COLLABORATION_MODE;\n" +
      "            pendingTurnStart?.resolve(turnId);"],
    ["      await this.codexAcpClient.waitForSessionNotifications(params.sessionId);\n      await eventHandler.flushPendingErrors();",
      "      await this.codexAcpClient.waitForSessionNotifications(params.sessionId);\n" +
      "      if (turnCompleted[cmaAdministrativeStop]) {\n" +
      "        administrativeStop = turnCompleted[cmaAdministrativeStop];\n        return cmaAdministrativeStop;\n      }\n" +
      "      await eventHandler.flushPendingErrors();"],
    ["      await eventHandler?.dispose();\n      disposePromptRequestCancellation();",
      "      try {\n        await eventHandler?.dispose();\n      } catch (error) {\n" +
      "        if (!administrativeStop) throw error;\n        cmaOutputProjectionFailed = true;\n      }\n" +
      "      disposePromptRequestCancellation();"],
    ["      activePrompt.complete();\n    }\n  }\n  async requestPlanImplementationPermission",
      "      try {\n        if (administrativeStop && publishStopFence && !output.hasLostOutput() &&\n" +
      "            !cmaOutputProjectionFailed && !activePrompt.signal.aborted && this.activePrompts.get(params.sessionId) === activePrompt) {\n" +
      "          await publishStopFence({ version: 1, ...administrativeStop });\n        }\n" +
      "      } catch (error) {\n        logger.error(\"Could not deliver native stop boundary\", error);\n" +
      "      } finally {\n        activePrompt.complete();\n      }\n    }\n  }\n" +
      "  async cmaStopNativeTurn(params) {\n" +
      "    const active = this.activePrompts.get(params?.sessionId);\n" +
      "    if (!active?.cmaCanSeal || active.signal.aborted || active.currentTurn?.threadId !== params.sessionId) return { status: \"hold\" };\n" +
      "    return await cmaNativeStop.stop(this.codexAcpClient.codexClient, params.sessionId, active.currentTurn.turnId, params.dispatchKey, params.receipt);\n" +
      "  }\n  async requestPlanImplementationPermission"],
    ['.onNotification(methods.agent.session.cancel, (ctx) => getAgent().cancel(ctx.params))',
      '.onNotification("cma/codex/stop-exact-turn", emptyExtensionParamsParser, (ctx) => getAgent().cmaStopNativeTurn(ctx.params))' +
      '.onNotification(methods.agent.session.cancel, (ctx) => getAgent().cancel(ctx.params))'],
  ];
}

function replaceNativeBoundDispatch(source, edits) {
  for (const [before, after] of edits) {
    assert.equal(source.split(before).length, 2, "Unexpected native dispatch insertion point");
    source = source.replace(before, after);
  }
  return source;
}

function applyNativeBoundDispatch(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP composition for native dispatch binding");
  return replaceNativeBoundDispatch(source, nativeBoundDispatchEdits());
}

function withoutNativeBoundDispatch(source) {
  if (!source.includes(stopMarker)) return source;
  const edits = nativeBoundDispatchEdits().reverse().map(([before, after]) => [after, before]);
  const original = replaceNativeBoundDispatch(source, edits);
  assert.equal(createHash("sha256").update(original).digest("hex"), sourceSha256,
    "Unexpected pre-binding ACP composition");
  return original;
}

module.exports = {
  createNativePromptStarter,
  createNativeStopCoordinator,
  applyNativeBoundDispatch,
  withoutNativeBoundDispatch,
};
if (require.main === module) {
  writeFileSync(bridgePath, applyNativeBoundDispatch(readFileSync(bridgePath, "utf8")));
}
