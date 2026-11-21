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
    () =>
      persistConversations(
        readFileSync(process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath, "utf8"),
      ),
    /Unexpected Codex ACP source/,
  );
});

function startBridge(directory) {
  const child = spawn(
    process.env.CMA_ACP_TEST_BRIDGE ? process.execPath : "codex-acp",
    process.env.CMA_ACP_TEST_BRIDGE ? [process.env.CMA_ACP_TEST_BRIDGE] : [],
    {
      cwd: directory,
      env: {
        ...process.env,
        CODEX_HOME: directory,
        CODEX_PATH: process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex",
      },
      detached: true,
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  const closed = new Promise((resolve) => child.once("close", resolve));
  const signalGroup = (signal) => {
    try {
      process.kill(-child.pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  };
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
      const frame = await this.frame(method, params);
      assert.equal(frame.error, undefined, JSON.stringify(frame.error));
      return frame.result;
    },
    async frame(method, params) {
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
        return frame;
      } finally {
        clearTimeout(timer);
        pending.delete(id);
      }
    },
    async close() {
      lines.close();
      signalGroup("SIGTERM");
      const force = setTimeout(() => signalGroup("SIGKILL"), 1500);
      try {
        await closed;
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
      const repeated = await bridge.frame("initialize", {
        protocolVersion: 1,
        clientCapabilities: {},
      });
      assert.deepEqual(repeated.error, {
        code: -32603,
        message: "Internal error",
        data: { details: "Already initialized" },
      });
    };
    try {
      await connect();
      const session = await bridge.call("session/new", {
        cwd: directory,
        mcpServers: [],
      });
      assert.equal(typeof session.sessionId, "string");
      const sessions = join(directory, "sessions");
      const files = readdirSync(sessions, { recursive: true }).filter((path) =>
        /\.jsonl$/.test(path),
      );
      assert.equal(files.length, 1);
      const entries = readFileSync(join(sessions, files[0]), "utf8")
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
      const resumed = readFileSync(join(sessions, files[0]), "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      assert.equal(resumed[0].payload.id, session.sessionId);
      assert(
        resumed.every(
          (entry) =>
            entry.type !== "response_item" && (entry.type !== "event_msg" ||
              // CLI 0.165 persists settings on resume without creating a turn.
              (entry.payload.type === "thread_settings_applied" && entry.payload.thread_id === session.sessionId)),
        ),
      );
    } finally {
      await bridge?.close();
      // Native descendants can finish their home writes after the bridge exits.
      rmSync(directory, { recursive: true, maxRetries: 10, retryDelay: 100 });
    }
  },
);
