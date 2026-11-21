// Build-time fixture only; production recovery keeps its native /proc proofs.
const { createHash } = require("node:crypto");
const { readFileSync, readlinkSync } = require("node:fs");
const { basename, isAbsolute, normalize } = require("node:path");

const rosetta = "/run/rosetta/rosetta";

function nativeProcessIdentity(pid, executable, expected, io = { readFileSync, readlinkSync }) {
  // Keep the native Linux predicate independent of argv and emulation pins.
  if (basename(executable) === "codex") return true;
  if (executable !== rosetta && io.readlinkSync("/proc/self/exe") !== rosetta) return false;
  if (typeof expected.path !== "string" || !isAbsolute(expected.path) ||
      normalize(expected.path) !== expected.path || basename(expected.path) !== "codex" ||
      !/^[a-f0-9]{64}$/.test(expected.sha256 ?? "")) {
    throw new Error("Rosetta fixture requires an exact native executable path and SHA-256");
  }
  const bytes = io.readFileSync(`/proc/${pid}/cmdline`);
  if (bytes.length === 0 || bytes.length > 32768 || bytes.at(-1) !== 0) return false;
  const argv = bytes.toString("utf8").slice(0, -1).split("\0");
  // Some translator versions expose their own argv prefix. It must be exact;
  // a launcher or arbitrary argument mentioning the pinned binary is no proof.
  if (argv[0] === rosetta) argv.shift();
  if (argv.length !== 2 || argv[0] !== expected.path || argv[1] !== "app-server") return false;
  return createHash("sha256").update(io.readFileSync(expected.path)).digest("hex") === expected.sha256;
}

module.exports = { nativeProcessIdentity };
