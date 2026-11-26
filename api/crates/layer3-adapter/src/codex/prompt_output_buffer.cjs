function createPromptOutputBuffer(enqueue, identify, isCurrentPrompt, reportOverflow) {
  const maxFrames = 128;
  const maxBytes = 1024 * 1024;
  // ACP 1.4.0 projects these methods as conversation or provider state.
  // A missing turn ID on any other method cannot qualify it for projection.
  const sessionMethods = new Set([
    "account/rateLimits/updated",
    "configWarning",
    "deprecationNotice",
    "fuzzyFileSearch/sessionCompleted",
    "fuzzyFileSearch/sessionUpdated",
    "serverRequest/resolved",
    "thread/archived",
    "thread/closed",
    "thread/goal/cleared",
    "thread/goal/updated",
    "thread/name/updated",
    "thread/settings/updated",
    "thread/status/changed",
    "thread/unarchived",
    "warning",
  ]);
  let pending = [];
  let bytes = 0;
  let boundTurn = null;
  let awaitingStart = true;
  let overflowed = false;
  let publish;

  function clearPending() {
    pending = [];
    bytes = 0;
  }

  function matches(identity, turn) {
    return identity.threadId === turn.threadId && identity.turnId === turn.turnId;
  }

  async function drain(turn) {
    if (boundTurn !== turn || !isCurrentPrompt()) return;
    const frames = pending;
    clearPending();
    for (const { identity, event } of frames) {
      if (boundTurn !== turn || !isCurrentPrompt()) return;
      if (matches(identity, turn)) await publish(event);
    }
  }

  function close() {
    awaitingStart = false;
    boundTurn = null;
    clearPending();
  }

  return {
    route(handler) {
      publish = handler;
      return async event => {
        const identity = identify(event);
        if (identity.turnId === null && sessionMethods.has(event.method)) {
          await publish(event);
          return;
        }
        if (!identity.turnId || !identity.threadId || !isCurrentPrompt()) return;
        const turn = boundTurn;
        if (turn !== null) {
          await drain(turn);
          if (boundTurn === turn && isCurrentPrompt() && matches(identity, turn)) await publish(event);
          return;
        }
        if (!awaitingStart || overflowed) return;
        const frameBytes = Buffer.byteLength(JSON.stringify(event));
        if (pending.length === maxFrames || frameBytes > maxBytes - bytes) {
          clearPending();
          overflowed = true;
          reportOverflow();
          return;
        }
        pending.push({ identity, event });
        bytes += frameBytes;
      };
    },
    bind(turn) {
      if (!awaitingStart || !isCurrentPrompt()) return;
      awaitingStart = false;
      boundTurn = turn;
      // A prompt can finish without another notification after this response.
      // Put the drain on the queue the ordinary terminal already waits for.
      enqueue(() => drain(turn));
    },
    nextTurn() {
      close();
      awaitingStart = true;
    },
    cancelPending() {
      if (awaitingStart) close();
    },
    hasLostOutput() {
      return overflowed;
    },
    close,
  };
}

module.exports = { createPromptOutputBuffer };
