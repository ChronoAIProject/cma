// Test-only transparent transport to the actual pinned native app-server.
// Control requests go to that same process; no provider reply/event is invented.
const { writeFileSync } = require("node:fs");
const { createConnection } = require("node:net");
const { join } = require("node:path");

function controlledBinary(directory, binary) {
  const path = join(directory, "native-control.cjs");
  const socket = join(directory, "native-control.sock");
  writeFileSync(path, `#!${process.execPath}
const { spawn } = require("node:child_process");
const { createServer } = require("node:net");
const { createInterface } = require("node:readline");
const { rmSync, writeFileSync } = require("node:fs");
const socket = ${JSON.stringify(socket)};
rmSync(socket, { force: true });
const child = spawn(${JSON.stringify(binary)}, process.argv.slice(2), { stdio: ["pipe", "pipe", "inherit"] });
writeFileSync(${JSON.stringify(join(directory, "native-control.pid"))}, String(child.pid));
const pending = new Map(); let serial = 0;
process.stdin.pipe(child.stdin);
createInterface({ input: child.stdout }).on("line", line => {
  const frame = JSON.parse(line), client = pending.get(frame.id);
  if (client) { pending.delete(frame.id); client.end(line + "\\n"); }
  else process.stdout.write(line + "\\n");
});
const server = createServer(client => {
  createInterface({ input: client }).once("line", line => {
    const request = JSON.parse(line), id = "native-fixture-" + ++serial;
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
