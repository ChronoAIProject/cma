// Provisional synthetic projections. PR-1 serializer SHA
// f3e63466760db540053a3874dde296bf391a62b7 confirms sandbox_id in the BSON
// envelope; the real projection/export and Mongo data remain unverified.
export function fixture() {
  const now = "2026-11-16T01:00:00Z";
  const lease = (fence) => ({
    holder: "fixture-deployment:00000000-0000-0000-0000-000000000001",
    token: "00000000-0000-0000-0000-000000000002",
    fence, expires_at: "2026-11-16T01:30:00Z",
  });
  const sandbox = (id, released = false) => ({
    _id: id, owner: `owner-${id}`,
    record: { lifecycle: { home: { released, receipt: {
      storage_generation: `uid-${id}`, home_identity: `identity-${id}`,
      storage_reference: `home-${id}`,
    } } } },
  });
  const backup = (id, epoch, minute) => ({
    _id: `hb-${id}-${epoch}`, id: `hb-${id}-${epoch}`, document_schema_version: 1,
    sandbox_id: id, epoch, state: "ready",
    source: { pvc_uid: `uid-${id}`, home_identity: `identity-${id}`,
      runtime_generation: 1, runtime_version: "fixture-image", boot_id: `boot-${id}` },
    cut: { requested_at: `2026-11-16T00:${minute}:00Z`,
      provider_creation_time: `2026-11-16T00:${minute}:00Z`,
      ready_at: `2026-11-16T00:${Number(minute) + 1}:00Z` },
    storage: { snapshot_name: `${id}-${epoch}`, snapshot_class: "retain-fixture",
      storage_handle: `projects/fixture/global/snapshots/${id}-${epoch}`,
      restore_size_bytes: 1073741824 },
    custody: { cmek_key_version_id: "cmek-version-fixture", auth_key_ids: ["auth-old", "auth-new"] },
    retention: { class: "hourly", expires_at: "2026-11-17T00:00:00Z" },
    restore_pins: 0, lease: null, next_fence: 1, revision: 1, failure: null, erasure: null,
  });
  const home_backups = [backup("a", 1, "10"), backup("a", 2, "40")];
  const failed = {
    _id: "hb-a-3", id: "hb-a-3", document_schema_version: 1,
    sandbox_id: "a", epoch: 3, state: "failed",
    source: { pvc_uid: "uid-a", home_identity: "identity-a", runtime_generation: 1,
      runtime_version: "fixture-image", boot_id: "boot-a" },
    cut: { requested_at: "2026-11-16T00:50:00Z", provider_creation_time: null, ready_at: null },
    storage: { snapshot_name: "a-3", snapshot_class: "retain-fixture",
      storage_handle: null, restore_size_bytes: null },
    custody: { cmek_key_version_id: null, auth_key_ids: [] },
    retention: { class: "short", expires_at: "2026-11-16T01:00:00Z" },
    restore_pins: 0, lease: null, next_fence: 1, revision: 1,
    failure: "copy_timed_out", erasure: null,
  };
  home_backups.push(failed);
  for (const [state, epoch] of [["requested", 4], ["creating", 5], ["expiring", 6], ["deleted", 7]]) {
    const row = structuredClone(failed);
    row._id = row.id = `hb-a-${epoch}`;
    row.epoch = epoch;
    row.state = state;
    row.cut.requested_at = `2026-11-16T00:${50 + epoch}:00Z`;
    row.retention.expires_at = "2026-11-17T00:00:00Z";
    row.failure = null;
    if (state !== "deleted") {
      row.lease = lease(1);
      row.next_fence = 2;
    }
    row.erasure = state === "deleted"
      ? { erased_at: "2026-11-16T00:59:00Z", receipt: `receipt-a-${epoch}` } : null;
    home_backups.push(row);
  }
  return { mode: "fixture", now,
    sandboxes: [sandbox("a"), sandbox("b"), sandbox("released", true),
      { _id: "unallocated", owner: "owner-unallocated", record: { lifecycle: {} } }],
    home_backups,
    cloud: { mode: "fixture", project: "fixture", observed_at: now, snapshots: [
      ...home_backups.filter((row) => row.state === "ready").map((row) => ({ handle: row.storage.storage_handle,
        status: "READY", created_at: row.cut.provider_creation_time,
        ready_observed_at: row.cut.ready_at, storage_locations: ["fixture-region"],
        cmek_key_version_id: "cmek-version-fixture" })),
      { handle: "projects/fixture/global/snapshots/orphan", status: "READY",
        created_at: now, ready_observed_at: now, storage_locations: ["fixture-region"],
        ownership: { kind: "cma-home-backup", receipt: "fixture-ownership-receipt" } },
    ] },
    dependencies: { atlas: { earliest_at: "2026-11-16T00:00:00Z", latest_at: now },
      available_key_ids: ["auth-old", "auth-new", "cmek-version-fixture"],
      objects: { status: "replicated", consistent_at: now, receipt: "fixture-object-receipt" } },
  };
}
