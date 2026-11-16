import http from "node:http";
import { performance } from "node:perf_hooks";
import { LIMITS, SOURCE } from "./matrix.mjs";

function requireValue(value, message) {
  if (!value) throw new Error(message);
}

function localUrl(value) {
  const url = new URL(value);
  requireValue(
    url.protocol === "http:" &&
      ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) &&
      !url.username &&
      !url.password,
    "loopback_http_required",
  );
  return url;
}

// A deliberately incomplete JSON body cannot execute a file/browser mutation.
// The real Axum middleware admits the request before the JSON extractor waits.
// Never retry a holder: cancellation joins its socket before returning.
export async function holdBody({ url, headers, method, prefix }) {
  url = localUrl(url);
  requireValue(
    ["POST", "PUT"].includes(method) && prefix.endsWith(":"),
    "incomplete_json_prefix_required",
  );
  const state = {
    flushed: false,
    closed: false,
    response_status: null,
    transport_error: false,
    deadline: false,
  };
  const request = http.request(url, {
    method,
    agent: false,
    headers: {
      ...headers,
      "Content-Type": "application/json",
      "Content-Length": "1024",
      Connection: "close",
    },
  });
  const closed = new Promise((resolve) =>
    request.once("close", () => {
      state.closed = true;
      resolve();
    }),
  );
  request.on("response", (response) => {
    state.response_status = response.statusCode;
    response.resume();
  });
  request.on("error", () => {
    state.transport_error = true;
  });
  const timer = setTimeout(() => {
    state.deadline = true;
    request.destroy();
  }, 20000);
  async function close() {
    clearTimeout(timer);
    request.destroy();
    let joinTimer;
    try {
      await Promise.race([
        closed,
        new Promise((_, reject) => {
          joinTimer = setTimeout(
            () => reject(new Error("holder_close_unjoined")),
            3000,
          );
        }),
      ]);
    } finally {
      clearTimeout(joinTimer);
    }
  }
  try {
    await new Promise((resolve, reject) => {
      const onError = () => reject(new Error("holder_transport_failed"));
      request.once("error", onError);
      request.write(prefix, () => {
        request.removeListener("error", onError);
        state.flushed = true;
        resolve();
      });
    });
  } catch (error) {
    await close();
    throw error;
  }
  return { state, close };
}

export function assertSameServing(before, after, { am, origin, image }) {
  for (const sample of [before, after]) {
    requireValue(
      sample?.am === am &&
        sample.origin === origin &&
        sample.ready === true &&
        sample.draining === false &&
        sample.source === SOURCE &&
        sample.image === image,
      "serving_process_not_bound",
    );
    for (const key of ["pod_uid", "instance_id", "container_id"]) {
      requireValue(
        typeof sample[key] === "string" && sample[key].length > 0,
        "serving_identity_missing",
      );
      requireValue(sample[key] === before[key], "serving_process_changed");
    }
    requireValue(
      Number.isFinite(Date.parse(sample.observed_at)),
      "serving_time_missing",
    );
  }
  requireValue(
    Date.parse(after.observed_at) >= Date.parse(before.observed_at),
    "serving_time_reversed",
  );
}

function active(holders) {
  requireValue(
    holders.length === 2 &&
      holders.every(
        ({ state }) =>
          state.flushed &&
          !state.closed &&
          !state.transport_error &&
          state.response_status === null &&
          !state.deadline,
      ),
    "holders_not_live",
  );
}

/**
 * Called inside the sole parent's admitted local-kind run. api is the existing
 * cma/core.mjs API; rawRequest preserves the explicitly selected ingress.
 * sampleServing must read the parent's actual Pod/port-forward identity.
 * whileFull performs real retained-client input and interrupt/Stop and saves
 * independent receiver effects. Its return is evidence, never a verdict oracle.
 * This transport probe alone cannot close a compatibility matrix row.
 */
export async function capacityProbe({
  api,
  am,
  agentId,
  runtimeIncarnation,
  image,
  sampleServing,
  whileFull,
  record,
}) {
  requireValue(
    ["a", "b"].includes(am) && /^[-a-zA-Z0-9_]{1,160}$/.test(agentId),
    "fixture_identity_required",
  );
  requireValue(
    typeof runtimeIncarnation === "string" && runtimeIncarnation.length > 0,
    "runtime_incarnation_required",
  );
  requireValue(
    typeof whileFull === "function" && typeof record === "function",
    "parent_controls_and_recorder_required",
  );
  requireValue(
    typeof image === "string" && /@sha256:[a-f0-9]{64}$/.test(image),
    "exact_image_required",
  );
  const base = `/api/v1/agents/${agentId}`;
  const origin = localUrl(api.url(am, `${base}/files`)).origin;
  const selected = { am, origin, image };
  const initial = await sampleServing(am);
  assertSameServing(initial, initial, selected);
  const holders = [];
  const refusals = [];
  let controls, failure;
  try {
    for (const [path, method, prefix] of [
      [`${base}/files`, "POST", '{"path":'],
      [`${base}/browser`, "PUT", '{"runtime_incarnation":'],
    ]) {
      const hold = await holdBody({
        url: api.url(am, path),
        headers: api.headers(),
        method,
        prefix,
      });
      holders.push(hold);
    }
    // Give authentication/body admission time to execute. One attempt only;
    // failure to saturate is retained as a failed sample, not silently retried.
    await new Promise((resolve) => setTimeout(resolve, LIMITS.pollMinimumMs));
    active(holders);
    for (const path of [
      `${base}/files/tree?path=`,
      `${base}/browser?runtime_incarnation=${encodeURIComponent(runtimeIncarnation)}`,
    ]) {
      const before = await sampleServing(am);
      const began = performance.now();
      const response = await api.rawRequest(am, "GET", path, {
        allowed: [200, 429],
        timeout: 5000,
      });
      const elapsed_ms = performance.now() - began;
      const after = await sampleServing(am);
      const receipt = {
        method: "GET",
        path,
        status: response.status,
        code: response.body?.code,
        request_id: response.timing?.request_id,
        elapsed_ms,
        serving: before,
        serving_after: after,
      };
      refusals.push(receipt);
      await record({ event: "capacity_refusal", ...receipt });
      assertSameServing(initial, after, selected);
      assertSameServing(before, after, selected);
      requireValue(
        typeof receipt.request_id === "string" && receipt.request_id.length > 0,
        "request_id_missing",
      );
      requireValue(
        response.status === 429 && response.body?.code === "file_capacity",
        "typed_file_capacity_required",
      );
      requireValue(
        elapsed_ms <= LIMITS.refusalMaximumMs,
        "capacity_refusal_exceeds_one_second",
      );
      active(holders);
      await new Promise((resolve) => setTimeout(resolve, LIMITS.pollMinimumMs));
    }
    // The callback must finish input before Stop closes that Agent's runtime.
    controls = await whileFull();
    active(holders);
    const after = await sampleServing(am);
    assertSameServing(initial, after, selected);
  } catch (error) {
    failure = error;
  } finally {
    const results = await Promise.allSettled(
      holders.map((holder) => holder.close()),
    );
    await record({
      event: "capacity_cleanup",
      holders: holders.map(({ state }) => ({ ...state })),
      joined: results.every((result) => result.status === "fulfilled"),
    });
    if (results.some((result) => result.status === "rejected"))
      failure ??= new Error("holder_cleanup_failed");
  }
  if (failure) throw failure;
  return {
    refusals,
    controls,
    holders_closed: holders.every(({ state }) => state.closed),
  };
}
