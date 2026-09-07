#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFile as execFileCallback } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs/promises";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);

const CODEX_VERSION = "codex-cli 0.147.0";
const ACP_VERSION = "1.4.0";
const ACP_HASH =
  "2c370f2337a5cc689d9a76b4bba1289d42afe6c17de824e2ca48004ba9670ab6";
const CODEX_LAUNCHER_HASH =
  "134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477";
const CODEX_NATIVE_HASHES = Object.freeze({
  x64: "cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40",
  arm64: "e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2",
});
const BWRAP_PATH = "/usr/bin/bwrap";
const IMAGE_ARCHITECTURES = Object.freeze({ amd64: "x64", arm64: "arm64" });
const EVIDENCE_SCHEMA = "cma-trigger-d09-v3";
const GATEWAY_PROBE_COMMAND = JSON.stringify([
  "codex",
  "sandbox",
  "--permission-profile",
  "cma-trigger",
  "--",
  "/usr/local/bin/layer4-runtime",
  "runtime-gateway-probe-helper-v2",
]);
const LAUNCH_ID = "12345678-1234-1234-1234-1234567890ab";
const SESSION_ID = `sesn_trg_${LAUNCH_ID.replaceAll("-", "")}`;
const RUNTIME_AUDIENCE = "runtime-d09";
const SERVICE_ID = "service-1";
const CATALOG_SERVICE_ID = "catalog-service-github";
const USER_SERVICE_SLUG = "api-github-2";
const FORK_REPOSITORY_ID = 30;
const FIXTURE_CREDENTIAL = "d09-synthetic-agent-key";
const PROVIDER_BODY_MARKER = "D09_PROVIDER_BODY_MUST_NOT_ESCAPE";
const POLICY_DENIED = "policy_denied";
const MANAGED_DENIAL_VALUE = "blocked-by-allowlist";
const ISOLATION_HEADER = "x-cma-trigger-loopback-isolation";
const ISOLATION_VALUE = "denied";
const WORKSPACE_SOURCE_REVISION = "a".repeat(40);
const WORKSPACE_PROVIDER_OBJECT = "b".repeat(40);
const WORKSPACE_EPOCH = 1;
const WORKSPACE_CHECKPOINT_VERSION = 1;
const WORKSPACE_MOUNT = "/home/chrono/workspace";
const WORKSPACE_ROOT = `${WORKSPACE_MOUNT}/repository`;
const WORKSPACE_WRITER_PID = `${WORKSPACE_ROOT}/.d09-writer-pid`;
const WORKSPACE_WRITER_OUTPUT = `${WORKSPACE_ROOT}/d09-writer.log`;

const PERMISSION_CONFIG = `model = "gpt-5.6-sol"
approval_policy = "never"
default_permissions = "cma-trigger"

[features]
network_proxy = true

[permissions.cma-trigger]
extends = ":workspace"

[permissions.cma-trigger.network]
enabled = true
enable_socks5 = false
enable_socks5_udp = false
allow_upstream_proxy = false
allow_local_binding = false

[permissions.cma-trigger.network.domains]
"127.0.0.2" = "allow"
`;

const FIXTURE_SERVER = String.raw`
const http = require("node:http");
const expectedPath = "/api/v1/proxy/catalog-service-github/repositories/30?_nyxid_via=service-1";
const state = {
  githubCalls: 0,
  modelCalls: 0,
  connectCalls: 0,
  unexpectedCalls: 0,
  authorizationFailures: 0,
  probeHeaderLeaks: 0,
};
const server = http.createServer((request, response) => {
  if (request.method === "GET" && request.url === "/__state") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify(state));
    return;
  }
  request.on("data", () => {});
  request.on("end", () => {
    if (request.url.startsWith("/model")) {
      state.modelCalls += 1;
      response.writeHead(500);
      response.end();
      return;
    }
    if (request.method !== "GET" || request.url !== expectedPath) {
      state.unexpectedCalls += 1;
      response.writeHead(418);
      response.end();
      return;
    }
    if (request.headers.authorization !== "Bearer d09-synthetic-agent-key") {
      state.authorizationFailures += 1;
      response.writeHead(401);
      response.end();
      return;
    }
    if (request.headers["x-cma-runtime-gateway-probe"] !== undefined) {
      state.probeHeaderLeaks += 1;
      response.writeHead(400);
      response.end();
      return;
    }
    state.githubCalls += 1;
    const statuses = [200, 403, 503];
    const status = statuses[state.githubCalls - 1] ?? 500;
    response.writeHead(status, {
      "content-type": "text/plain",
      "x-provider-body-marker": "present",
    });
    response.end("D09_PROVIDER_BODY_MUST_NOT_ESCAPE");
  });
});
server.on("connect", (_request, socket) => {
  state.connectCalls += 1;
  socket.end("HTTP/1.1 502 Closed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
});
server.listen(18080, "0.0.0.0");
`;

const IMAGE_FACTS = String.raw`
const crypto = require("node:crypto");
const fs = require("node:fs");
const cp = require("node:child_process");
const digest = (path) => crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex");
const native = {
  x64: "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex",
  arm64: "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex",
}[process.arch];
if (!native) throw new Error("unsupported Codex native architecture");
fs.accessSync("${BWRAP_PATH}", fs.constants.X_OK);
const facts = {
  codexVersion: cp.execFileSync("codex", ["--version"], { encoding: "utf8" }).trim(),
  acpVersion: require("/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/package.json").version,
  acpHash: digest("/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js"),
  bwrapPath: "${BWRAP_PATH}",
  bwrapVersion: cp.execFileSync("${BWRAP_PATH}", ["--version"], { encoding: "utf8" }).trim(),
  bwrapHash: digest("${BWRAP_PATH}"),
  codexLauncherHash: digest("/usr/local/lib/node_modules/@openai/codex/bin/codex.js"),
  codexNativeHash: digest(native),
  runtimeHash: digest("/usr/local/bin/layer4-runtime"),
  uid: process.getuid(),
  platform: process.platform,
  architecture: process.arch,
};
process.stdout.write(JSON.stringify(facts));
`;

const LISTENER_INVENTORY = String.raw`
const fs = require("node:fs");
function ipv4(hex) {
  return hex.match(/../g).reverse().map((part) => Number.parseInt(part, 16)).join(".");
}
const listeners = fs.readFileSync("/proc/net/tcp", "utf8")
  .trim().split("\n").slice(1)
  .map((line) => line.trim().split(/\s+/))
  .filter((fields) => fields[3] === "0A")
  .map((fields) => {
    const [address, port] = fields[1].split(":");
    return ipv4(address) + ":" + Number.parseInt(port, 16);
  })
  .filter((listener) => /:909[0-2]$/.test(listener))
  .sort();
process.stdout.write(JSON.stringify(listeners));
`;

const WRITER_OBSERVATION = String.raw`
const fs = require("node:fs");
const [pidPath, outputPath] = process.argv.slice(1);
const pid = Number(fs.readFileSync(pidPath, "utf8").trim());
if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error("invalid writer pid");
const stat = fs.readFileSync("/proc/" + pid + "/stat", "utf8");
const close = stat.lastIndexOf(")");
if (close < 0) throw new Error("malformed proc stat");
const fields = stat.slice(close + 1).trim().split(/\s+/);
const observation = {
  pid,
  state: fields[0],
  parent: Number(fields[1]),
  processGroup: Number(fields[2]),
  session: Number(fields[3]),
  bytes: fs.statSync(outputPath).size,
};
process.stdout.write(JSON.stringify(observation));
`;

