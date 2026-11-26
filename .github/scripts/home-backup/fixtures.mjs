// Provisional synthetic projections. PR-1 serializer SHA
// f3e63466760db540053a3874dde296bf391a62b7 confirms sandbox_id in the BSON
// envelope; the real projection/export and Mongo data remain unverified.
import { createHash } from "node:crypto";

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

// Synthetic parser fixtures only. Mongo-to-Rust-to-Node contracts live in the
// store suite and construct artifacts through the real exporter.
export function catalogFixture() {
  const input = fixture();
  const authority = (lease) => ({ validation: "strict_codec_v1", lease_present: lease !== null });
  const hash = (s) => createHash("sha256").update(s).digest("hex");
  const rows = {
    sandboxes: input.sandboxes.map(s => ({ id: s._id, owner_sha256: hash(s.owner), home: s.record.lifecycle.home ?? null })),
    home_backup_schedules: [{ id: "a", home: { mode: "full_home", home_source_id: "uid-a",
      home_identity: "identity-a", source_reference: "home-a" }, state: "active",
      next_due_at: input.now, last_ready_at: "2026-11-16T00:41:00Z", last_ready_backup_id: "hb-a-2",
      in_flight: null, next_fence: 1, revision: 1, authority: authority(null) }],
    home_backups: input.home_backups.map(b => {
      const { _id, document_schema_version, lease, source, cut, storage, custody, ...rest } = b;
      const { pvc_uid, ...sourceRest } = source;
      const { provider_creation_time, ...cutRest } = cut;
      const { storage_handle, ...storageRest } = storage;
      const { cmek_key_version_id, ...custodyRest } = custody;
      return { ...rest, source: { ...sourceRest, home_source_id: pvc_uid },
        cut: { ...cutRest, source_creation_time: provider_creation_time },
        storage: { ...storageRest, source_reference: storage_handle },
        custody: { ...custodyRest, encryption_key_version_id: cmek_key_version_id }, authority: authority(lease) };
    }),
  };
  const collections = Object.fromEntries(Object.entries(rows).map(([name, data]) => [name, {
    rows: data.sort((a, b) => Buffer.compare(Buffer.from(a.id), Buffer.from(b.id))),
    count: data.length, pages: 1, eof: true,
  }]));
  return { format: "home-backup-catalog-payload-v1", source_kind: "mongo_snapshot",
    run_id: "00000000-0000-4000-8000-000000000001",
    identity: { source_revision_claim: "a".repeat(40), executable_sha256: "b".repeat(64) },
    provenance: "operator_claim_requires_external_verification", observed_start: input.now, observed_end: input.now,
    snapshot: { time: Date.parse(input.now) / 1000, increment: 1, session_sha256: "c".repeat(64), scope_sha256: "d".repeat(64) },
    scope: "all_records_in_three_collections", page_size: 128, collections };
}

export function encodeCatalogFixture(payload = catalogFixture()) {
  const hash = (s) => createHash("sha256").update(s).digest("hex");
  const payload_json = JSON.stringify(payload), payload_sha256 = hash(payload_json);
  const bytes = JSON.stringify({ format: "home-backup-catalog-export-v1", payload_sha256, payload_json });
  const receipt = { format: "home-backup-catalog-receipt-v1", artifact_sha256: hash(bytes), payload_sha256,
    run_sha256: hash(payload.run_id), ...payload.identity,
    counts: Object.fromEntries(Object.entries(payload.collections).map(([name, entry]) => [name, entry.count])) };
  return { bytes, receipt };
}
