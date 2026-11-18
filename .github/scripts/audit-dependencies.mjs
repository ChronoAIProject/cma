#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { evaluateAudit } from "./audit-policy.mjs";

export function npmJson(args, cwd, run = spawnSync) {
  const result = run("npm", args, {
    cwd,
    encoding: "utf8",
    timeout: 120_000,
    maxBuffer: 16 * 1024 * 1024,
  });
  // npm audit exits 1 when it reports vulnerabilities; npm ls must succeed.
  const allowed = args[0] === "audit" ? [0, 1] : [0];
  if (result.error || result.signal || !allowed.includes(result.status)) {
    throw new Error(
      `npm ${args[0]} failed; dependency evidence is unavailable`,
    );
  }
  return JSON.parse(result.stdout);
}

export function auditWeb(root, run = spawnSync) {
  const web = resolve(root, "web");
  const exceptions = JSON.parse(
    readFileSync(resolve(web, "audit-exceptions.json"), "utf8"),
  );
  const tree = npmJson(["ls", "--omit=dev", "--all", "--json"], web, run);
  const report = npmJson(["audit", "--audit-level=high", "--json"], web, run);
  const accepted = evaluateAudit(report, tree, exceptions);
  for (const entry of accepted) {
    console.log(
      `Accepted dev-only advisory ${entry.advisory} for ${entry.package} through ${entry.expires}: ${entry.reason}`,
    );
  }
  console.log(
    `Dependency audit passed; ${accepted.length} active dev-only exception(s).`,
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    auditWeb(resolve(dirname(fileURLToPath(import.meta.url)), "../.."));
  } catch (error) {
    console.error(`Dependency audit failed: ${error.message}`);
    process.exitCode = 1;
  }
}