const RAW_HTTP_CLIENT = String.raw`
const crypto = require("node:crypto");
const net = require("node:net");
const [method, host, rawPort, target, hostHeader, managedProxyRaw, tunnelTarget] = process.argv.slice(1);
const port = Number(rawPort);
const managedProxy = managedProxyRaw === "true";
let connectHost = host;
let connectPort = port;
let wireTarget = target;
if (managedProxy) {
  const rawProxy = process.env.HTTPS_PROXY || process.env.https_proxy || process.env.HTTP_PROXY || process.env.http_proxy;
  if (!rawProxy) throw new Error("managed proxy environment is absent");
  const proxy = new URL(rawProxy);
  if (proxy.protocol !== "http:") throw new Error("managed proxy is not HTTP");
  connectHost = proxy.hostname;
  connectPort = Number(proxy.port || 80);
  if (method !== "CONNECT") wireTarget = "http://" + hostHeader + target;
}
let settled = false;
let bytes = Buffer.alloc(0);
let awaitingTunnel = managedProxy && tunnelTarget.length > 0;
let tunnelEstablished = false;
const socket = net.createConnection({ host: connectHost, port: connectPort });
const timeout = setTimeout(() => finish(new Error("raw HTTP response timed out")), 5000);
function finish(error, networkDenied = false) {
  if (settled) return;
  settled = true;
  clearTimeout(timeout);
  socket.destroy();
  if (error && !networkDenied) {
    console.error(error.message);
    process.exitCode = 1;
    return;
  }
  if (networkDenied) {
    process.stdout.write(JSON.stringify({
      status: null,
      networkDenied: true,
      proxyUsed: managedProxy,
      tunnelEstablished,
      managedDenial: null,
      isolationDenial: null,
      gatewayOutcome: null,
      bodyBytes: 0,
      bodySha256: crypto.createHash("sha256").digest("hex"),
      headerNames: [],
    }));
    return;
  }
  const boundary = bytes.indexOf(Buffer.from("\r\n\r\n"));
  if (boundary < 0) {
    console.error("raw HTTP response had no header boundary");
    process.exitCode = 1;
    return;
  }
  const head = bytes.subarray(0, boundary).toString("latin1");
  const body = bytes.subarray(boundary + 4);
  const lines = head.split("\r\n");
  const match = /^HTTP\/\d(?:\.\d)?\s+(\d{3})\b/.exec(lines[0] ?? "");
  if (!match) {
    console.error("raw HTTP response had no status line");
    process.exitCode = 1;
    return;
  }
  const headers = {};
  for (const line of lines.slice(1)) {
    const at = line.indexOf(":");
    if (at > 0) headers[line.slice(0, at).toLowerCase()] = line.slice(at + 1).trim();
  }
  process.stdout.write(JSON.stringify({
    status: Number(match[1]),
    networkDenied: false,
    proxyUsed: managedProxy,
    tunnelEstablished,
    managedDenial: headers["x-proxy-error"] ?? null,
    isolationDenial: headers["x-cma-trigger-loopback-isolation"] ?? null,
    gatewayOutcome: headers["x-cma-runtime-gateway-outcome"] ?? null,
    bodyBytes: body.length,
    bodySha256: crypto.createHash("sha256").update(body).digest("hex"),
    headerNames: Object.keys(headers).sort(),
  }));
}
socket.on("connect", () => {
  if (awaitingTunnel) {
    socket.write("CONNECT " + tunnelTarget + " HTTP/1.1\r\nHost: " + tunnelTarget + "\r\nConnection: keep-alive\r\n\r\n");
  } else {
    socket.write(method + " " + wireTarget + " HTTP/1.1\r\nHost: " + hostHeader + "\r\nConnection: close\r\n\r\n");
  }
});
socket.on("data", (chunk) => {
  bytes = Buffer.concat([bytes, chunk]);
  if (bytes.length > 65536) finish(new Error("raw HTTP response exceeded the closed bound"));
  if (awaitingTunnel) {
    const boundary = bytes.indexOf(Buffer.from("\r\n\r\n"));
    if (boundary < 0) return;
    const line = bytes.subarray(0, boundary).toString("latin1").split("\r\n", 1)[0];
    const match = /^HTTP\/\d(?:\.\d)?\s+(\d{3})\b/.exec(line);
    if (!match || Number(match[1]) < 200 || Number(match[1]) >= 300) {
      finish();
      return;
    }
    awaitingTunnel = false;
    tunnelEstablished = true;
    bytes = Buffer.alloc(0);
    socket.write(method + " " + target + " HTTP/1.1\r\nHost: " + hostHeader + "\r\nConnection: close\r\n\r\n");
    return;
  }
  if (method === "CONNECT" && bytes.includes(Buffer.from("\r\n\r\n"))) finish();
});
socket.on("end", () => finish());
socket.on("error", () => finish(null, true));
`;

const STARTUP_REFUSAL = String.raw`
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const os = require("node:os");
const cp = require("node:child_process");
const spec = JSON.parse(process.env.D09_STARTUP_CASE);
const DIAGNOSTIC_LIMIT = 16 * 1024;
fs.mkdirSync(process.env.CHRONO_RUNTIME_WORKSPACE, { recursive: true });
function podAddress() {
  for (const values of Object.values(os.networkInterfaces())) {
    for (const value of values ?? []) {
      if (value.family === "IPv4" && !value.internal) return value.address;
    }
  }
  throw new Error("no non-loopback IPv4 address");
}
function listen() {
  if (!spec.occupy) return Promise.resolve(null);
  return new Promise((resolve, reject) => {
    const server = net.createServer((socket) => socket.destroy());
    server.once("error", reject);
    const host = spec.occupy.host === "pod-address" ? podAddress() : spec.occupy.host;
    server.listen(spec.occupy.port, host, () => resolve(server));
  });
}
function probeReady(address) {
  return new Promise((resolve) => {
    const request = http.get({ host: address, port: 9090, path: "/readyz", timeout: 150 }, (response) => {
      response.resume();
      resolve({ answered: true, status: response.statusCode ?? null });
    });
    request.on("timeout", () => request.destroy());
    request.on("error", () => resolve({ answered: false, status: null }));
  });
}
(async () => {
  const reservation = await listen();
  const environment = { ...process.env };
  delete environment.D09_STARTUP_CASE;
  for (const key of spec.remove ?? []) delete environment[key];
  Object.assign(environment, spec.set ?? {});
  const child = cp.spawn("/usr/local/bin/layer4-runtime", [], {
    env: environment,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const stdout = [];
  let stdoutBytes = 0;
  let stdoutOverflow = false;
  const stderr = [];
  let stderrBytes = 0;
  let stderrOverflow = false;
  child.stdout.on("data", (chunk) => {
    const remaining = DIAGNOSTIC_LIMIT - stdoutBytes;
    if (remaining > 0) {
      const retained = chunk.subarray(0, remaining);
      stdout.push(retained);
      stdoutBytes += retained.length;
    }
    if (chunk.length > remaining) stdoutOverflow = true;
  });
  child.stderr.on("data", (chunk) => {
    const remaining = DIAGNOSTIC_LIMIT - stderrBytes;
    if (remaining > 0) {
      const retained = chunk.subarray(0, remaining);
      stderr.push(retained);
      stderrBytes += retained.length;
    }
    if (chunk.length > remaining) stderrOverflow = true;
  });
  let readyAnswered = false;
  const readyStatuses = new Set();
  const addresses = [...new Set([podAddress(), "127.0.0.1"])];
  async function observeReady() {
    for (const address of addresses) {
      const observation = await probeReady(address);
      readyAnswered ||= observation.answered;
      if (observation.status !== null) readyStatuses.add(observation.status);
    }
  }
  let continueProbing = true;
  const readinessObserver = (async () => {
    while (continueProbing) {
      await observeReady();
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
  })();
  const exit = await Promise.race([
    new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", (code, signal) => resolve({ code, signal }));
    }),
    new Promise((resolve) => setTimeout(() => resolve(null), 8000)),
  ]);
  continueProbing = false;
  await readinessObserver;
  await observeReady();
  if (reservation) await new Promise((resolve) => reservation.close(resolve));
  if (!exit) {
    child.kill("SIGKILL");
    throw new Error("runtime did not fail startup within the bound");
  }
  if (
    exit.signal !== null ||
    exit.code !== spec.expectedExitCode ||
    readyAnswered
  ) {
    throw new Error("runtime answered readiness, was signalled, or returned the wrong refusal code");
  }
  if (stdoutOverflow || stderrOverflow) {
    throw new Error("runtime refusal diagnostics exceeded the closed bound");
  }
  const diagnostic = Buffer.concat([...stdout, ...stderr])
    .toString("utf8")
    .replace(/\u001b\[[0-9;]*m/g, "");
  if (
    !Array.isArray(spec.expectedDiagnosticMarkers) ||
    spec.expectedDiagnosticMarkers.length === 0 ||
    !spec.expectedDiagnosticMarkers.every((marker) =>
      typeof marker === "string" && marker.length > 0 && diagnostic.includes(marker)
    )
  ) {
    throw new Error("runtime did not emit the expected closed refusal marker");
  }
  process.stdout.write(JSON.stringify({
    name: spec.name,
    exitCode: exit.code,
    signal: null,
    readyAnswered: false,
    readyStatusCount: readyStatuses.size,
    markerObserved: true,
  }));
})().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
`;

