// Provisional synthetic projections. Catalog DTO mapping is BLOCKED pending
// PR-1 alignment: sandbox_id is not established by §3.2; this is not Mongo data.
export function fixture() {
  const now = "2026-11-16T01:00:00Z";
  const sandbox = (id, released = false) => ({
    _id: id, owner: `owner-${id}`,
    record: { lifecycle: { home: { released, receipt: {
      storage_generation: `uid-${id}`, storage_reference: `home-${id}`,
    } } } },
  });
  const backup = (id, epoch, minute) => ({
    _id: `hb-${id}-${epoch}`, sandbox_id: id, epoch, state: "ready",
    source: { pvc_uid: `uid-${id}`, home_identity: `identity-${id}` },
    cut: { requested_at: `2026-11-16T00:${minute}:00Z`,
      provider_creation_time: `2026-11-16T00:${minute}:00Z`,
      ready_at: `2026-11-16T00:${Number(minute) + 1}:00Z` },
    storage: { storage_handle: `projects/fixture/global/snapshots/${id}-${epoch}` },
    custody: { cmek_key_version_id: "cmek-version-fixture", auth_key_ids: ["auth-old", "auth-new"] },
    retention: { class: "hourly", expires_at: "2026-11-17T00:00:00Z" },
  });
  const home_backups = [backup("a", 1, "10"), backup("a", 2, "40")];
  return { mode: "fixture", now,
    sandboxes: [sandbox("a"), sandbox("b"), sandbox("released", true),
      { _id: "unallocated", owner: "owner-unallocated", record: { lifecycle: {} } }],
    home_backups,
    cloud: { mode: "fixture", project: "fixture", observed_at: now, snapshots: [
      ...home_backups.map((row) => ({ handle: row.storage.storage_handle,
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
