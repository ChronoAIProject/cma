import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { inspectEvidence } from "./visitor-abuse-authority.mjs";
import { campaignFixture } from "./visitor-abuse-campaign.fixture.mjs";

function harness(t) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "tc117-inspect-")));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const f = campaignFixture();
  const write = (file, value) =>
    writeFileSync(
      join(directory, file),
      typeof value === "string" || Buffer.isBuffer(value)
        ? value
        : JSON.stringify(value),
      { mode: 0o600 },
    );
  const writeFixture = () => {
    write("context.json", f.context);
    write(".validated-run", f.context.run);
    write("tc117-summary.json", f.summary);
    write(
      "cleanup.ndjson",
      f.journal.map((row) => JSON.stringify(row)).join("\n"),
    );
    for (const [name, bytes] of Object.entries(f.artifacts)) write(name, bytes);
  };
  writeFixture();
  const inspect = () =>
    inspectEvidence(directory, join(directory, "context.json"), f.now);
  return { directory, f, write, writeFixture, inspect };
}
test("private inspection binds exact input bytes and retains qualification blockers", (t) => {
  const h = harness(t);
  const result = h.inspect();
  assert.equal(result.fullCampaign, false);
  assert.equal(result.provenanceVerified, false);
  assert.equal(Object.keys(result.evidence).length, 6);
  assert.equal(result.qualificationBlockers.length, 2);
  assert.ok(!JSON.stringify(result).includes(h.f.context.origin));
  assert.ok(!JSON.stringify(result).includes(h.f.context.subjects.a));
});
for (const [name, mutate] of [
  [
    "world-readable context",
    (h) => chmodSync(join(h.directory, "context.json"), 0o644),
  ],
  [
    "world-readable artifact",
    (h) => chmodSync(join(h.directory, "tc117-utc.json"), 0o644),
  ],
  ["world-readable directory", (h) => chmodSync(h.directory, 0o755)],
  ["wrong run marker", (h) => h.write(".validated-run", "other-run")],
  [
    "symlink artifact",
    (h) => {
      rmSync(join(h.directory, "tc117-utc.json"));
      symlinkSync(
        join(h.directory, "tc117-shared.json"),
        join(h.directory, "tc117-utc.json"),
      );
    },
  ],
  ["truncated journal", (h) => h.write("cleanup.ndjson", "{")],
  [
    "oversized artifact",
    (h) => h.write("tc117-utc.json", " ".repeat(16 * 1024 * 1024 + 1)),
  ],
  ["tampered artifact", (h) => h.write("tc117-utc.json", "{}")],
])
  test(`inspection refuses ${name}`, (t) => {
    const h = harness(t);
    mutate(h);
    assert.throws(h.inspect);
  });
test("CLI refuses malformed private evidence without emitting private values", (t) => {
  const h = harness(t);
  h.write("context.json", "PRIVATE_TC117_INPUT_CANARY");
  const output = spawnSync(
    process.execPath,
    [
      resolve(".github/scripts/visitor-abuse-authority.mjs"),
      "inspect",
      h.directory,
      join(h.directory, "context.json"),
    ],
    { encoding: "utf8" },
  );
  assert.equal(output.status, 1);
  assert.equal(output.stdout, "");
  assert.equal(
    output.stderr,
    "TC-117 evidence inspection refused; private inputs withheld.\n",
  );
  assert.equal(
    readFileSync(join(h.directory, "context.json"), "utf8"),
    "PRIVATE_TC117_INPUT_CANARY",
  );
});

test("CLI reports consistent inputs but cannot return campaign success or accept a self-issued grant", (t) => {
  const h = harness(t);
  h.f.context.grant = { authorized: true, fullCampaign: true };
  h.writeFixture();
  const output = spawnSync(
    process.execPath,
    [
      "--import",
      `data:text/javascript,Date.now%20%3D%20()%20%3D%3E%20${h.f.now}`,
      resolve(".github/scripts/visitor-abuse-authority.mjs"),
      "inspect",
      h.directory,
      join(h.directory, "context.json"),
    ],
    { encoding: "utf8", env: {} },
  );
  assert.equal(output.status, 2);
  const result = JSON.parse(output.stdout);
  assert.equal(result.status, "consistent");
  assert.equal(result.fullCampaign, false);
  assert.equal(result.provenanceVerified, false);
  assert.match(output.stderr, /^BLOCKED: TC-117/);
  assert.ok(!output.stdout.includes(h.f.context.origin));
});