class CampaignFailure extends Error {
  constructor(code, result = "fail", detail = code) {
    super(detail);
    this.name = "CampaignFailure";
    this.code = code;
    this.result = result;
  }
}

function sanitizeDiagnostic(value) {
  return String(value ?? "")
    .replaceAll(FIXTURE_CREDENTIAL, "[redacted-agent-key]")
    .replaceAll(PROVIDER_BODY_MARKER, "[redacted-provider-body]")
    .replace(
      /\bauthorization\s*:\s*bearer\s+[^\s,;]+/giu,
      "authorization: Bearer [redacted]",
    )
    .replace(/\bcookie\s*:\s*[^\r\n]+/giu, "cookie: [redacted]")
    .replace(/\bset-cookie\s*:\s*[^\r\n]+/giu, "set-cookie: [redacted]")
    .slice(-4000);
}

function closedCode(label) {
  return label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 80);
}

function parseArguments(argv) {
  const result = {
    image: "",
    output: "",
    buildCommit: "",
    expectedArchitecture: "amd64",
    selfTest: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--image") result.image = argv[++index] ?? "";
    else if (argument === "--output") result.output = argv[++index] ?? "";
    else if (argument === "--build-commit")
      result.buildCommit = argv[++index] ?? "";
    else if (argument === "--expected-architecture")
      result.expectedArchitecture = argv[++index] ?? "";
    else if (argument === "--self-test") result.selfTest = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  if (!result.selfTest && !result.image) throw new Error("--image is required");
  if (!(result.expectedArchitecture in IMAGE_ARCHITECTURES)) {
    throw new Error("--expected-architecture must be amd64 or arm64");
  }
  return result;
}

async function command(file, args, label, options = {}) {
  try {
    return await execFile(file, args, {
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
      timeout: options.timeout ?? 120_000,
    });
  } catch (error) {
    const diagnostic = sanitizeDiagnostic(
      error.stderr || error.stdout || error.message,
    );
    throw new CampaignFailure(
      `command_${closedCode(label) || "failed"}`,
      options.classification ?? "fail",
      `${label} failed${diagnostic ? `: ${diagnostic}` : ""}`,
    );
  }
}

async function docker(args, label, options) {
  return command("docker", args, label, options);
}

function environmentArguments(environment) {
  return Object.entries(environment).flatMap(([name, value]) => [
    "--env",
    `${name}=${value}`,
  ]);
}

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function u64(value) {
  const encoded = Buffer.alloc(8);
  encoded.writeBigUInt64BE(BigInt(value));
  return encoded;
}

function hashText(hash, value) {
  const encoded = Buffer.from(value, "utf8");
  hash.update(u64(encoded.length));
  hash.update(encoded);
}

function hashOptionalText(hash, value) {
  if (value === null) {
    hash.update(Buffer.from([0]));
    return;
  }
  hash.update(Buffer.from([1]));
  hashText(hash, value);
}

function workspaceFixture() {
  const payload = Buffer.from("D-09 workspace baseline\n", "utf8");
  const entry = {
    path: "README.md",
    kind: "regular",
    mode: 0o100644,
    size: payload.length,
    content_sha256: digest(payload),
    provider_object_id: WORKSPACE_PROVIDER_OBJECT,
    offset: 0,
    stored_size: payload.length,
    compression: "identity",
  };
  const manifest = crypto.createHash("sha256");
  manifest.update(Buffer.from("cma-workspace-manifest-v1\0", "utf8"));
  hashText(manifest, WORKSPACE_SOURCE_REVISION);
  manifest.update(u64(1));
  hashText(manifest, entry.path);
  manifest.update(Buffer.from([1]));
  const mode = Buffer.alloc(4);
  mode.writeUInt32BE(entry.mode);
  manifest.update(mode);
  manifest.update(u64(entry.size));
  hashText(manifest, entry.content_sha256);
  hashOptionalText(manifest, entry.provider_object_id);
  hashOptionalText(manifest, null);
  const manifestDigest = manifest.digest("hex");
  const header = Buffer.from(
    JSON.stringify({
      cma_workspace: 2,
      workspace_epoch: WORKSPACE_EPOCH,
      checkpoint_version: 0,
      quiesced_writers: 0,
      source_revision: WORKSPACE_SOURCE_REVISION,
      manifest_digest: manifestDigest,
      entries: [entry],
    }),
    "utf8",
  );
  const archive = Buffer.concat([u64(header.length), header, payload]);
  return {
    archive,
    archiveDigest: digest(archive),
    manifestDigest,
  };
}

async function restoreWorkspace(baseUrl, fixture) {
  const query = new URLSearchParams({
    epoch: String(WORKSPACE_EPOCH),
    source: WORKSPACE_SOURCE_REVISION,
    baseline: fixture.manifestDigest,
    manifest: fixture.manifestDigest,
    archive: fixture.archiveDigest,
  });
  const response = await fetch(
    `${baseUrl}/runtime/workspace?${query.toString()}`,
    { method: "POST", body: fixture.archive },
  );
  const body = await response.text();
  assert.equal(response.status, 200, `workspace restore failed: ${body}`);
  const receipt = JSON.parse(body);
  assert.deepEqual(receipt, {
    workspace_epoch: WORKSPACE_EPOCH,
    source_revision: WORKSPACE_SOURCE_REVISION,
    baseline_manifest_digest: fixture.manifestDigest,
    manifest_digest: fixture.manifestDigest,
    entries: 1,
    bytes: fixture.archive.subarray(
      8 + Number(fixture.archive.readBigUInt64BE(0)),
    ).length,
  });
}

async function holdWorkspaceCheckpoint(baseUrl, fixture) {
  const query = new URLSearchParams({
    epoch: String(WORKSPACE_EPOCH),
    version: String(WORKSPACE_CHECKPOINT_VERSION),
    source: WORKSPACE_SOURCE_REVISION,
    baseline: fixture.manifestDigest,
    disposition: "hold_for_teardown",
  });
  const response = await fetch(
    `${baseUrl}/runtime/workspace/checkpoint?${query.toString()}`,
    { method: "POST" },
  );
  const wire = Buffer.from(await response.arrayBuffer());
  assert.equal(response.status, 200, "workspace checkpoint hold failed");
  assert(wire.length >= 8);
  const acknowledgementBytes = Number(wire.readBigUInt64BE(0));
  assert(Number.isSafeInteger(acknowledgementBytes));
  assert(wire.length >= 8 + acknowledgementBytes);
  const acknowledgement = JSON.parse(
    wire.subarray(8, 8 + acknowledgementBytes).toString("utf8"),
  );
  const archive = wire.subarray(8 + acknowledgementBytes);
  assert.equal(acknowledgement.cma_checkpoint, 2);
  assert.equal(acknowledgement.workspace_epoch, WORKSPACE_EPOCH);
  assert.equal(
    acknowledgement.checkpoint_version,
    WORKSPACE_CHECKPOINT_VERSION,
  );
  assert.equal(acknowledgement.source_revision, WORKSPACE_SOURCE_REVISION);
  assert.equal(acknowledgement.writers_resumed, false);
  assert.equal(acknowledgement.writer_state, "held_for_teardown");
  assert.match(acknowledgement.writer_resume_token, /^[0-9a-f]{64}$/);
  assert(Number.isInteger(acknowledgement.quiesced_writers));
  assert(acknowledgement.quiesced_writers >= 2);
  assert.equal(digest(archive), acknowledgement.archive_sha256);
  return acknowledgement;
}

async function resumeWorkspaceWriters(baseUrl, resumeToken) {
  const response = await fetch(
    `${baseUrl}/runtime/workspace/checkpoint/resume`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ resume_token: resumeToken }),
    },
  );
  const body = await response.text();
  assert.equal(response.status, 200, `workspace writer resume failed: ${body}`);
  assert.deepEqual(JSON.parse(body), {
    resume_token: resumeToken,
    writers_resumed: true,
  });
}

function baseEnvironment(fixtureName, keyring) {
  return {
    CHRONO_RUNTIME_BIND: "0.0.0.0:9090",
    CHRONO_RUNTIME_COMMAND: "/bin/sh",
    CHRONO_RUNTIME_COMMANDS: "/bin/sh",
    CHRONO_RUNTIME_WORKSPACE: WORKSPACE_ROOT,
    CHRONO_RUNTIME_CONFIG_PATH: ".codex/config.toml",
    CHRONO_RUNTIME_CONFIG: PERMISSION_CONFIG,
    CHRONO_UPSTREAM_BASE_URL: `http://${fixtureName}:18080/model`,
    CHRONO_PLATFORM_BASE_URL: `http://${fixtureName}:18080`,
    CHRONO_GITHUB_CAPABILITY_REQUIRED: "true",
    CHRONO_GITHUB_CAPABILITY_VERIFY_KEYS: keyring,
    CHRONO_SESSION_ID: SESSION_ID,
    CHRONO_SESSION_REACHES: USER_SERVICE_SLUG,
    CHRONO_RUNTIME_GATEWAY_PROBE_COMMAND: GATEWAY_PROBE_COMMAND,
    INITIAL_AGENT_MODE: "cma-trigger",
  };
}

