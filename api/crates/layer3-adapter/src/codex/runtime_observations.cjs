const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { bridgePath } = require("./persist_conversations.cjs");
const sourceSha256 = "af393f07cda5fcea0c89b2659440094ebe31ca1c65b1920457b4126de4b7d602";

// Preserve the provider's consumed input (including cache reads) separately
// from the context-size observation. No polling, text estimates or shell cwd
// inference: only the app-server's canonical thread metadata is forwarded.
const usageBefore = `    const used = this.sessionState.lastTokenUsage?.totalTokens;
    const size = this.sessionState.modelContextWindow;
    if (used == null || size == null || size <= 0) {
      return null;
    }
    return {
      sessionUpdate: "usage_update",
      used,
      size
    };`;
const usageAfter = `    const total = params.tokenUsage.total;
    const last = params.tokenUsage.last;
    const counts = [total.inputTokens, total.outputTokens, last.inputTokens,
      last.outputTokens, last.totalTokens];
    if (!counts.every((n) => Number.isSafeInteger(n) && n >= 0) ||
        typeof params.turnId !== "string" || !params.turnId) return null;
    const size = params.tokenUsage.modelContextWindow;
    return {
      sessionUpdate: "usage_update",
      used: last.totalTokens,
      size: Number.isSafeInteger(size) && size > 0 ? size : 0,
      _meta: { cma: { usageSnapshot: {
        turnId: params.turnId,
        inputTokens: total.inputTokens, outputTokens: total.outputTokens,
        lastInputTokens: last.inputTokens, lastOutputTokens: last.outputTokens,
        contextUsed: last.totalTokens,
        contextWindow: Number.isSafeInteger(size) && size > 0 ? size : null,
        // The pinned provider's fill_to_context_window sentinel zeros consumed
        // counters. It is not evidence that those tokens were consumed again.
        countersReset: total.inputTokens === 0 && total.outputTokens === 0 &&
          total.totalTokens > 0
      } } }
    };`;
