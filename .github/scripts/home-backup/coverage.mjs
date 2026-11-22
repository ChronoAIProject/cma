import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { requireThat, timestamp, nonempty } from "./evidence.mjs";
import { loadCatalogExport } from "./catalog-export.mjs";

const hash = (s) => createHash("sha256").update(s).digest("hex");
const states = new Set(["requested", "creating", "ready", "failed", "expiring", "deleted"]);
const failures = new Set(["source_changed", "copy_timed_out", "source_unavailable",
  "copy_unavailable", "copy_missing", "capture_cancelled"]);
const retentionClasses = new Set(["short", "hourly", "daily"]);
const uuid = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
function unique(rows, field) {
  requireThat(Array.isArray(rows), "invalid_inventory");
  const keys = rows.map((r) => r?.[field]);
  requireThat(keys.every(nonempty) && new Set(keys).size === keys.length, "duplicate_or_missing_identity");
}
function validateLease(lease, nextFence) {
  if (lease === null) return;
  let expires;
  try { expires = timestamp(lease?.expires_at); } catch { throw new Error("invalid_lease"); }
  requireThat(lease && nonempty(lease.holder) && nonempty(lease.token) &&
    uuid.test(lease.token) && Number.isSafeInteger(lease.fence) && lease.fence > 0 &&
    lease.fence < nextFence && expires, "invalid_lease");
  const separator = lease.holder.lastIndexOf(":");
  requireThat(separator > 0 && nonempty(lease.holder.slice(0, separator)) &&
    uuid.test(lease.holder.slice(separator + 1)), "invalid_lease");
}