function signingFixture() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
  const spki = publicKey.export({ format: "der", type: "spki" });
  const rawPublicKey = spki.subarray(spki.length - 32);
  const keyId = "d09-key";
  return {
    keyring: JSON.stringify({ [keyId]: rawPublicKey.toString("base64") }),
    envelope(generation, fence) {
      const now = Math.floor(Date.now() / 1000);
      const document = {
        kind: "policy",
        version: 2,
        launch_id: LAUNCH_ID,
        session_id: SESSION_ID,
        runtime_audience: RUNTIME_AUDIENCE,
        service_id: SERVICE_ID,
        catalog_service_id: CATALOG_SERVICE_ID,
        user_service_slug: USER_SERVICE_SLUG,
        upstream: {
          repository_id: 10,
          repository_node_id: "R_upstream",
          owner_id: 11,
          owner_node_id: "U_upstream",
          canonical_owner: "upstream-owner",
          canonical_name: "project",
        },
        fork: {
          repository_id: FORK_REPOSITORY_ID,
          repository_node_id: "R_fork",
          owner_id: 31,
          owner_node_id: "U_fork",
          canonical_owner: "contributor",
          canonical_name: "project",
          fork_parent_id: 10,
          fork_source_id: 10,
        },
        allowed_ref: "refs/heads/cma/launch-d09",
        github_operation_classes: [
          "repository_read",
          "tree_read",
          "object_create",
          "launch_ref_create",
          "launch_ref_update",
          "status_read",
        ],
        generation,
        fence,
        not_before_epoch_seconds: now,
        expires_at_epoch_seconds: now + 59,
        issuer: "cma-d09",
        audience: "cma-layer4-github-capability-v2",
        signature_key_id: keyId,
        numeric_binding_observed_at: new Date().toISOString(),
      };
      const payload = Buffer.from(JSON.stringify(document));
      const message = Buffer.concat([
        Buffer.from("CMA_GITHUB_CAPABILITY_V2\0"),
        payload,
      ]);
      return JSON.stringify({
        version: 2,
        key_id: keyId,
        payload: payload.toString("base64url"),
        signature: crypto.sign(null, message, privateKey).toString("base64url"),
      });
    },
  };
}

async function waitFor(label, action, predicate, timeout = 30_000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await action();
      if (predicate(last)) return last;
    } catch (error) {
      last = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `${label} did not become ready: ${String(last?.message ?? last ?? "no result")}`,
  );
}

async function fixtureState(name) {
  const result = await docker(
    [
      "exec",
      name,
      "node",
      "-e",
      'fetch("http://127.0.0.1:18080/__state").then(async (r) => { if (!r.ok) throw new Error(String(r.status)); process.stdout.write(await r.text()); })',
    ],
    "read D-09 fixture state",
  );
  return JSON.parse(result.stdout);
}

async function inspectContainer(name, format) {
  const result = await docker(
    ["inspect", "--format", format, name],
    `inspect ${name}`,
  );
  return result.stdout.trim();
}

async function fetchJson(url, options) {
  const response = await fetch(url, options);
  const text = await response.text();
  if (!response.ok) throw new Error(`${url} returned HTTP ${response.status}`);
  return JSON.parse(text);
}

async function gatewayProbe(
  baseUrl,
  serviceId,
  resourceId,
  expected,
  catalogServiceId = CATALOG_SERVICE_ID,
  userServiceSlug = USER_SERVICE_SLUG,
) {
  const receipt = await fetchJson(`${baseUrl}/runtime/gateway-probe`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      version: 2,
      service_id: serviceId,
      catalog_service_id: catalogServiceId,
      user_service_slug: userServiceSlug,
      resource_id: resourceId,
    }),
  });
  if (receipt?.outcome === "command_sandbox_initialization_failed") {
    throw new CampaignFailure(
      "command_sandbox_initialization_failed",
      "inconclusive",
    );
  }
  assert.deepEqual(receipt, { version: 2, outcome: expected });
  const rendered = JSON.stringify(receipt);
  assert(!rendered.includes(PROVIDER_BODY_MARKER));
  assert(!rendered.includes(FIXTURE_CREDENTIAL));
  return receipt.outcome;
}

async function installCapability(control, signing, generation) {
  const installed = await control.request(
    {
      t: "github_capability",
      operation_id: `install-policy-${generation}`,
      envelope: signing.envelope(generation, generation),
    },
    "github_capability_result",
  );
  assert.deepEqual(
    {
      accepted: installed.accepted,
      generation: installed.generation,
      fence: installed.fence,
      revoked: installed.revoked,
    },
    { accepted: true, generation, fence: generation, revoked: false },
  );
  return generation;
}

class ControlSocket {
  constructor(socket) {
    this.socket = socket;
    this.messages = [];
    this.waiters = [];
    socket.addEventListener("message", (event) => {
      if (typeof event.data !== "string") return;
      let parsed;
      try {
        parsed = JSON.parse(event.data);
      } catch {
        return;
      }
      this.messages.push(parsed);
      for (const wake of this.waiters.splice(0)) wake();
    });
  }

  async wait(predicate, label, timeout = 10_000) {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
      const index = this.messages.findIndex(predicate);
      if (index >= 0) return this.messages.splice(index, 1)[0];
      await Promise.race([
        new Promise((resolve) => this.waiters.push(resolve)),
        new Promise((resolve) => setTimeout(resolve, 100)),
      ]);
    }
    throw new Error(`timed out waiting for ${label}`);
  }

  async request(frame, resultTag) {
    this.socket.send(JSON.stringify(frame));
    return this.wait(
      (message) =>
        message.t === resultTag && message.operation_id === frame.operation_id,
      `${resultTag}/${frame.operation_id}`,
    );
  }

  sendTerminal(bytes) {
    this.socket.send(
      Buffer.concat([Buffer.from([0x01]), Buffer.from(bytes, "utf8")]),
    );
  }

  close() {
    this.socket.close();
  }
}

async function openControlSocket(url) {
  const socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("control WebSocket open timed out")),
      10_000,
    );
    socket.addEventListener("open", () => {
      clearTimeout(timer);
      resolve();
    });
    socket.addEventListener("error", () => {
      clearTimeout(timer);
      reject(new Error("control WebSocket failed"));
    });
  });
  const control = new ControlSocket(socket);
  const ready = await control.wait(
    (message) => message.t === "ready",
    "runtime ready frame",
  );
  assert.equal(ready.wireVersion, 1);
  assert.deepEqual(ready.lanes, ["ctl", "term"]);
  return control;
}

async function writerObservation(container) {
  const result = await docker(
    [
      "exec",
      container,
      "node",
      "-e",
      WRITER_OBSERVATION,
      WORKSPACE_WRITER_PID,
      WORKSPACE_WRITER_OUTPUT,
    ],
    "observe daemonized workspace writer",
  );
  return JSON.parse(result.stdout);
}

async function writerProcessExists(container, pid) {
  const result = await docker(
    [
      "exec",
      container,
      "node",
      "-e",
      'process.stdout.write(String(require("node:fs").existsSync("/proc/" + process.argv[1])))',
      String(pid),
    ],
    "observe adopted writer cleanup",
  );
  return result.stdout.trim() === "true";
}

