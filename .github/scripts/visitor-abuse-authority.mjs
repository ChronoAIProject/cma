import assert from "node:assert/strict";
import { lstatSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { privateFile } from "./staging-authority.mjs";
import { caseIds, evidenceHash } from "./visitor-abuse-evidence.mjs";
import { validateTC117Evidence } from "./visitor-abuse-campaign.mjs";

function readPrivate(path) {
  const stat = lstatSync(path);
  assert.ok(
    stat.size > 0 && stat.size <= 16 * 1024 * 1024,
    "Evidence size exceeded",
  );
  return privateFile(path);
}

export function inspectEvidence(directory, contextFile, now = Date.now()) {
  assert.equal(directory, realpathSync(directory));
  const stat = lstatSync(directory);
  assert.ok(
    stat.isDirectory() &&
      !stat.isSymbolicLink() &&
      stat.uid === process.getuid() &&
      (stat.mode & 0o077) === 0,
  );
  const contextBytes = readPrivate(contextFile);
  const context = JSON.parse(contextBytes.toString("utf8"));
  assert.equal(
    readPrivate(join(directory, ".validated-run")).toString("utf8"),
    context.run,
  );
  const summaryBytes = readPrivate(join(directory, "tc117-summary.json"));
  const journalBytes = readPrivate(join(directory, "cleanup.ndjson"));
  const journal = journalBytes
    .toString("utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  const artifacts = Object.fromEntries(
    caseIds.map((name) => {
      const file = `tc117-${name}.json`;
      return [file, readPrivate(join(directory, file))];
    }),
  );
  const result = validateTC117Evidence(
    JSON.parse(summaryBytes.toString("utf8")),
    artifacts,
    journal,
    context,
    now,
  );
  const files = {
    "tc117-summary.json": summaryBytes,
    "cleanup.ndjson": journalBytes,
    ...artifacts,
  };
  for (const [name, bytes] of Object.entries(files))
    assert.equal(
      evidenceHash(readPrivate(join(directory, name))),
      evidenceHash(bytes),
    );
  assert.equal(
    evidenceHash(readPrivate(contextFile)),
    evidenceHash(contextBytes),
  );
  return {
    ...result,
    evidence: Object.fromEntries(
      Object.entries(files).map(([name, bytes]) => [name, evidenceHash(bytes)]),
    ),
    contextSha256: evidenceHash(contextBytes),
    qualificationBlockers: [
      "run_authority_contract_missing",
      "independent_cleanup_acknowledgment_missing",
    ],
  };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const [mode, directory, contextFile, ...extra] = process.argv.slice(2);
  try {
    assert.equal(mode, "inspect");
    assert.ok(directory && contextFile && extra.length === 0);
    const result = inspectEvidence(directory, contextFile);
    // No local artifact, successful scenario, or caller-supplied context authorizes a campaign.
    process.stdout.write(`${JSON.stringify(result)}\n`);
    process.stderr.write(
      "BLOCKED: TC-117 execution and qualification require independent run authority and cleanup acknowledgment.\n",
    );
    process.exitCode = 2;
  } catch {
    process.stderr.write(
      "TC-117 evidence inspection refused; private inputs withheld.\n",
    );
    process.exitCode = 1;
  }
}
