import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { requireThat, timestamp, nonempty } from "./evidence.mjs";

// This interface is opt-in and read-only. No default project or ambient mutation.
export function inventory(project, run = spawnSync) {
  requireThat(typeof project === "string" && /^[a-z][a-z0-9-]{2,62}$/.test(project), "invalid_project");
  const result = run("gcloud", ["compute", "snapshots", "list", "--project", project, "--format=json", "--quiet"],
    { encoding: "utf8", timeout: 60_000, maxBuffer: 64 * 1024 * 1024 });
  requireThat(result.status === 0 && !result.error, "cloud_list_failed");
  let rows;
  try { rows = JSON.parse(result.stdout); } catch { throw new Error("cloud_invalid_json"); }
  requireThat(Array.isArray(rows), "cloud_invalid_rows");
  const observed_at = new Date().toISOString();
  const snapshots = rows.map((row) => {
    const prefix = "https://www.googleapis.com/compute/v1/";
    requireThat(nonempty(row.selfLink) && row.selfLink.startsWith(`${prefix}projects/${project}/global/snapshots/`), "cloud_wrong_project");
    timestamp(row.creationTimestamp);
    requireThat(nonempty(row.status), "cloud_missing_status");
    return { handle: row.selfLink.slice(prefix.length), status: row.status,
      created_at: row.creationTimestamp,
      // GCE list has no exact completion timestamp: this is an upper bound.
      ready_observed_at: row.status === "READY" ? observed_at : null,
      storage_locations: row.storageLocations ?? [],
      cmek_key_version_id: row.snapshotEncryptionKey?.kmsKeyName ?? null,
      // A project-wide list cannot prove application ownership, even by name.
      ownership: { kind: "unknown" } };
  });
  return { mode: "cloud", scope: "project", project, observed_at, snapshots };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    requireThat(process.argv.length === 3, "usage_inventory_project");
    console.log(JSON.stringify(inventory(process.argv[2]), null, 2));
  } catch { console.error("inventory_failed"); process.exitCode = 1; }
}