async function proveWorkspaceWriterFence(control, container, baseUrl, fixture) {
  const writerLoop = `printf "%s\\n" "$$" > ${WORKSPACE_WRITER_PID}; while :; do printf x >> ${WORKSPACE_WRITER_OUTPUT}; sleep 0.01; done`;
  const launcher = `const cp=require("node:child_process");const child=cp.spawn("/usr/bin/setsid",["-f","/bin/sh","-c",${JSON.stringify(writerLoop)}],{detached:true,stdio:"ignore"});child.unref()`;
  assert(!launcher.includes("'"));
  control.sendTerminal(`node -e '${launcher}'\n`);

  const active = await waitFor(
    "fully daemonized workspace writer",
    () => writerObservation(container),
    (observation) =>
      observation.bytes > 128 &&
      observation.parent === 1 &&
      observation.processGroup === observation.pid &&
      observation.session === observation.pid &&
      !["T", "t", "Z", "X"].includes(observation.state),
  );

  const checkpoint = await holdWorkspaceCheckpoint(baseUrl, fixture);
  const stopped = await writerObservation(container);
  assert.equal(stopped.pid, active.pid);
  assert(["T", "t"].includes(stopped.state));
  const frozenBytes = stopped.bytes;
  await new Promise((resolve) => setTimeout(resolve, 250));
  const stillStopped = await writerObservation(container);
  assert.equal(stillStopped.pid, active.pid);
  assert(["T", "t"].includes(stillStopped.state));
  assert.equal(stillStopped.bytes, frozenBytes);

  await resumeWorkspaceWriters(baseUrl, checkpoint.writer_resume_token);
  const resumed = await waitFor(
    "daemonized workspace writer resume",
    () => writerObservation(container),
    (observation) =>
      observation.pid === active.pid &&
      observation.bytes > frozenBytes &&
      !["T", "t", "Z", "X"].includes(observation.state),
  );

  control.sendTerminal(`kill -KILL "$(cat ${WORKSPACE_WRITER_PID})"\n`);
  await waitFor(
    "adopted workspace writer reap",
    () => writerProcessExists(container, active.pid),
    (exists) => exists === false,
  );

  return {
    boundary: "linux_child_subreaper_v1",
    fullyDaemonized: true,
    adoptedBySupervisor: true,
    escapedOriginalSession: true,
    quiescedWriters: checkpoint.quiesced_writers,
    stableWhileHeld: stillStopped.bytes === frozenBytes,
    resumed: resumed.bytes > frozenBytes,
    reapedAfterExit: true,
  };
}

async function rawRequest(
  container,
  method,
  host,
  port,
  target,
  {
    sandboxed = false,
    managedProxy = sandboxed,
    hostHeader = `${host}:${port}`,
    tunnelTarget = "",
  } = {},
) {
  const client = [
    "node",
    "-e",
    RAW_HTTP_CLIENT,
    method,
    host,
    String(port),
    target,
    hostHeader,
    String(managedProxy),
    tunnelTarget,
  ];
  const result = await docker(
    [
      "exec",
      "--workdir",
      WORKSPACE_ROOT,
      container,
      ...(sandboxed
        ? ["codex", "sandbox", "--permission-profile", "cma-trigger", "--"]
        : []),
      ...client,
    ],
    `${sandboxed ? "sandboxed" : "direct"} raw ${method} refusal`,
    { classification: sandboxed ? "inconclusive" : "fail" },
  );
  const observation = JSON.parse(result.stdout);
  assert.deepEqual(Object.keys(observation).sort(), [
    "bodyBytes",
    "bodySha256",
    "gatewayOutcome",
    "headerNames",
    "isolationDenial",
    "managedDenial",
    "networkDenied",
    "proxyUsed",
    "status",
    "tunnelEstablished",
  ]);
  assert(Number.isInteger(observation.bodyBytes));
  assert.match(observation.bodySha256, /^[0-9a-f]{64}$/);
  assert.equal(observation.proxyUsed, managedProxy);
  assert.equal(observation.tunnelEstablished, tunnelTarget.length > 0);
  return observation;
}

async function sandboxObservation(container, url) {
  const parsed = new URL(url);
  const port = Number(parsed.port || (parsed.protocol === "https:" ? 443 : 80));
  return rawRequest(
    container,
    "GET",
    parsed.hostname,
    port,
    `${parsed.pathname}${parsed.search}`,
    {
      sandboxed: true,
      hostHeader: parsed.port ? parsed.host : parsed.hostname,
    },
  );
}

function requireManagedDenial(observation) {
  assert.equal(observation.status, 403);
  assert.equal(observation.networkDenied, false);
  assert.equal(observation.managedDenial, MANAGED_DENIAL_VALUE);
  assert.equal(observation.isolationDenial, null);
}

function requireIsolationDenial(observation) {
  assert.equal(observation.status, 403);
  assert.equal(observation.networkDenied, false);
  assert.equal(observation.isolationDenial, ISOLATION_VALUE);
  assert.equal(observation.managedDenial, null);
  assert.equal(observation.gatewayOutcome, null);
  assert.equal(observation.bodyBytes, 0);
  const allowedHeaders = new Set([
    "connection",
    "content-length",
    "content-type",
    "date",
    ISOLATION_HEADER,
  ]);
  assert(observation.headerNames.every((name) => allowedHeaders.has(name)));
}

async function startupRefusal(image, network, environment, spec) {
  const result = await docker(
    [
      "run",
      "--rm",
      "--network",
      network,
      "--entrypoint",
      "node",
      ...environmentArguments(environment),
      "--env",
      `D09_STARTUP_CASE=${JSON.stringify(spec)}`,
      image,
      "-e",
      STARTUP_REFUSAL,
    ],
    `startup refusal ${spec.name}`,
    { timeout: 20_000 },
  );
  const receipt = JSON.parse(result.stdout);
  assert.deepEqual(Object.keys(receipt).sort(), [
    "exitCode",
    "markerObserved",
    "name",
    "readyAnswered",
    "readyStatusCount",
    "signal",
  ]);
  assert.equal(receipt.name, spec.name);
  assert.equal(receipt.exitCode, 2);
  assert.equal(receipt.signal, null);
  assert.equal(receipt.readyAnswered, false);
  assert.equal(receipt.readyStatusCount, 0);
  assert.equal(receipt.markerObserved, true);
  return receipt;
}

const REDACTION_STATEMENT =
  "Only closed outcomes, status classes, artifact hashes, synthetic identities, and counts are retained; provider bodies and credential values are discarded.";

function failureFrom(error) {
  if (error instanceof CampaignFailure) {
    return { code: error.code, result: error.result };
  }
  if (error instanceof assert.AssertionError) {
    return { code: "campaign_assertion_failed", result: "fail" };
  }
  return { code: "campaign_failed", result: "fail" };
}

function campaignErrorFrom(error, stage) {
  if (error instanceof CampaignFailure) return error;
  const closedStage = closedCode(stage) || "unknown";
  return new CampaignFailure(
    error instanceof assert.AssertionError
      ? `assertion_${closedStage}`
      : `campaign_${closedStage}_failed`,
  );
}

function imageEvidence(options, inspected) {
  if (!inspected) return null;
  const revision =
    inspected.Config?.Labels?.["org.opencontainers.image.revision"] ??
    "local-unpinned";
  return {
    reference: options.image,
    id: inspected.Id,
    repoDigests: inspected.RepoDigests ?? [],
    os: inspected.Os,
    architecture: inspected.Architecture,
    revision,
  };
}

function failureEvidence(
  options,
  inspected,
  facts,
  startedAt,
  failure,
  cleanupFailures,
) {
  return {
    schema: EVIDENCE_SCHEMA,
    evidenceClass: "D",
    result: cleanupFailures.length > 0 ? "fail" : failure.result,
    startedAt,
    recordedAt: new Date().toISOString(),
    buildCommit:
      inspected?.Config?.Labels?.["org.opencontainers.image.revision"] ??
      (options.buildCommit || "unknown"),
    image: imageEvidence(options, inspected),
    artifacts: facts ?? null,
    failure: {
      code: cleanupFailures.length > 0 ? "cleanup_failed" : failure.code,
    },
    cleanup: {
      status: cleanupFailures.length === 0 ? "verified" : "failed",
      failureCodes: cleanupFailures,
    },
    redaction: REDACTION_STATEMENT,
  };
}

function guardEvidence(evidence) {
  const rendered = JSON.stringify(evidence);
  if (
    !rendered.includes(FIXTURE_CREDENTIAL) &&
    !rendered.includes(PROVIDER_BODY_MARKER) &&
    !/authorization\s*:\s*bearer/iu.test(rendered) &&
    !/\b(?:set-)?cookie\s*:/iu.test(rendered)
  ) {
    return evidence;
  }
  return {
    schema: EVIDENCE_SCHEMA,
    evidenceClass: "D",
    result: "fail",
    startedAt: evidence.startedAt ?? null,
    recordedAt: new Date().toISOString(),
    buildCommit: "redacted",
    image: null,
    artifacts: null,
    failure: { code: "evidence_redaction_guard" },
    cleanup: evidence.cleanup ?? { status: "unknown", failureCodes: [] },
    redaction: REDACTION_STATEMENT,
  };
}

