import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { requireThat, timestamp, nonempty } from "./evidence.mjs";

const hash = (s) => createHash("sha256").update(s).digest("hex");
function unique(rows, field) {
  requireThat(Array.isArray(rows), "invalid_inventory");
  const keys = rows.map((r) => r?.[field]);
  requireThat(keys.every(nonempty) && new Set(keys).size === keys.length, "duplicate_or_missing_identity");
}

export function coverage(input) {
  const { sandboxes, home_backups: backups, cloud, dependencies: deps } = input;
  requireThat(["fixture", "cloud"].includes(input.mode) && cloud?.mode === input.mode, "mode_mismatch");
  const now = timestamp(input.now);
  requireThat(timestamp(cloud.observed_at) <= now && now - timestamp(cloud.observed_at) <= 300_000, "cloud_observation_stale_or_future");
  unique(sandboxes, "_id"); unique(backups, "_id"); unique(cloud.snapshots, "handle");
  requireThat(deps?.atlas && Array.isArray(deps.available_key_ids) && deps.objects, "dependencies_missing");
  const mongoStart = timestamp(deps.atlas.earliest_at), mongoEnd = timestamp(deps.atlas.latest_at);
  requireThat(mongoStart <= mongoEnd && mongoEnd <= now, "invalid_pitr_window");
  const retained = sandboxes.filter((row) => {
    requireThat(row.record?.lifecycle && nonempty(row.owner), "invalid_sandbox_projection");
    const home = row.record.lifecycle.home;
    if (home == null) return false;
    requireThat(typeof home.released === "boolean", "invalid_home_projection");
    if (home.released || home.receipt == null) return false;
    requireThat(nonempty(home.receipt.storage_generation) && nonempty(home.receipt.storage_reference), "invalid_home_receipt");
    return true;
  });
  for (const b of backups) {
    requireThat(nonempty(b.sandbox_id) && nonempty(b.state) && nonempty(b.storage?.storage_handle), "invalid_catalog_projection");
    if (b.state === "ready") {
      requireThat(nonempty(b.source?.pvc_uid) && nonempty(b.source.home_identity), "invalid_source");
      requireThat(nonempty(b.custody?.cmek_key_version_id) && Array.isArray(b.custody.auth_key_ids) &&
        b.custody.auth_key_ids.length > 0 && b.custody.auth_key_ids.every(nonempty), "invalid_custody");
      const requested = timestamp(b.cut?.requested_at), created = timestamp(b.cut?.provider_creation_time), ready = timestamp(b.cut?.ready_at);
      requireThat(requested <= created && created <= ready && ready <= now, "invalid_catalog_times");
      timestamp(b.retention?.expires_at);
    }
  }
  const homes = retained.map((row) => {
    const reasons = [];
    const candidates = backups.filter((b) => b.sandbox_id === row._id && b.state === "ready")
      .sort((a, b) => timestamp(b.cut.provider_creation_time) - timestamp(a.cut.provider_creation_time));
    const latest = candidates[0];
    const result = { sandbox_sha256: hash(row._id), owner_sha256: hash(row.owner), reasons,
      capture_gap_seconds: null, completion_upper_bound_seconds: null };
    if (!latest) { reasons.push("catalog_missing"); return result; }
    result.backup_sha256 = hash(latest._id);
    const created = timestamp(latest.cut.provider_creation_time);
    if (latest.source.pvc_uid !== row.record.lifecycle.home.receipt.storage_generation) reasons.push("source_changed");
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
  const known = new Set(backups.map((b) => b.storage.storage_handle));
  const unmatched = cloud.snapshots.filter((s) => !known.has(s.handle));
  const attributed = (s, kinds) => kinds.includes(s.ownership?.kind) && nonempty(s.ownership?.receipt);
  const orphans = unmatched.filter((s) => attributed(s, ["cma-home-backup"])).map((s) => hash(s.handle));
  const excluded = unmatched.filter((s) => attributed(s, ["seed", "unrelated"]));
  const unknown = unmatched.filter((s) => !attributed(s, ["cma-home-backup", "seed", "unrelated"])).map((s) => hash(s.handle));
  return { mode: input.mode, status: !homes.length ? "NOT_RUN" : backed === homes.length ? "PASS" : "FAIL",
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
