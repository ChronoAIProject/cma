const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { createInterface } = require("node:readline");
const { test } = require("node:test");
const {
  bridgePath,
  persistConversations,
} = require("./persist_conversations.cjs");

test("persistence patch refuses unexpected or already patched bridge bytes", () => {
  assert.throws(
    () => persistConversations("unreviewed bridge"),
    /Unexpected Codex ACP source/,
  );
  assert.throws(
    () => persistConversations(readFileSync(bridgePath, "utf8")),
    /Unexpected Codex ACP source/,
  );
});

function startBridge(directory) {
  const child = spawn("codex-acp", [], {
    cwd: directory,
    env: {
      ...process.env,
      CODEX_HOME: directory,
      CODEX_PATH: "/usr/local/bin/codex",
    },
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stderr.resume();
  const pending = new Map();
  let sequence = 0;
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const frame = JSON.parse(line);
    if (pending.has(frame.id)) pending.get(frame.id)(frame);
  });
  return {
    async call(method, params) {
      const id = ++sequence;
      const result = new Promise((resolve) => pending.set(id, resolve));
      child.stdin.write(
        JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n",
      );
      let timer;
      try {
        const frame = await Promise.race([
          result,
          new Promise((_, reject) => {
            timer = setTimeout(
              () => reject(new Error("Bridge deadline: " + method)),
              20000,
            );
          }),
        ]);
        assert.equal(frame.error, undefined, JSON.stringify(frame.error));
        return frame.result;
      } finally {
        clearTimeout(timer);
        pending.delete(id);
      }
    },
    async close() {
      lines.close();
      if (child.exitCode !== null) return;
      const exited = new Promise((resolve) => child.once("exit", resolve));
      process.kill(-child.pid, "SIGTERM");
      const force = setTimeout(() => {
        try {
          process.kill(-child.pid, "SIGKILL");
        } catch {}
      }, 1500);
      try {
        await exited;
      } finally {
        clearTimeout(force);
      }
    },
  };
}

test(
  "untouched Agent persists zero turns and resumes the same conversation after restart",
  { timeout: 60000 },
  async () => {
    const directory = mkdtempSync(join(tmpdir(), "cma-empty-conversation-"));
    writeFileSync(
      join(directory, "config.toml"),
      [
        'model = "gpt-5.6-sol"',
        'model_provider = "cma_test"',
        'web_search = "disabled"',
        "[model_providers.cma_test]",
        'name = "CMA Offline Test"',
        'base_url = "http://127.0.0.1:9/v1"',
        'wire_api = "responses"',
        "requires_openai_auth = false",
      ].join("\n"),
    );
    let bridge;
    const connect = async () => {
      bridge = startBridge(directory);
      await bridge.call("initialize", {
        protocolVersion: 1,
        clientCapabilities: {},
      });
    };
    try {
      await connect();
      const session = await bridge.call("session/new", {
        cwd: directory,
        mcpServers: [],
      });
      assert.equal(typeof session.sessionId, "string");
      const files = readdirSync(directory, { recursive: true }).filter((path) =>
        /^sessions\/.*\.jsonl$/.test(path),
      );
      assert.equal(files.length, 1);
      const entries = readFileSync(join(directory, files[0]), "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      assert.deepEqual(
        entries.map((entry) => entry.type),
        ["session_meta"],
      );
      assert.equal(entries[0].payload.id, session.sessionId);
      await bridge.close();
      await connect();
      const loaded = await bridge.call("session/load", {
        sessionId: session.sessionId,
        cwd: directory,
        mcpServers: [],
      });
      assert(loaded);
      const resumed = readFileSync(join(directory, files[0]), "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      assert.equal(resumed[0].payload.id, session.sessionId);
      assert(
        resumed.every(
          (entry) =>
            entry.type !== "response_item" && entry.type !== "event_msg",
        ),
      );
    } finally {
      await bridge?.close();
      rmSync(directory, { recursive: true });
    }
  },
);