async function emitEvidence(output, evidence) {
  const guarded = guardEvidence(evidence);
  const rendered = `${JSON.stringify(guarded, null, 2)}\n`;
  if (output) await fs.writeFile(output, rendered, { mode: 0o600 });
  process.stdout.write(rendered);
  return guarded;
}

async function cleanupDockerResource(args, label, missingPattern) {
  try {
    await docker(args, label);
  } catch (error) {
    if (missingPattern.test(error.message)) return;
    throw error;
  }
}

async function cleanupCampaign(
  control,
  containers,
  network,
  networkRequested,
  label,
) {
  const failures = [];
  try {
    control?.close();
  } catch {
    failures.push("control_socket_close_failed");
  }
  for (const name of [...containers].reverse()) {
    try {
      await cleanupDockerResource(
        ["rm", "--force", name],
        `remove ${name}`,
        /no such container/iu,
      );
    } catch {
      failures.push("container_remove_failed");
    }
  }
  if (networkRequested) {
    try {
      await cleanupDockerResource(
        ["network", "rm", network],
        `remove ${network}`,
        /no such network|not found/iu,
      );
    } catch {
      failures.push("network_remove_failed");
    }
  }
  try {
    const containersLeft = await docker(
      ["ps", "--all", "--quiet", "--filter", `label=${label}`],
      "verify D-09 container cleanup",
    );
    const networksLeft = await docker(
      ["network", "ls", "--quiet", "--filter", `label=${label}`],
      "verify D-09 network cleanup",
    );
    if (containersLeft.stdout.trim() || networksLeft.stdout.trim()) {
      failures.push("labelled_resource_remains");
    }
  } catch {
    failures.push("cleanup_verification_failed");
  }
  return [...new Set(failures)].sort();
}

