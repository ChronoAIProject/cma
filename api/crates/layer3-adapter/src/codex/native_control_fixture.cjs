// Test-only transparent transport to the actual pinned native app-server.
// Native requests go to that same process. Opt-in fault injection can delay or
// remove identity from genuine fixture notifications.
const { writeFileSync } = require("node:fs");
const { createConnection } = require("node:net");
const { join } = require("node:path");

function controlledBinary(directory, binary, heldDelta = null) {
  const path = join(directory, "native-control.cjs");
  const socket = join(directory, "native-control.sock");
  writeFileSync(path, `#!${process.execPath}
const { spawn } = require("node:child_process");
const { createServer } = require("node:net");
const { createInterface } = require("node:readline");
const { rmSync, writeFileSync } = require("node:fs");
const socket = ${JSON.stringify(socket)};
const heldDelta = ${JSON.stringify(heldDelta)};
rmSync(socket, { force: true });
const child = spawn(${JSON.stringify(binary)}, process.argv.slice(2), { stdio: ["pipe", "pipe", "inherit"] });
writeFileSync(${JSON.stringify(join(directory, "native-control.pid"))}, String(child.pid));
const pending = new Map(); let serial = 0;
let held;
let deltaSample;
let startArmed = false;
let startRequestId;
let startResponse;
let earlyDelta;
let observedEarlyDelta = false;
if (heldDelta === null) process.stdin.pipe(child.stdin);
else createInterface({ input: process.stdin }).on("line", line => {
  const frame = JSON.parse(line);
  if (startArmed && frame.method === "turn/start") {
    startArmed = false;
    startRequestId = frame.id;
  }
  child.stdin.write(line + "\\n");
});
createInterface({ input: child.stdout }).on("line", line => {
  const frame = JSON.parse(line), client = pending.get(frame.id);
  if (startRequestId !== undefined && frame.id === startRequestId) {
    startResponse = line;
    return;
  }
  if (earlyDelta !== undefined && frame.method === "item/agentMessage/delta" && frame.params.delta.includes(earlyDelta)) {
    observedEarlyDelta = true;
  }
  if (heldDelta !== null && frame.method === "item/agentMessage/delta" && frame.params.delta.includes(heldDelta)) {
    if (held) throw new Error("Duplicate held native notification");
    held = { frame, line };
    deltaSample = frame;
    return;
  }
  if (client) { pending.delete(frame.id); client.end(line + "\\n"); }
  else process.stdout.write(line + "\\n");
});
const server = createServer(client => {
  createInterface({ input: client }).once("line", line => {
    const request = JSON.parse(line), id = "native-fixture-" + ++serial;
    if (heldDelta !== null && request.method === "fixture/held") {
      const source = held ? { threadId: held.frame.params.threadId, turnId: held.frame.params.turnId } : null;
      client.end(JSON.stringify({ id, result: { source } }) + "\\n");
      return;
    }
    if (heldDelta !== null && request.method === "fixture/release") {
      const saved = held;
      held = undefined;
      if (saved) process.stdout.write(saved.line + "\\n");
      client.end(JSON.stringify({ id, result: { released: Boolean(saved) } }) + "\\n");
      return;
    }
    if (heldDelta !== null && request.method === "fixture/emit-without-turn") {
      if (!deltaSample) throw new Error("No observed delta to replay without identity");
      const params = { ...deltaSample.params, delta: request.params.delta };
      delete params.turnId;
      process.stdout.write(JSON.stringify({ ...deltaSample, params }) + "\\n");
      client.end(JSON.stringify({ id, result: {} }) + "\\n");
      return;
    }
    if (heldDelta !== null && request.method === "fixture/hold-start") {
      startArmed = true;
      earlyDelta = request.params.earlyDelta;
      client.end(JSON.stringify({ id, result: {} }) + "\\n");
      return;
    }
    if (heldDelta !== null && request.method === "fixture/start-state") {
      client.end(JSON.stringify({ id, result: { responseHeld: Boolean(startResponse), observedEarlyDelta } }) + "\\n");
      return;
    }
    if (heldDelta !== null && request.method === "fixture/release-start") {
      const response = startResponse;
      startResponse = undefined;
      startRequestId = undefined;
      if (response) process.stdout.write(response + "\\n");
      client.end(JSON.stringify({ id, result: { released: Boolean(response) } }) + "\\n");
      return;
    }
    pending.set(id, client);
    child.stdin.write(JSON.stringify({ id, ...request }) + "\\n");
  });
});
server.listen(socket);
child.on("exit", code => { server.close(); process.exit(code ?? 1); });
`, { mode: 0o755 });
  return path;
}

function control(directory, method, params) {
  return new Promise((resolve, reject) => {
    const socket = createConnection(join(directory, "native-control.sock"));
    let reply = "";
    const timer = setTimeout(() => { socket.destroy(); reject(new Error("Native control deadline: " + method)); }, 15000);
    socket.on("connect", () => socket.write(JSON.stringify({ method, params }) + "\n"));
    socket.on("data", chunk => { reply += chunk; });
    socket.on("error", reject);
    socket.on("close", () => clearTimeout(timer));
    socket.on("end", () => {
      try {
        const frame = JSON.parse(reply);
        if (frame.error) reject(new Error(JSON.stringify(frame.error)));
        else resolve(frame.result);
      } catch (error) { reject(error); }
    });
  });
}
module.exports = { controlledBinary, control };
