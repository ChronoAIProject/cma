// Embedded into the SHA-pinned ACP bundle. Keep this factory self-contained.
function createOpenCompletionRuntime(require, agent, alive, invalidRequest = message => new Error(message)) {
  const crypto = require("node:crypto");
  const path = require("node:path");
  const records = new Map();
  const queuedTargets = new Set();
  const processNonce = crypto.randomUUID();
  let tail = Promise.resolve(), queued = 0, nativeWorkInFlight = 0, opening = null, dead = false;
  const canonical = value => {
    if (value === null || typeof value !== "object") return JSON.stringify(value);
    if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
    return "{" + Object.keys(value).sort().map(key => JSON.stringify(key) + ":" + canonical(value[key])).join(",") + "}";
  };
  const freeze = value => {
    if (value && typeof value === "object") {
      Object.values(value).forEach(freeze);
      Object.freeze(value);
    }
    return value;
  };
  function snapshot(params) {
    const value = { cwd: params.cwd, mcpServers: params.mcpServers ?? [] };
    const encoded = canonical(value);
    if (typeof value.cwd !== "string" || !path.isAbsolute(value.cwd) ||
        !Array.isArray(value.mcpServers) || value.mcpServers.length > 32 ||
        Buffer.byteLength(encoded, "utf8") > 65536) return null;
    const names = value.mcpServers.map(server => server?.name);
    if (names.some(name => typeof name !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(name)) ||
        new Set(names).size !== names.length ||
        !value.mcpServers.some(server => server.type === "http" &&
          ["cma_access", "cmaeg", "cma_channel"].includes(server.name))) return null;
    return { value: freeze(structuredClone(value)),
      digest: "v1:" + crypto.createHash("sha256").update(encoded).digest("hex") };
  }
  function live(record) {
    return !dead && alive() && record && !agent.sessionIsClosing(record.sessionId) &&
      agent.sessions.get(record.sessionId) === record.state &&
      agent.getSessionGeneration(record.sessionId) === record.sessionGeneration &&
      record.state.cwd === record.snapshot.cwd;
  }
  function retire(sessionId) {
    records.delete(sessionId);
    for (const target of queuedTargets) if (target.sessionId === sessionId) target.invalid = true;
    if (opening) {
      if (opening.sessionId === sessionId) opening.invalid = true;
      // A fresh new request learns its id only when native start returns.
      if (opening.closedIds.size < 64) opening.closedIds.add(sessionId);
      else opening.invalid = true;
    }
  }
  function retireAll() {
    dead = true;
    records.clear();
    for (const target of queuedTargets) target.invalid = true;
    if (opening) opening.invalid = true;
  }
  function observe(event) {
    if (event.eventType === "notification" && event.method === "thread/closed" &&
        typeof event.params?.threadId === "string") retire(event.params.threadId);
  }
  function installing(sessionId, state) {
    records.delete(sessionId);
    if (!opening) return;
    if (opening.sessionId && opening.sessionId !== sessionId) { opening.invalid = true; return; }
    opening.sessionId = sessionId;
    opening.state = state;
    opening.installs += 1;
    if (opening.closedIds.has(sessionId)) opening.invalid = true;
  }
  async function bounded(operation) {
    let timer;
    try {
      return await Promise.race([operation(), new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("Open evidence deadline")), 3000);
      })]);
    } finally { clearTimeout(timer); }
  }
  function exclusive(operation, sessionId) {
    if (queued >= 32) return Promise.reject(new Error("Open queue capacity exceeded"));
    // Preserve ACP's synchronous beginSessionOpen/getSessionState admission:
    // work delayed by this queue must never reopen or mutate a closed/replaced
    // target. A close fence can change even when its state/generation do not.
    const target = typeof sessionId === "string" ? { sessionId,
      state: agent.sessions.get(sessionId), generation: agent.getSessionGeneration(sessionId),
      invalid: agent.sessionIsClosing(sessionId) } : null;
    if (target) queuedTargets.add(target);
    queued += 1;
    const result = tail.then(() => {
      if (target) {
        queuedTargets.delete(target);
        if (target.invalid || agent.sessionIsClosing(sessionId) ||
            agent.getSessionGeneration(sessionId) !== target.generation ||
            agent.sessions.get(sessionId) !== target.state)
          throw invalidRequest("Session changed while operation was queued");
      }
      return operation();
    });
    tail = result.catch(() => {}).finally(() => { queued -= 1; });
    return result;
  }
  function open(kind, params, operation) {
    // Capture before the first await, including time spent behind another open.
    const request = structuredClone(params);
    const binding = snapshot(request);
    const requestedGeneration = kind === "new" ? null : agent.getSessionGeneration(request.sessionId);
    return exclusive(async () => {
      const previous = records.get(request.sessionId);
      const carried = live(previous) && binding?.digest === previous.digest ? previous : null;
      records.delete(request.sessionId);
      const current = { sessionId: request.sessionId, binding, carried, kind,
        invalid: nativeWorkInFlight > 0, closedIds: new Set(),
        installs: 0, nativeCalls: 0, nativeComplete: false, state: null };
      opening = current;
      try {
        const result = await operation(request);
        const id = kind === "new" ? result.sessionId : request.sessionId;
        const state = agent.sessions.get(id);
        if (binding && current.eligible && !current.invalid && current.nativeComplete &&
            current.nativeCalls === 1 && current.installs === 1 && current.sessionId === id &&
            current.nativeSessionId === id && state === current.state && state.cwd === binding.value.cwd &&
            !current.closedIds.has(id) && !dead && alive() && !agent.sessionIsClosing(id) &&
            (requestedGeneration === null || agent.getSessionGeneration(id) === requestedGeneration)) {
          // A cached resume retains the original configuration evidence. Never
          // promote the caller's newly supplied descriptors into prior proof.
          const original = carried?.snapshot ?? binding.value;
          if (records.size >= 32) records.delete(records.keys().next().value);
          records.set(id, { sessionId: id, state, snapshot: original, digest: binding.digest,
            sessionGeneration: agent.getSessionGeneration(id), openGeneration: crypto.randomUUID() });
        }
        return result;
      } finally {
        // A rejection, including history streaming failure, cannot leave proof.
        opening = null;
      }
    }, kind === "new" ? undefined : request.sessionId);
  }
  async function nativeOpen(kind, params, operation) {
    const current = opening;
    records.delete(params.threadId);
    if (!current || (current.kind === "new" ? kind !== "new" : kind !== "resume") ||
        (current.sessionId && current.sessionId !== params.threadId)) {
      if (current) current.invalid = true;
      return operation();
    }
    current.nativeCalls += 1;
    const binding = current.binding;
    if (binding && !current.invalid && current.nativeCalls === 1 && !dead && alive()) {
      try {
        const requested = binding.value.mcpServers;
        const configured = await bounded(() => agent.codexAcpClient.getConfigMcpServerNames(binding.value.cwd));
        const exact = params.cwd === binding.value.cwd && requested.every(server =>
          !configured.has(server.name) &&
          canonical(params.config?.mcp_servers?.[server.name]) ===
            canonical(agent.codexAcpClient.createMcpSeverConfig(server)));
        if (exact && current.kind === "new") current.eligible = true;
        else if (exact && current.carried && live(current.carried)) current.eligible = true;
        else if (exact) {
          const inventory = await bounded(() => agent.codexAcpClient.codexClient.sendRequest({
            method: "thread/loaded/list", params: { limit: 128 }
          }));
          // This narrower absence rule also excludes an unobserved loaded
          // subagent racing our resume. A fresh native process has no threads.
          current.eligible = Array.isArray(inventory.data) && inventory.data.length === 0 &&
            inventory.nextCursor == null;
        }
      } catch { current.eligible = false; }
    }
    const result = await operation();
    current.nativeSessionId = result.thread?.id;
    current.nativeComplete = typeof current.nativeSessionId === "string" &&
      result.cwd === binding?.value.cwd;
    if (!current.sessionId) current.sessionId = current.nativeSessionId;
    if (current.closedIds.has(current.sessionId)) current.invalid = true;
    return result;
  }
  async function nativeWork(operation) {
    nativeWorkInFlight += 1;
    promptStarting();
    try { return await operation(); } finally { nativeWorkInFlight -= 1; }
  }
  function promptStarting() {
    // A prompt may create native subagents. It must not race cold absence proof.
    if (opening) opening.invalid = true;
  }
  function read(params) {
    const base = { complete: false, version: 1, sessionId: params.sessionId, challenge: params.challenge };
    const record = records.get(params.sessionId);
    if (!live(record) || record.digest !== params.bindingDigest) return base;
    return { ...base, complete: true, bindingDigest: record.digest, processNonce,
      openGeneration: record.openGeneration, sessionGeneration: record.sessionGeneration };
  }
  function current(params) {
    // Only this process's completed, still-live opens; nothing is inferred.
    // A caller holding no session id can adopt exactly one of these, and an
    // open still in flight must be waited for rather than duplicated.
    const sessions = [];
    for (const record of records.values()) {
      if (live(record)) sessions.push({ sessionId: record.sessionId, bindingDigest: record.digest });
    }
    return { version: 1, challenge: params.challenge, processNonce, opening: opening !== null, sessions };
  }
  return { open, nativeOpen, nativeWork, exclusive, installing, retire, retireAll, observe, promptStarting, read,
    current };
}
module.exports = { createOpenCompletionRuntime };