function runtimeObservations(source) {
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256,
    "Unexpected ACP source; review runtime observations before upgrading");
  const replace = (before, after, count = 1) => {
    assert.equal(source.split(before).length - 1, count, "Unexpected observation insertion point");
    source = source.split(before).join(after);
  };
  replace(usageBefore, usageAfter);
  // Before its first prompt (including after load), upstream has no notification
  // handler. Observe metadata there too; prompt subscription replaces this one.
  replace('  async subscribeToSessionEvents(sessionId, eventHandler, approvalHandler, elicitationHandler) {', `  subscribeToSessionMetadata(sessionId, eventHandler) {
    this.codexClient.onServerNotification(sessionId, (event) => {
      this.enqueueSessionNotification(sessionId, () => eventHandler(event));
    });
  }
  async subscribeToSessionEvents(sessionId, eventHandler, approvalHandler, elicitationHandler) {`);
  // The native bridge already distinguishes explicit names from prompt previews.
  // Preserve that evidence; text equality is not evidence of a native title.
  replace('          title: notification.params.threadName ?? null', `          title: notification.params.threadName ?? null,
          _meta: { cma: { runtimeMetadata: {
            description: notification.params.threadName ?? undefined,
            descriptionSource: "provider_title"
          } } }`);
  replace('        title: explicitTitle', `        title: explicitTitle,
        _meta: { cma: { runtimeMetadata: {
          description: explicitTitle, descriptionSource: "provider_title"
        } } }`);
  replace(`    const historyTitle = this.findFirstUserMessageTitle(thread) ?? this.normalizeSessionTitle(thread.preview);
    await this.publishFallbackSessionTitle(sessionState, historyTitle);`, `    const input = this.findFirstUserMessageInput(thread);
    const historyTitle = this.normalizeSessionTitle(input) ?? this.normalizeSessionTitle(thread.preview);
    await this.publishFallbackSessionTitle(sessionState, historyTitle, input, "first");`);
  replace('  findFirstUserMessageTitle(thread) {', '  findFirstUserMessageInput(thread) {');
  replace(`        const title = this.normalizeSessionTitle(item.content.filter((input) => input.type === "text").map((input) => input.text).join(" "));
        if (title) return title;`, `        const input = item.content.filter((input) => input.type === "text").map((input) => input.text).join(" ");
        if (this.normalizeSessionTitle(input)) return input;`);
  replace('  async publishFallbackSessionTitle(sessionState, title) {',
    '  async publishFallbackSessionTitle(sessionState, title, input, position) {');
  replace(`      sessionUpdate: "session_info_update",
      title
    });`, `      sessionUpdate: "session_info_update",
      title,
      _meta: { cma: { runtimeMetadata: {
        description: title, descriptionSource: "prompt_preview",
        descriptionInput: input, descriptionInputPosition: position
      } } }
    });`);
  replace('        this.createPromptFallbackTitle(params.prompt)\n', `        this.createPromptFallbackTitle(params.prompt),
        params.prompt.filter((block) => block.type === "text").map((block) => block.text).join(" "),
        "current"
`);
  // The persistence patch seeds empty threads with New Agent. That placeholder
  // must not suppress ACP's existing provider-history description on reload.
  replace('    const explicitTitle = this.normalizeSessionTitle(thread.name);',
    '    const explicitTitle = thread.name === "New Agent" ? null : this.normalizeSessionTitle(thread.name);');
  replace('      case "thread/settings/updated":\n', "");
  replace('      case "thread/tokenUsage/updated":', `      case "thread/settings/updated":
        this.sessionState.cwd = notification.params.threadSettings.cwd;
        return { sessionUpdate: "session_info_update", _meta: { cma: {
          runtimeMetadata: { workingDirectory: notification.params.threadSettings.cwd }
        } } };
      case "thread/tokenUsage/updated":`);
  replace('      thread: historyResponse.thread,', '      thread: { ...historyResponse.thread, cwd: response.cwd },');
  // New/resumed sessions expose the actual returned thread rather than the
  // requested cwd. Loading already returns its thread history snapshot.
  replace('      currentServiceTier: response.serviceTier ?? null,\n      additionalDirectories',
    '      currentServiceTier: response.serviceTier ?? null,\n      thread: { ...response.thread, cwd: response.cwd },\n      additionalDirectories', 2);
  replace('      cwd: request.cwd,\n      additionalDirectories: sessionMetadata.additionalDirectories,',
    '      cwd: sessionMetadata.thread?.cwd ?? request.cwd,\n      additionalDirectories: sessionMetadata.additionalDirectories,', 2);
  replace('    this.sessions.set(sessionId, sessionState);', `    this.sessions.set(sessionId, sessionState);
    const metadataHandler = new CodexEventHandler(this.connection, sessionState, false, false, this.sessionFailureEpoch);
    this.codexAcpClient.subscribeToSessionMetadata(sessionId, async (event) => {
      if (this.sessions.get(sessionId) === sessionState &&
          ["thread/name/updated", "thread/settings/updated"].includes(event.method)) {
        await metadataHandler.handleNotification(event);
      }
    });
    await new ACPSessionConnection(this.connection, sessionId).update({
      sessionUpdate: "session_info_update", _meta: { cma: { runtimeMetadata: {
        workingDirectory: sessionMetadata.thread?.cwd,
        description: (sessionMetadata.thread?.name !== "New Agent" ? sessionMetadata.thread?.name : undefined) || sessionMetadata.thread?.preview || undefined,
        descriptionSource: sessionMetadata.thread?.name && sessionMetadata.thread.name !== "New Agent"
          ? "provider_title" : sessionMetadata.thread?.preview ? "prompt_preview" : undefined
      } } }
    });`, 2);
  return source;
}
module.exports = { runtimeObservations, sourceSha256, usageBefore, usageAfter };
if (require.main === module) {
  assert.equal(process.argv.length, 2);
  writeFileSync(bridgePath, runtimeObservations(readFileSync(bridgePath, "utf8")));
}
