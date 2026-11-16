import { performance } from "node:perf_hooks";
import { LIMITS } from "./matrix.mjs";

function check(value, code) {
  if (!value) throw new Error(code);
}

// Exported for negative evidence tests. There is deliberately no injectable
// clock/duration in the connected observer below.
export function validateLifetime({
  elapsed_ms,
  close_initiator,
  socket_ids,
  samples,
  max_gap_ms,
}) {
  check(
    Number.isFinite(elapsed_ms) && elapsed_ms >= LIMITS.socketLifetimeMs,
    "early_socket_close",
  );
  // 10s send timeout + 2s revoke + 3s durable release + 2s close, with scheduling allowance.
  check(
    elapsed_ms <= LIMITS.socketLifetimeMs + 30000,
    "socket_lifetime_overrun",
  );
  check(close_initiator === "server", "server_close_not_observed");
  check(
    Array.isArray(socket_ids) &&
      socket_ids.length === 1 &&
      typeof socket_ids[0] === "string" &&
      socket_ids[0].length > 0,
    "original_socket_not_preserved",
  );
  check(
    Number.isInteger(samples) && samples >= 360 && max_gap_ms <= 15000,
    "lifetime_observation_gap",
  );
}

/**
 * Parent binds the exact retained Web/noVNC executable and installs observation
 * before Upgrade. openedAt is the parent's performance.now() at that Upgrade;
 * closed resolves from that ORIGINAL socket's real close event (not UI state).
 * sample() reads actual socket/receiver/frame state, never a fixture verdict.
 * Auto-reconnect creates a different socket and must fail this original-socket cell.
 * The parent must dispose session resources after any outcome, including abort.
 */
export async function observeLifetime({
  openedAt,
  socketId,
  closed,
  sample,
  record,
  signal,
}) {
  check(
    Number.isFinite(openedAt) &&
      openedAt <= performance.now() &&
      performance.now() - openedAt < 15000,
    "fresh_upgrade_timestamp_required",
  );
  check(
    typeof socketId === "string" && socketId.length > 0,
    "socket_identity_required",
  );
  check(
    closed && typeof closed.then === "function",
    "actual_close_event_required",
  );
  const deadline = openedAt + LIMITS.socketLifetimeMs + 30000;
  let terminal,
    closeError,
    last = openedAt,
    maxGap = 0,
    samples = 0;
  const joinedClose = Promise.resolve(closed).then(
    (event) => {
      terminal = { ...event, observedAt: performance.now() };
    },
    (error) => {
      closeError = error;
    },
  );
  while (!terminal && !closeError) {
    signal?.throwIfAborted();
    check(performance.now() <= deadline, "socket_lifetime_overrun");
    const sampleStarted = performance.now();
    let sampleTimer;
    let observation;
    try {
      observation = await Promise.race([
        sample(),
        new Promise((_, reject) => {
          sampleTimer = setTimeout(
            () => reject(new Error("lifetime_sample_timeout")),
            10000,
          );
        }),
      ]);
    } finally {
      clearTimeout(sampleTimer);
    }
    if (terminal || closeError) break;
    const now = performance.now();
    maxGap = Math.max(maxGap, now - last);
    last = now;
    check(
      observation.socket_id === socketId &&
        observation.frame_count > 0 &&
        observation.receiver_authorized === true,
      "original_live_socket_unproved",
    );
    samples += 1;
    await record({
      event: "desktop_lifetime_sample",
      elapsed_ms: now - openedAt,
      ...observation,
    });
    let timer;
    try {
      await Promise.race([
        joinedClose,
        new Promise((resolve) => {
          timer = setTimeout(
            resolve,
            Math.max(0, 5000 - (performance.now() - sampleStarted)),
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
  if (closeError) throw closeError;
  check(terminal?.socket_id === socketId, "close_socket_identity_mismatch");
  const result = {
    elapsed_ms: terminal.observedAt - openedAt,
    close_initiator: terminal.initiator,
    socket_ids: [socketId],
    samples,
    max_gap_ms: Math.max(maxGap, terminal.observedAt - last),
  };
  await record({ event: "desktop_lifetime_close", ...result });
  validateLifetime(result);
  return result;
}