export function coverage(input) {
  const exported = input.catalog_export ? loadCatalogExport(input.catalog_export) : null;
  if (exported) {
    requireThat(!["sandboxes", "home_backups", "home_backup_schedules"].some((name) => Object.hasOwn(input, name)), "catalog_export_inventory_override");
    input = { ...input, ...exported };
  }
  const { sandboxes, home_backups: backups, cloud, dependencies: deps } = input;
  requireThat(["fixture", "cloud"].includes(input.mode) && cloud?.mode === input.mode, "mode_mismatch");
  const now = timestamp(input.now);
  if (exported) requireThat(timestamp(exported.observed_end) <= now &&
    now - timestamp(exported.observed_end) <= 300_000 && now - exported.snapshot_time * 1000 <= 300_000,
    "catalog_export_stale_or_future");
  requireThat(timestamp(cloud.observed_at) <= now && now - timestamp(cloud.observed_at) <= 300_000, "cloud_observation_stale_or_future");
  unique(sandboxes, "_id"); unique(backups, "_id"); unique(cloud.snapshots, "handle");
  requireThat(deps?.atlas && Array.isArray(deps.available_key_ids) && deps.objects, "dependencies_missing");
  const mongoStart = timestamp(deps.atlas.earliest_at), mongoEnd = timestamp(deps.atlas.latest_at);
  requireThat(mongoStart <= mongoEnd && mongoEnd <= now, "invalid_pitr_window");
  const retained = sandboxes.filter((row) => {
    requireThat(row.record?.lifecycle && (exported ? nonempty(row.owner_sha256) : nonempty(row.owner)), "invalid_sandbox_projection");
    const home = row.record.lifecycle.home;
    if (home == null) return false;
    requireThat(typeof home.released === "boolean", "invalid_home_projection");
    if (home.released || home.receipt == null) return false;
    requireThat(nonempty(home.receipt.storage_generation) && nonempty(home.receipt.home_identity) &&
      nonempty(home.receipt.storage_reference), "invalid_home_receipt");
    return true;
  });
  for (const b of backups) {
    requireThat(nonempty(b._id) && b.id === b._id && b.document_schema_version === 1 &&
      nonempty(b.sandbox_id) && Number.isSafeInteger(b.epoch) && b.epoch > 0 &&
      b._id === `hb-${b.sandbox_id}-${b.epoch}` && states.has(b.state), "invalid_catalog_projection");
    requireThat(nonempty(b.source?.pvc_uid) && nonempty(b.source.home_identity) &&
      Number.isSafeInteger(b.source.runtime_generation) && b.source.runtime_generation > 0 &&
      nonempty(b.source.runtime_version) && nonempty(b.source.boot_id), "invalid_source");
    requireThat(timestamp(b.cut?.requested_at), "invalid_cut");
    for (const field of ["provider_creation_time", "ready_at"])
      if (b.cut[field] != null) timestamp(b.cut[field]);
    requireThat(nonempty(b.storage?.snapshot_name) && nonempty(b.storage.snapshot_class), "invalid_storage");
    if (b.storage.storage_handle != null) requireThat(nonempty(b.storage.storage_handle), "invalid_storage_handle");
    if (b.storage.restore_size_bytes != null)
      requireThat(Number.isSafeInteger(b.storage.restore_size_bytes) && b.storage.restore_size_bytes > 0, "invalid_restore_size");
    requireThat(b.custody && (b.custody.cmek_key_version_id == null || nonempty(b.custody.cmek_key_version_id)) &&
      Array.isArray(b.custody.auth_key_ids) && b.custody.auth_key_ids.every(nonempty), "invalid_custody");
    requireThat(retentionClasses.has(b.retention?.class), "invalid_retention");
    timestamp(b.retention?.expires_at);
    requireThat(Number.isSafeInteger(b.restore_pins) && b.restore_pins >= 0 &&
      Number.isSafeInteger(b.next_fence) && b.next_fence > 0 &&
      Number.isSafeInteger(b.revision) && b.revision > 0 && (exported || b.lease === null || typeof b.lease === "object"),
      "invalid_authority_projection");
    if (!exported) validateLease(b.lease, b.next_fence);
    if (b.failure != null) requireThat(failures.has(b.failure), "invalid_failure");
    if (b.erasure != null) requireThat(nonempty(b.erasure.receipt) && timestamp(b.erasure.erased_at), "invalid_erasure");
    if (b.state === "failed") requireThat(b.failure != null, "failed_without_reason");
    if (b.state === "deleted") requireThat(b.erasure != null, "deleted_without_erasure");
    if (b.state !== "ready") requireThat(b.restore_pins === 0, "nonready_restore_pin");
    if (b.state === "ready") {
      requireThat(nonempty(b.storage.storage_handle) && b.cut.provider_creation_time != null && b.cut.ready_at != null,
        "ready_missing_evidence");
      const requested = timestamp(b.cut.requested_at), created = timestamp(b.cut.provider_creation_time), ready = timestamp(b.cut.ready_at);
      requireThat(requested <= created && created <= ready && ready <= now, "invalid_catalog_times");
    }
  }
  const homes = retained.map((row) => {
    const reasons = [];
    const sandboxCandidates = backups.filter((b) => b.sandbox_id === row._id && b.state === "ready")
      .sort((a, b) => timestamp(b.cut.provider_creation_time) - timestamp(a.cut.provider_creation_time));
    const receipt = row.record.lifecycle.home.receipt;
    const matchesCurrentHome = (b) => b.source.pvc_uid === receipt.storage_generation &&
      b.source.home_identity === receipt.home_identity;
    // Superseded homes remain in the catalog for project-wide orphan accounting,
    // but cannot provide coverage or cadence evidence for the retained home.
    const candidates = sandboxCandidates.filter(matchesCurrentHome);
    const latestOverall = sandboxCandidates[0];
    const latest = candidates[0];
    const result = { sandbox_sha256: hash(row._id), owner_sha256: exported ? row.owner_sha256 : hash(row.owner), reasons,
      capture_gap_seconds: null, completion_upper_bound_seconds: null };
    if (exported) {
      const schedule = input.home_backup_schedules.find((s) => s.id === row._id);
      result.schedule_status = "MATCHING";
      if (!schedule) { result.schedule_status = "MISSING"; reasons.push("schedule_missing"); }
      else if (schedule.home.home_source_id !== receipt.storage_generation ||
        schedule.home.home_identity !== receipt.home_identity || schedule.home.source_reference !== receipt.storage_reference) {
        result.schedule_status = "MISMATCHED"; reasons.push("schedule_home_mismatch");
      } else {
        const references = [schedule.last_ready_backup_id, schedule.in_flight?.backup_id].filter(nonempty);
        if (references.some((id) => !backups.some((b) => b.id === id && b.sandbox_id === row._id &&
          matchesCurrentHome(b) && (id !== schedule.last_ready_backup_id ||
            b.cut.ready_at === schedule.last_ready_at)))) {
          result.schedule_status = "CATALOG_MISMATCHED"; reasons.push("schedule_catalog_mismatch");
        }
      }
    }
    if (!latest) {
      if (latestOverall) {
        result.backup_sha256 = hash(latestOverall._id);
        reasons.push("source_changed");
      } else reasons.push("catalog_missing");
      return result;
    }
    // A newer ready record for a different physical home supersedes the old
    // evidence; do not fall back to an older matching record.
    if (latestOverall && !matchesCurrentHome(latestOverall)) {
      result.backup_sha256 = hash(latestOverall._id);
      reasons.push("source_changed");
      return result;
    }
    result.backup_sha256 = hash(latest._id);
    const created = timestamp(latest.cut.provider_creation_time);
    if (timestamp(latest.retention.expires_at) <= now) reasons.push("expired");
    if (now - created > 3_600_000) reasons.push("integrated_rpo");
    if (now - timestamp(latest.cut.requested_at) > 1_800_000) reasons.push("capture_overdue");
    if (timestamp(latest.cut.ready_at) - timestamp(latest.cut.requested_at) > 1_800_000) reasons.push("catalog_completion_late");
    if (candidates.length < 2) reasons.push("capture_gap_unknown");
    else {
      // Older hourly/daily retention is not a continuous capture-history log.
      // Report only the latest pair; historical cadence needs raw capture receipts.
      const ordered = [...candidates.slice(0, 2)].sort((a, b) => timestamp(a.cut.requested_at) - timestamp(b.cut.requested_at));
      result.capture_gap_seconds = Math.max(...ordered.slice(1).map((b, i) =>
        (timestamp(b.cut.requested_at) - timestamp(ordered[i].cut.requested_at)) / 1000));
      if (result.capture_gap_seconds > 1800) reasons.push("capture_gap");
    }
    if (mongoEnd < created || mongoStart > created) reasons.push("mongo_cut_unavailable");
    if (now - mongoEnd > 300_000) reasons.push("mongo_rpo");
    if ([latest.custody.cmek_key_version_id, ...latest.custody.auth_key_ids]
      .some((id) => !deps.available_key_ids.includes(id))) reasons.push("key_unavailable");
    if (deps.objects.status !== "replicated" || !nonempty(deps.objects.receipt) ||
      timestamp(deps.objects.consistent_at) < mongoEnd || timestamp(deps.objects.consistent_at) > now) reasons.push("objects_unavailable");
    const snapshot = cloud.snapshots.find((s) => s.handle === latest.storage.storage_handle);
    if (!snapshot) reasons.push("cloud_missing");
    else {
      if (snapshot.status !== "READY") reasons.push("cloud_not_ready");
      if (timestamp(snapshot.created_at) !== created) reasons.push("cloud_cut_mismatch");
      if (!Array.isArray(snapshot.storage_locations) || !snapshot.storage_locations.length) reasons.push("location_unknown");
      if (snapshot.cmek_key_version_id !== latest.custody.cmek_key_version_id) reasons.push("cmek_mismatch");
      if (!snapshot.ready_observed_at) reasons.push("completion_unknown");
      else {
        const upper = timestamp(snapshot.ready_observed_at);
        requireThat(upper >= created && upper <= timestamp(cloud.observed_at), "invalid_cloud_times");
        result.completion_upper_bound_seconds = (upper - timestamp(latest.cut.requested_at)) / 1000;
        if (result.completion_upper_bound_seconds > 1800) reasons.push("completion_not_proven_within_30m");
      }
    }
    return result;
  });
  const backed = homes.filter((h) => h.reasons.length === 0).length;
  const known = new Set(backups.map((b) => b.storage.storage_handle).filter(nonempty));
  const unmatched = cloud.snapshots.filter((s) => !known.has(s.handle));
  const attributed = (s, kinds) => kinds.includes(s.ownership?.kind) && nonempty(s.ownership?.receipt);
  const orphans = unmatched.filter((s) => attributed(s, ["cma-home-backup"])).map((s) => hash(s.handle));
  const excluded = unmatched.filter((s) => attributed(s, ["seed", "unrelated"]));
  const unknown = unmatched.filter((s) => !attributed(s, ["cma-home-backup", "seed", "unrelated"])).map((s) => hash(s.handle));
  return { mode: input.mode, status: !homes.length ? "NOT_RUN" : backed === homes.length ? "PASS" : "FAIL",
    catalog_export_status: exported ? "AVAILABLE" : "BLOCKED",
    catalog_provenance_status: exported?.provenance_status ?? "NOT_RUN",
    catalog_artifact_sha256: exported?.artifact_sha256 ?? null,
    capture_cadence_scope: "latest_current_home_pair",
    historical_cadence_status: "NOT_RUN", home_restore_status: "NOT_RUN", full_dr_status: "NOT_RUN",
    restore_status: "NOT_RUN", qualification_status: "NOT_RUN", failure_domain_status: "NOT_RUN",
    cmek_recovery_status: "NOT_RUN", retained_homes: homes.length, backed_homes: backed,
    coverage: homes.length ? backed / homes.length : null, homes,
    orphan_count: orphans.length, orphan_sha256: orphans,
    ownership_unknown_count: unknown.length, ownership_unknown_sha256: unknown,
    excluded_snapshot_count: excluded.length };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    requireThat(process.argv.length === 3, "usage_coverage_input_json");
    const report = coverage(JSON.parse(readFileSync(process.argv[2], "utf8")));
    console.log(JSON.stringify(report, null, 2));
    if (report.status !== "PASS") process.exitCode = 1;
  } catch { console.error("coverage_failed"); process.exitCode = 1; }
}
