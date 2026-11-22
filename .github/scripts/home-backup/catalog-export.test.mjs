import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { catalogFixture, encodeCatalogFixture, fixture } from "./fixtures.mjs";
import { loadCatalogExport, validateCatalogExport } from "./catalog-export.mjs";
import { coverage } from "./coverage.mjs";

function retained(payload, run) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "synthetic-catalog-")));
  chmodSync(root, 0o700);
  const paths = { artifact_path: join(root, "artifact.json"), receipt_path: join(root, "receipt.json") };
  const { bytes, receipt } = encodeCatalogFixture(payload);
  writeFileSync(paths.artifact_path, bytes, { mode: 0o600 });
  writeFileSync(paths.receipt_path, JSON.stringify(receipt), { mode: 0o600 });
  try { return run(paths, root); } finally { rmSync(root, { recursive: true }); }
}
function input(paths) {
  const { sandboxes, home_backups, ...remaining } = fixture();
  return { ...remaining, catalog_export: paths };
}

test("complete synthetic export keeps the denominator and exposes missing schedules separately", () => {
  retained(catalogFixture(), paths => {
    const report = coverage(input(paths));
    assert.equal(report.mode, "fixture");
    assert.equal(report.retained_homes, 2);
    assert.equal(report.backed_homes, 1);
    assert.equal(report.status, "FAIL");
    assert.equal(report.catalog_export_status, "AVAILABLE");
    assert.equal(report.catalog_provenance_status, "RECEIPT_BOUND_UNVERIFIED");
    assert.equal(report.homes[1].schedule_status, "MISSING");
    assert.deepEqual(report.homes[1].reasons, ["schedule_missing", "catalog_missing"]);
    for (const field of ["home_restore_status", "full_dr_status", "historical_cadence_status", "qualification_status", "cmek_recovery_status"])
      assert.equal(report[field], "NOT_RUN");
    assert.throws(() => coverage({ ...input(paths), sandboxes: [] }), /inventory_override/);
  });
});

test("matching inventory passes coverage while provenance and disaster recovery remain unqualified", () => {
  const payload = catalogFixture();
  payload.collections.sandboxes.rows.splice(1, 1); payload.collections.sandboxes.count--;
  retained(payload, paths => {
    const report = coverage(input(paths));
    assert.equal(report.status, "PASS");
    assert.equal(report.catalog_provenance_status, "RECEIPT_BOUND_UNVERIFIED");
    assert.equal(report.qualification_status, "NOT_RUN");
  });
});

test("schedule source or referenced backup mismatch fails coverage without denying export completeness", () => {
  for (const mutate of [s => { s.home.home_source_id = "different-home"; },
    s => { s.last_ready_backup_id = "hb-a-999"; }]) {
    const payload = catalogFixture(); mutate(payload.collections.home_backup_schedules.rows[0]);
    retained(payload, paths => {
      const report = coverage(input(paths));
      assert.equal(report.catalog_export_status, "AVAILABLE");
      assert.equal(report.backed_homes, 0);
      assert.match(report.homes[0].schedule_status, /MISMATCHED/);
    });
  }
});

test("empty inventory remains unmeasured and a stale complete export is refused", () => {
  const empty = catalogFixture();
  for (const entry of Object.values(empty.collections)) { entry.rows = []; entry.count = 0; }
  retained(empty, paths => assert.equal(coverage(input(paths)).status, "NOT_RUN"));
  retained(catalogFixture(), paths => {
    const value = input(paths); value.now = "2026-11-16T01:06:00Z";
    assert.throws(() => coverage(value), /catalog_export_stale/);
  });
});

const mutations = [
  p => { p.format = "future"; },
  p => { p.provenance = "authenticated"; },
  p => { p.snapshot.time = 0; },
  p => { delete p.snapshot.session_sha256; },
  p => { p.collections.sandboxes.eof = false; },
  p => { p.collections.sandboxes.count--; },
  p => { p.collections.sandboxes.rows.reverse(); },
  p => { p.collections.sandboxes.rows[1].id = "a"; },
  p => { p.collections.sandboxes.rows[0].unrelated_secret = "payload-sentinel"; },
  p => { p.collections.home_backups.rows[0].next_fence = 2 ** 53; },
  p => { p.collections.home_backups.rows[0].source.extra = "payload-sentinel"; },
  p => { p.collections.home_backups.rows[0].cut.requested_at = "2026-11-16T08:10:00+08:00"; },
  p => { p.collections.home_backups.rows[0].cut.requested_at = "2026-02-30T00:00:00Z"; },
  p => { p.collections.home_backups.rows[0].authority.token = "lease-sentinel"; },
  p => { p.collections.home_backups.rows[0].authority.validation = "unverified"; },
  p => { p.collections.home_backups.rows[0].state = "deleted"; },
  p => { p.collections.home_backup_schedules.rows[0].in_flight = { backup_id: "x" }; },
];
test("malformed completeness, provenance, identity, numeric and projection fields fail closed even with fresh digests", () => {
  for (const mutate of mutations) {
    const payload = catalogFixture(); mutate(payload);
    const { bytes, receipt } = encodeCatalogFixture(payload);
    assert.throws(() => validateCatalogExport(bytes, receipt), error => {
      assert.doesNotMatch(error.message, /payload-sentinel|lease-sentinel/); return true;
    });
  }
  const payload = catalogFixture(); payload.collections.home_backups.rows[0].next_fence = 2 ** 53 - 1;
  const { bytes, receipt } = encodeCatalogFixture(payload);
  assert.equal(validateCatalogExport(bytes, receipt).home_backups[0].next_fence, 2 ** 53 - 1);
});

test("truncated output, modified payload, mismatched receipt and executable identity cannot be accepted", () => {
  const { bytes, receipt } = encodeCatalogFixture();
  assert.throws(() => validateCatalogExport(bytes.slice(0, -1), receipt));
  assert.throws(() => validateCatalogExport("malformed-secret-sentinel", receipt), error => {
    assert.equal(error.message, "catalog_export_invalid_json"); return true;
  });
  assert.throws(() => validateCatalogExport(bytes.replace("uid-a", "changed-a"), receipt));
  for (const field of ["artifact_sha256", "payload_sha256", "run_sha256", "source_revision_claim", "executable_sha256"])
    assert.throws(() => validateCatalogExport(bytes, { ...receipt, [field]: "f".repeat(64) }), /receipt_mismatch/);
  assert.throws(() => validateCatalogExport(bytes, null));
});

test("catalog loader rejects symlinks, public paths and a missing separately retained receipt", () => {
  retained(catalogFixture(), (paths, root) => {
    assert.equal(loadCatalogExport(paths).sandboxes.length, 4);
    const link = join(root, "symlink.json"); symlinkSync(paths.artifact_path, link);
    assert.throws(() => loadCatalogExport({ ...paths, artifact_path: link }), /private_file/);
    chmodSync(paths.artifact_path, 0o644);
    assert.throws(() => loadCatalogExport(paths), /private_file/);
    chmodSync(paths.artifact_path, 0o600); chmodSync(root, 0o755);
    assert.throws(() => loadCatalogExport(paths), /private_path/);
    chmodSync(root, 0o700); rmSync(paths.receipt_path);
    assert.throws(() => loadCatalogExport(paths));
  });
});
