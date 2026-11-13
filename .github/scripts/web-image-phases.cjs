// Test-only preload, inherited by Playwright's forked workers. Private hooks are
// pinned to 1.58.2; fail on upgrades instead of silently losing phase evidence.
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const { errorMonitor } = require("node:events");
const localRequire = Module.createRequire(
  path.resolve(__dirname, "../../web/package.json"),
);
if (localRequire("playwright/package.json").version !== "1.58.2") {
  throw new Error(
    "Revalidate image phase instrumentation for this Playwright version",
  );
}
const directory = process.env.CMA_WEB_IMAGE_EVIDENCE_DIR;
if (!directory)
  throw new Error("Image phase instrumentation requires an evidence directory");
let ordinal = 0;
function begin(phase) {
  const id = ++ordinal;
  const start = performance.now();
  function record(event, fields = {}) {
    fs.mkdirSync(directory, { recursive: true });
    fs.appendFileSync(
      path.join(directory, `browser-phases-${process.pid}.jsonl`),
      JSON.stringify({
        event,
        phase,
        pid: process.pid,
        id,
        at: new Date().toISOString(),
        ...fields,
      }) + "\n",
    );
  }
  record("phase-begin");
  return (outcome) =>
    record("phase-end", { outcome, durationMs: performance.now() - start });
}

function wrap(prototype, method, phase) {
  const original = prototype?.[method];
  if (typeof original !== "function")
    throw new Error(`Missing image phase hook: ${phase}`);
  prototype[method] = async function (...args) {
    const end = begin(phase);
    try {
      const result = await original.apply(this, args);
      end("completed");
      return result;
    } catch (error) {
      end("failed");
      throw error;
    }
  };
}

const hooks = new Map(
  [
    [
      "playwright/lib/reporters/internalReporter.js",
      (m) => wrap(m.InternalReporter.prototype, "onEnd", "reporter-onEnd"),
    ],
    [
      "playwright/lib/runner/processHost.js",
      (m) => wrap(m.ProcessHost.prototype, "stop", "worker-host-stop"),
    ],
    [
      "playwright/lib/worker/workerMain.js",
      (m) => wrap(m.WorkerMain.prototype, "gracefullyClose", "worker-teardown"),
    ],
    [
      "playwright/lib/worker/testTracing.js",
      (m) =>
        wrap(m.TestTracing.prototype, "stopIfNeeded", "trace-finalization"),
    ],
    [
      "playwright-core/lib/server/browser.js",
      (m) => wrap(m.Browser.prototype, "close", "browser-close"),
    ],
    [
      "playwright-core/lib/zipBundle.js",
      (m) => {
        const original = m.yazl.ZipFile.prototype.end;
        if (typeof original !== "function")
          throw new Error("Missing image ZIP phase hook");
        m.yazl.ZipFile.prototype.end = function (...args) {
          const end = begin("zip-finalization");
          let finished = false;
          const finish = (outcome) => {
            if (!finished) {
              finished = true;
              end(outcome);
            }
          };
          // Measures ZIP encoding/output delivery, not filesystem flush. The enclosing
          // trace-finalization phase includes trace file close and merge completion.
          this.outputStream.once("end", () => finish("completed"));
          this.outputStream.once(errorMonitor, () => finish("failed"));
          try {
            return original.apply(this, args);
          } catch (error) {
            finish("failed");
            throw error;
          }
        };
      },
    ],
  ].map(([suffix, hook]) => [
    path.resolve(
      path.dirname(
        localRequire.resolve(
          suffix.startsWith("playwright-core/")
            ? "playwright-core/package.json"
            : "playwright/package.json",
        ),
      ),
      suffix.slice(suffix.indexOf("/") + 1),
    ),
    hook,
  ]),
);

const load = Module._load;
Module._load = function (request, parent, isMain) {
  const filename = Module._resolveFilename(request, parent, isMain);
  const result = load.apply(this, arguments);
  const hook = hooks.get(filename);
  if (hook) {
    hooks.delete(filename);
    hook(result);
  }
  return result;
};