function runSelfTests() {
  for (const source of [
    FIXTURE_SERVER,
    IMAGE_FACTS,
    LISTENER_INVENTORY,
    WRITER_OBSERVATION,
    RAW_HTTP_CLIENT,
    STARTUP_REFUSAL,
  ]) {
    assert.doesNotThrow(() => new Function(source));
  }
  const sanitized = sanitizeDiagnostic(
    `${FIXTURE_CREDENTIAL} ${PROVIDER_BODY_MARKER} Authorization: Bearer unsafe Cookie: unsafe`,
  );
  assert(!sanitized.includes(FIXTURE_CREDENTIAL));
  assert(!sanitized.includes(PROVIDER_BODY_MARKER));
  assert(!sanitized.includes("Bearer unsafe"));
  assert(!sanitized.includes("Cookie: unsafe"));
  const guarded = guardEvidence({
    schema: EVIDENCE_SCHEMA,
    evidenceClass: "D",
    result: "pass",
    leaked: FIXTURE_CREDENTIAL,
    cleanup: { status: "verified", failureCodes: [] },
  });
  assert.equal(guarded.result, "fail");
  assert.equal(guarded.failure.code, "evidence_redaction_guard");
  const guardedCookie = guardEvidence({
    schema: EVIDENCE_SCHEMA,
    evidenceClass: "D",
    result: "pass",
    leaked: "Cookie: unsafe",
    cleanup: { status: "verified", failureCodes: [] },
  });
  assert.equal(guardedCookie.failure.code, "evidence_redaction_guard");
  assert.equal(CODEX_NATIVE_HASHES.x64.length, 64);
  assert.equal(CODEX_NATIVE_HASHES.arm64.length, 64);
  const workspace = workspaceFixture();
  assert.match(workspace.archiveDigest, /^[0-9a-f]{64}$/);
  assert.match(workspace.manifestDigest, /^[0-9a-f]{64}$/);
  assert.match(SESSION_ID, /^sesn_trg_[0-9a-f]{32}$/);
  assert.equal(
    campaignErrorFrom(new Error("unsafe diagnostic"), "runtime-startup").code,
    "campaign_runtime_startup_failed",
  );
  const retained = new CampaignFailure("typed_failure", "inconclusive");
  assert.equal(campaignErrorFrom(retained, "runtime-startup"), retained);
  assert.equal(
    campaignErrorFrom(
      new assert.AssertionError({
        actual: "actual",
        expected: "expected",
        operator: "strictEqual",
      }),
      "runtime-startup",
    ).code,
    "assertion_runtime_startup",
  );
  process.stdout.write("CMA Trigger D-09 self-test passed\n");
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  if (options.selfTest) {
    runSelfTests();
    return;
  }
  const startedAt = new Date().toISOString();
  const suffix = `${process.pid}-${Date.now()}`;
  const network = `cma-trigger-d09-${suffix}`;
  const fixture = `cma-trigger-d09-fixture-${suffix}`;
  const runtime = `cma-trigger-d09-runtime-${suffix}`;
  const resourceLabel = `cma.trigger.d09=${suffix}`;
  const signing = signingFixture();
  const createdContainers = [];
  let control;
  let networkRequested = false;
  let inspected;
  let facts;
  let workspaceFence;
  let evidence;
  let campaignError;
  let campaignStage = "image-inspection";

  try {
    inspected = JSON.parse(
      (await docker(["image", "inspect", options.image], "inspect D-09 image"))
        .stdout,
    )[0];
    assert.equal(inspected.Os, "linux");
    assert.equal(inspected.Architecture, options.expectedArchitecture);
    assert.match(inspected.Id, /^sha256:[0-9a-f]{64}$/);
    const imageRevision =
      inspected.Config?.Labels?.["org.opencontainers.image.revision"] ??
      "local-unpinned";
    if (options.buildCommit) {
      assert.match(options.buildCommit, /^[0-9a-f]{40}$/);
      assert.equal(imageRevision, options.buildCommit);
    }

    campaignStage = "artifact-identity";
    facts = JSON.parse(
      (
        await docker(
          [
            "run",
            "--rm",
            "--entrypoint",
            "node",
            options.image,
            "-e",
            IMAGE_FACTS,
          ],
          "inspect pinned runtime artifacts",
        )
      ).stdout,
    );
    assert.equal(facts.codexVersion, CODEX_VERSION);
    assert.equal(facts.acpVersion, ACP_VERSION);
    assert.equal(facts.acpHash, ACP_HASH);
    assert.equal(facts.bwrapPath, BWRAP_PATH);
    assert.match(
      facts.bwrapVersion,
      /^bubblewrap \d+\.\d+(?:\.\d+)?(?:[-+~].*)?$/,
    );
    assert.match(facts.bwrapHash, /^[0-9a-f]{64}$/);
    assert.equal(facts.codexLauncherHash, CODEX_LAUNCHER_HASH);
    assert.equal(
      facts.codexNativeHash,
      CODEX_NATIVE_HASHES[IMAGE_ARCHITECTURES[options.expectedArchitecture]],
    );
    assert.equal(facts.uid, 10001);
    assert.equal(facts.platform, "linux");
    assert.equal(
      facts.architecture,
      IMAGE_ARCHITECTURES[options.expectedArchitecture],
    );

    campaignStage = "fixture-startup";
    networkRequested = true;
    await docker(
      ["network", "create", "--label", resourceLabel, network],
      "create D-09 network",
    );

    createdContainers.push(fixture);
    await docker(
      [
        "run",
        "--detach",
        "--name",
        fixture,
        "--label",
        resourceLabel,
        "--network",
        network,
        "--entrypoint",
        "node",
        options.image,
        "-e",
        FIXTURE_SERVER,
      ],
      "start D-09 provider fixture",
    );
    await waitFor(
      "D-09 provider fixture",
      () => fixtureState(fixture),
      (state) => state.githubCalls === 0,
    );

    campaignStage = "runtime-startup";
    const environment = baseEnvironment(fixture, signing.keyring);
    createdContainers.push(runtime);
    await docker(
      [
        "run",
        "--detach",
        "--name",
        runtime,
        "--label",
        resourceLabel,
        "--network",
        network,
        // These outer-container accommodations let the system bwrap establish
        // Codex's nested user/mount boundary: the seccomp filter permits its
        // namespace syscalls, dropping docker-default lets /usr/bin/bwrap
        // attach the targeted host AppArmor profile, and unmasking system paths
        // permits the sandbox's fresh /proc mount. No capabilities, privileged
        // mode, or host network are granted; the inner permission profile is
        // still mandatory and is exercised below.
        "--security-opt",
        "seccomp=unconfined",
        "--security-opt",
        "apparmor=unconfined",
        "--security-opt",
        "systempaths=unconfined",
        "--publish",
        "127.0.0.1::9090",
        "--tmpfs",
        `${WORKSPACE_MOUNT}:rw,uid=10001,gid=10001,mode=0700`,
        ...environmentArguments(environment),
        options.image,
      ],
      "start D-09 runtime",
    );

    const hostPort = await waitFor(
      "published runtime port",
      () =>
        inspectContainer(
          runtime,
          '{{(index (index .NetworkSettings.Ports "9090/tcp") 0).HostPort}}',
        ),
      (value) => /^\d+$/.test(value),
    );
    const baseUrl = `http://127.0.0.1:${hostPort}`;
    const ready = await waitFor(
      "runtime readiness",
      () => fetchJson(`${baseUrl}/readyz`),
      (value) => value.status === "ready",
    );
    assert.equal(ready.githubCapability.runtime.required, true);
    assert.equal(ready.githubCapability.runtime.installed, false);
    assert.equal(ready.githubCapability.name, "github_capability_v2");
    assert.equal(ready.githubCapability.version, 2);
    assert.equal(ready.runtimeGatewayProbe.name, "runtime_gateway_probe_v2");
    assert.equal(ready.runtimeGatewayProbe.version, 2);
    assert.equal(ready.runtimeGatewayProbe.supported, true);
    assert.deepEqual(ready.workspaceCheckpoint, { version: 5 });

    campaignStage = "listener-inventory";
    const containerAddress = await inspectContainer(
      runtime,
      `{{(index .NetworkSettings.Networks "${network}").IPAddress}}`,
    );
    assert.match(containerAddress, /^\d+\.\d+\.\d+\.\d+$/);
    const listenerResult = await docker(
      ["exec", runtime, "node", "-e", LISTENER_INVENTORY],
      "read runtime listener inventory",
    );
    const listeners = JSON.parse(listenerResult.stdout);
    const expectedListeners = [
      `${containerAddress}:9090`,
      "127.0.0.1:9091",
      "127.0.0.2:9090",
      "127.0.0.2:9091",
      "127.0.0.2:9092",
    ].sort();
    assert.deepEqual(listeners, expectedListeners);
    assert(!listeners.includes("0.0.0.0:9090"));

    campaignStage = "permission-profile";
    const configResult = await docker(
      [
        "exec",
        runtime,
        "node",
        "-e",
        'process.stdout.write(require("node:fs").readFileSync("/home/chrono/.codex/config.toml", "utf8"))',
      ],
      "read installed Trigger permission profile",
    );
    assert.equal(configResult.stdout, PERMISSION_CONFIG);

    campaignStage = "workspace-restore";
    const workspace = workspaceFixture();
    await restoreWorkspace(baseUrl, workspace);

    // The runtime protocol deliberately collapses command-sandbox diagnostics
    // to one closed outcome. Run one release-only preflight first so CI can
    // distinguish a runner namespace failure from a policy failure without
    // ever surfacing a provider response or credential.
    campaignStage = "sandbox-preflight";
    const sandboxPreflight = await sandboxObservation(
      runtime,
      "http://127.0.0.1:9092/livez",
    );
    requireManagedDenial(sandboxPreflight);

    campaignStage = "missing-policy";
    const outcomes = {};
    outcomes.missingPolicy = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID,
      "policy_denied",
    );
    assert.equal((await fixtureState(fixture)).githubCalls, 0);

    campaignStage = "capability-installation";
    control = await openControlSocket(
      `ws://127.0.0.1:${hostPort}/runtime/${RUNTIME_AUDIENCE}/terminal?wire=1`,
    );

    campaignStage = "workspace-writer-fence";
    workspaceFence = await proveWorkspaceWriterFence(
      control,
      runtime,
      baseUrl,
      workspace,
    );

    campaignStage = "capability-installation";
    let policyGeneration = await installCapability(control, signing, 2);
    const stale = await control.request(
      {
        t: "github_capability",
        operation_id: "install-stale-policy-1",
        envelope: signing.envelope(1, 1),
      },
      "github_capability_result",
    );
    assert.equal(stale.accepted, false);
    assert.equal(stale.refusal, "stale_generation");
    outcomes.wrongGeneration = stale.refusal;

    const credential = await control.request(
      {
        t: "credential",
        operation_id: "install-d09-credential",
        value: FIXTURE_CREDENTIAL,
      },
      "credential_result",
    );
    assert.equal(credential.installed, true);

    campaignStage = "traversal-refusal";
    const traversalTargets = [
      {
        form: "literal-dot-dot",
        target:
          "/api/v1/proxy/catalog-service-github/../chrono-llm/models?_nyxid_via=service-1",
      },
      {
        form: "encoded-dot-dot",
        target:
          "/api/v1/proxy/catalog-service-github/%2e%2e/chrono-llm/models?_nyxid_via=service-1",
      },
      {
        form: "double-encoded-dot-dot",
        target:
          "/api/v1/proxy/catalog-service-github/%252e%252e/chrono-llm/models?_nyxid_via=service-1",
      },
      {
        form: "mixed-dot-dot",
        target:
          "/api/v1/proxy/catalog-service-github/.%2e/chrono-llm/models?_nyxid_via=service-1",
      },
      {
        form: "encoded-slash",
        target:
          "/api/v1/proxy/catalog-service-github/repositories%2f30?_nyxid_via=service-1",
      },
      {
        form: "backslash",
        target:
          "/api/v1/proxy/catalog-service-github/..\\chrono-llm/models?_nyxid_via=service-1",
      },
      {
        form: "canonical-github-slug-route",
        target:
          "/api/v1/proxy/s/api-github/repositories/30?_nyxid_via=service-1",
      },
      {
        form: "suffixed-github-slug-route",
        target:
          "/api/v1/proxy/s/api-github-2/repositories/30?_nyxid_via=service-1",
      },
      {
        form: "pat-lookalike-slug-route",
        target:
          "/api/v1/proxy/s/api-github-pat/repositories/30?_nyxid_via=service-1",
      },
      {
        form: "user-service-id-as-slug-route",
        target: `/api/v1/proxy/s/${SERVICE_ID}/repositories/30?_nyxid_via=${SERVICE_ID}`,
      },
      {
        form: "catalog-service-id-as-slug-route",
        target: `/api/v1/proxy/s/${CATALOG_SERVICE_ID}/repositories/30?_nyxid_via=${SERVICE_ID}`,
      },
    ];
    const traversal = [];
    for (const { form, target } of traversalTargets) {
      const response = await rawRequest(
        runtime,
        "GET",
        "127.0.0.2",
        9092,
        target,
        { sandboxed: true, hostHeader: "127.0.0.2:9092" },
      );
      assert.equal(response.status, 403);
      assert.equal(response.networkDenied, false);
      assert.equal(response.gatewayOutcome, POLICY_DENIED);
      traversal.push({
        form,
        status: response.status,
        outcome: response.gatewayOutcome,
        bodyBytes: response.bodyBytes,
        bodySha256: response.bodySha256,
      });
    }
    assert.deepEqual(await fixtureState(fixture), {
      githubCalls: 0,
      modelCalls: 0,
      connectCalls: 0,
      unexpectedCalls: 0,
      authorizationFailures: 0,
      probeHeaderLeaks: 0,
    });

    campaignStage = "provider-outcomes";
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.allowed = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID,
      "allowed",
    );
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.providerDenied = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID,
      "provider_denied",
    );
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.brokerTransient = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID,
      "transient_broker_failure",
    );
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.wrongService = await gatewayProbe(
      baseUrl,
      "service-other",
      FORK_REPOSITORY_ID,
      "policy_denied",
    );
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.wrongCatalogService = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID,
      "policy_denied",
      "catalog-service-other",
    );
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.wrongUserServiceSlug = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID,
      "policy_denied",
      CATALOG_SERVICE_ID,
      "api-github-3",
    );
    policyGeneration = await installCapability(
      control,
      signing,
      policyGeneration + 1,
    );
    outcomes.wrongResource = await gatewayProbe(
      baseUrl,
      SERVICE_ID,
      FORK_REPOSITORY_ID + 1,
      "policy_denied",
    );

    campaignStage = "network-denials";
    const managedDenials = [
      {
        target: "127.0.0.1",
        status: sandboxPreflight.status,
        marker: sandboxPreflight.managedDenial,
        bodyBytes: sandboxPreflight.bodyBytes,
        bodySha256: sandboxPreflight.bodySha256,
      },
    ];
    for (const [target, url] of [
      ["localhost", "http://localhost:9092/livez"],
      ["public-destination", "http://example.com/"],
      ["pod-address", `http://${containerAddress}:9090/readyz`],
    ]) {
      const observation = await sandboxObservation(runtime, url);
      requireManagedDenial(observation);
      managedDenials.push({
        target,
        status: observation.status,
        marker: observation.managedDenial,
        bodyBytes: observation.bodyBytes,
        bodySha256: observation.bodySha256,
      });
    }
    const isolationDenials = [];
    for (const [target, url] of [
      ["supervisor-front", "http://127.0.0.2:9090/readyz"],
      ["model-front", "http://127.0.0.2:9091/v1/models"],
    ]) {
      const observation = await sandboxObservation(runtime, url);
      requireIsolationDenial(observation);
      isolationDenials.push({
        target,
        status: observation.status,
        marker: observation.isolationDenial,
        bodyBytes: observation.bodyBytes,
        bodySha256: observation.bodySha256,
      });
    }

    campaignStage = "connect-refusal";
    const fixtureBeforeConnect = await fixtureState(fixture);
    const directConnectCases = [
      [containerAddress, 9090, "example.com:443", "supervisor"],
      ["127.0.0.1", 9091, "example.com:443", "model-proxy"],
      ["127.0.0.2", 9092, `${fixture}:18080`, "platform-upstream"],
      ["127.0.0.2", 9092, "example.com:443", "public-destination"],
    ];
    const connect = [];
    for (const [host, port, target, name] of directConnectCases) {
      campaignStage = `connect-direct-${name}`;
      const response = await rawRequest(
        runtime,
        "CONNECT",
        host,
        port,
        target,
        {
          hostHeader: target,
        },
      );
      assert.equal(response.networkDenied, false);
      assert(response.status < 200 || response.status >= 300);
      connect.push({
        path: "direct-listener",
        target: name,
        status: response.status,
        bodyBytes: response.bodyBytes,
        bodySha256: response.bodySha256,
      });
    }
    const sandboxedConnectCases = [
      {
        host: containerAddress,
        port: 9090,
        target: `${containerAddress}:9090`,
        name: "supervisor",
        managed: true,
      },
      {
        host: "127.0.0.1",
        port: 9091,
        target: "127.0.0.1:9091",
        name: "model-proxy",
        managed: true,
      },
      {
        host: "127.0.0.2",
        port: 9092,
        target: `${fixture}:18080`,
        name: "platform-upstream",
        managed: false,
      },
      {
        host: "127.0.0.2",
        port: 9092,
        target: "example.com:443",
        name: "public-via-gateway",
        managed: false,
      },
      {
        host: "example.com",
        port: 443,
        target: "example.com:443",
        name: "public-direct",
        managed: true,
      },
      {
        host: fixture,
        port: 18080,
        target: `${fixture}:18080`,
        name: "platform-direct",
        managed: true,
      },
    ];
    for (const spec of sandboxedConnectCases) {
      campaignStage = `connect-sandbox-${spec.name}`;
      const response = await rawRequest(
        runtime,
        "CONNECT",
        spec.host,
        spec.port,
        spec.target,
        {
          sandboxed: true,
          managedProxy: true,
          tunnelTarget: spec.managed ? "" : "127.0.0.2:9092",
          hostHeader: spec.target,
        },
      );
      if (spec.managed) requireManagedDenial(response);
      else {
        assert.equal(response.networkDenied, false);
        assert(response.status < 200 || response.status >= 300);
      }
      connect.push({
        path: "cma-trigger-sandbox",
        target: spec.name,
        status: response.status,
        marker: response.managedDenial,
        bodyBytes: response.bodyBytes,
        bodySha256: response.bodySha256,
      });
    }
    assert.deepEqual(await fixtureState(fixture), fixtureBeforeConnect);

    const finalFixture = await fixtureState(fixture);
    assert.deepEqual(finalFixture, {
      githubCalls: 3,
      modelCalls: 0,
      connectCalls: 0,
      unexpectedCalls: 0,
      authorizationFailures: 0,
      probeHeaderLeaks: 0,
    });

    campaignStage = "startup-refusals";
    const startupCases = [
      {
        name: "occupied-supervisor-ingress",
        occupy: { host: "pod-address", port: 9090 },
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "runtime supervisor could not start listening",
        ],
      },
      {
        name: "occupied-model-proxy",
        occupy: { host: "127.0.0.1", port: 9091 },
        expectedExitCode: 2,
        expectedDiagnosticMarkers: ["model credential proxy could not start"],
      },
      {
        name: "occupied-supervisor-denial",
        occupy: { host: "127.0.0.2", port: 9090 },
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "Trigger loopback isolation fronts could not start",
        ],
      },
      {
        name: "occupied-model-denial",
        occupy: { host: "127.0.0.2", port: 9091 },
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "Trigger loopback isolation fronts could not start",
        ],
      },
      {
        name: "occupied-platform-gateway",
        occupy: { host: "127.0.0.2", port: 9092 },
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "platform credential proxy could not start",
        ],
      },
      {
        name: "loopback-supervisor-ingress",
        set: { CHRONO_RUNTIME_BIND: "127.0.0.1:9090" },
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "runtime supervisor bind is not safe for Trigger isolation",
        ],
      },
      {
        name: "missing-model-upstream",
        remove: ["CHRONO_UPSTREAM_BASE_URL"],
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "a Trigger runtime upstream is missing",
          'upstream="model"',
        ],
      },
      {
        name: "missing-platform-upstream",
        remove: ["CHRONO_PLATFORM_BASE_URL"],
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "a Trigger runtime upstream is missing",
          'upstream="platform"',
        ],
      },
      {
        name: "missing-gateway-command",
        remove: ["CHRONO_RUNTIME_GATEWAY_PROBE_COMMAND"],
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "a required GitHub capability must also configure the runtime gateway probe",
        ],
      },
      {
        name: "missing-policy-keyring",
        remove: ["CHRONO_GITHUB_CAPABILITY_VERIFY_KEYS"],
        expectedExitCode: 2,
        expectedDiagnosticMarkers: [
          "GitHub capability configuration is not usable",
        ],
      },
    ];
    const startupRefusals = [];
    for (const spec of startupCases) {
      startupRefusals.push(
        await startupRefusal(options.image, network, environment, spec),
      );
    }

    campaignStage = "evidence-assembly";
    evidence = {
      schema: EVIDENCE_SCHEMA,
      evidenceClass: "D",
      result: "pass",
      startedAt,
      recordedAt: new Date().toISOString(),
      buildCommit:
        inspected.Config?.Labels?.["org.opencontainers.image.revision"] ??
        "local-unpinned",
      image: imageEvidence(options, inspected),
      artifacts: facts,
      permissionProfile: {
        name: "cma-trigger",
        approvalPolicy: "never",
        sha256: digest(PERMISSION_CONFIG),
        allowedDomains: ["127.0.0.2"],
        socks5: false,
        upstreamProxy: false,
        localBinding: false,
      },
      listeners,
      outcomes,
      capability: {
        firstGeneration: 2,
        lastGeneration: policyGeneration,
        renewedBeforeEveryProviderProbe: true,
      },
      managedDenials,
      isolationDenials,
      traversal,
      connect,
      startupRefusals,
      workspaceFence,
      providerFixture: finalFixture,
      cleanup: { status: "pending", failureCodes: [] },
      redaction: REDACTION_STATEMENT,
    };
  } catch (error) {
    campaignError = campaignErrorFrom(error, campaignStage);
  }

  const cleanupFailures = await cleanupCampaign(
    control,
    createdContainers,
    network,
    networkRequested,
    resourceLabel,
  );
  if (campaignError || cleanupFailures.length > 0) {
    const failure = failureFrom(
      campaignError ?? new CampaignFailure("cleanup_failed"),
    );
    const emitted = await emitEvidence(
      options.output,
      failureEvidence(
        options,
        inspected,
        facts,
        startedAt,
        failure,
        cleanupFailures,
      ),
    );
    throw new CampaignFailure(
      emitted.failure?.code ?? "campaign_failed",
      emitted.result,
    );
  }

  evidence.cleanup = { status: "verified", failureCodes: [] };
  evidence.recordedAt = new Date().toISOString();
  const emitted = await emitEvidence(options.output, evidence);
  if (emitted.result !== "pass") {
    throw new CampaignFailure(
      emitted.failure?.code ?? "evidence_redaction_guard",
    );
  }
}

main().catch((error) => {
  const failure = failureFrom(error);
  console.error(`CMA Trigger D-09 ${failure.result}: ${failure.code}`);
  process.exitCode = 1;
});
