import { randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { scan, requireThat } from "./evidence.mjs";
import { coverage } from "./coverage.mjs";
import { fixture } from "./fixtures.mjs";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const write = (path, value) => writeFileSync(path, value, { mode: 0o600, flag: "wx" });

function cargo(args, env, log) {
  return new Promise((done) => {
    write(log, "");
    const child = spawn("cargo", args, { cwd: join(root, "api"), env,
      stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = "";
    child.stdout.on("data", (bytes) => { stdout += bytes; appendFileSync(log, bytes); });
    child.stderr.on("data", (bytes) => { stderr += bytes; appendFileSync(log, bytes); });
    child.on("error", () => done({ code: null, signal: null, stdout, stderr: "cargo_spawn_failed" }));
    child.on("close", (code, signal) => done({ code, signal, stdout, stderr }));
  });
}

export async function runHarness(output, run = cargo) {
  output = resolve(output);
  // A new private directory per attempt; never overwrite a prior failed run.
  mkdirSync(output, { mode: 0o700 });
  const records = join(output, "encrypted-records");
  mkdirSync(records, { mode: 0o700 });
  const keys = Array.from({ length: 3 }, () => randomBytes(32));
  const encoded = keys.map((key) => key.toString("base64"));
  const sentinel = `custody-${randomBytes(32).toString("hex")}`;
  const sentinels = [sentinel, Buffer.from(sentinel).toString("base64"),
    Buffer.from(sentinel).toString("hex"), ...keys, ...encoded,
    ...keys.flatMap((key) => [key.toString("hex"), key.toString("base64url")])];
  const start = new Date().toISOString();
  const sha = spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" });
  const dirty = spawnSync("git", ["status", "--porcelain"], { cwd: root, encoding: "utf8" });
  const provenance = { source_sha: sha.status === 0 ? sha.stdout.trim() : null,
    dirty_tree: dirty.status === 0 ? dirty.stdout.length > 0 : null,
    image_digests: null };
  // If the parent is cancelled, retain the start receipt and streamed logs.
  write(join(output, "started.json"), JSON.stringify({ mode: "synthetic", start,
    ...provenance, qualification_status: "NOT_RUN" }));
  const attempts = [];
  for (const phase of ["write", "read"]) {
    for (const [name, args] of [
      ["auth", ["test", "--locked", "-p", "cma", "--bin", "cma",
        "store::encryption::restore_evidence_tests::restore_evidence_auth", "--", "--exact", "--nocapture"]],
      ["sealed", ["test", "--locked", "-p", "cmaeg-core", "--test", "restore_evidence",
        "restore_evidence_sealed", "--", "--exact", "--nocapture"]],
    ]) {
      const env = { ...process.env, HOME_BACKUP_EVIDENCE_KEYS: encoded.join(","),
        HOME_BACKUP_EVIDENCE_SENTINEL: sentinel, HOME_BACKUP_EVIDENCE_PHASE: phase,
        HOME_BACKUP_EVIDENCE_ARTIFACTS: records };
      const log = `${phase}-${name}.log`;
      const logPath = join(output, log);
      const result = await run(args, env, logPath);
      if (!existsSync(logPath)) write(logPath, `${result.stdout}\n${result.stderr}`);
      const passed = result.code === 0 && !result.signal && /test result: ok\. 1 passed;/.test(result.stdout);
      attempts.push({ phase, codec: name, exit_code: result.code, signal: result.signal,
        status: passed ? "PASS" : "FAIL", raw_result: log });
    }
  }
  const verifiedRecords = attempts.filter((a) => a.phase === "read" && a.status === "PASS")
    .reduce((count, a) => count + (a.codec === "auth" ? 6 : 2), 0);
  const report = { mode: "synthetic", environment: "isolated-local-processes", ...provenance,
    window: { start, end: new Date().toISOString() }, sample_count: verifiedRecords,
    planned_sample_count: 8,
    configuration: { auth_records: 6, sealed_records: 2, key_versions: 2, wrong_keys: 1 },
    switches: { home_restore: false, cloud_inventory: false }, attempt: 1,
    qualification_status: "NOT_RUN", home_restore_status: "NOT_RUN", full_dr_status: "NOT_RUN",
    key_tests_status: attempts.every((a) => a.status === "PASS") ? "PASS" : "FAIL",
    attempts, sentinel_scan: scan(output, sentinels) };
  report.sentinel_scan.files++; // final count includes result.json, scanned below
  if (report.sentinel_scan.matches) report.key_tests_status = "FAIL";
  write(join(output, "result.json"), `${JSON.stringify(report, null, 2)}\n`);
  // Include the final artifact in the scan as well. Report has counts only.
  const finalScan = scan(output, sentinels);
  requireThat(finalScan.matches === report.sentinel_scan.matches &&
    finalScan.files === report.sentinel_scan.files, "report_sentinel_leak");
  return report;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [mode, output] = process.argv.slice(2);
    requireThat(process.argv.length === 4 && ["--fixture", "--keys"].includes(mode), "usage_harness_mode_new_output_directory");
    if (mode === "--fixture") {
      mkdirSync(output, { mode: 0o700 });
      const report = coverage(fixture());
      write(join(output, "coverage.json"), `${JSON.stringify(report, null, 2)}\n`);
      console.log("fixture coverage: expected missing home; restore NOT_RUN");
      process.exitCode = report.status === "PASS" ? 0 : 1;
    } else {
      const report = await runHarness(output);
      console.log(`synthetic key tests: ${report.key_tests_status}; qualification NOT_RUN`);
      if (report.key_tests_status !== "PASS") process.exitCode = 1;
    }
  } catch { console.error("harness_failed"); process.exitCode = 1; }
}
