import { requireThat, timestamp as parseTime, nonempty } from "./evidence.mjs";

export function timestamp(value) {
  requireThat(typeof value === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(value), "catalog_export_timestamp");
  const time = parseTime(value);
  requireThat(time >= 0 && new Date(time).toISOString().replace(".000Z", "Z") === value, "catalog_export_timestamp");
  return time;
}

export function keys(value, names) {
  requireThat(value && typeof value === "object" && !Array.isArray(value) &&
    Object.keys(value).sort().join(",") === [...names].sort().join(","), "catalog_export_shape");
}
export const text = (value) => requireThat(nonempty(value) && value.trim() === value && !/[\x00-\x1f\x7f]/.test(value), "catalog_export_text");
export const integer = (value, min = 0) => requireThat(Number.isSafeInteger(value) && value >= min, "catalog_export_integer");
export const digest = (value) => requireThat(typeof value === "string" && /^[a-f0-9]{64}$/.test(value), "catalog_export_digest");
const optional = (value, validate) => { if (value !== null) validate(value); };
const member = (value, values) => requireThat(values.includes(value), "catalog_export_enum");
function authority(value) {
  keys(value, ["validation", "lease_present"]);
  requireThat(value.validation === "strict_codec_v1" && typeof value.lease_present === "boolean", "catalog_export_authority");
}
function home(value) {
  keys(value, ["mode", "home_source_id", "home_identity", "source_reference"]);
  [value.mode, value.home_source_id, value.home_identity].forEach(text);
  optional(value.source_reference, text);
}

export function sandbox(value) {
  keys(value, ["id", "owner_sha256", "home"]); text(value.id); digest(value.owner_sha256);
  if (value.home !== null) {
    keys(value.home, ["released", "receipt"]);
    requireThat(typeof value.home.released === "boolean", "catalog_export_home");
    if (value.home.receipt !== null) {
      keys(value.home.receipt, ["storage_generation", "home_identity", "storage_reference"]);
      Object.values(value.home.receipt).forEach(text);
    }
  }
  return { _id: value.id, owner_sha256: value.owner_sha256, record: { lifecycle: { home: value.home } } };
}

export function schedule(value) {
  keys(value, ["id", "home", "state", "next_due_at", "last_ready_at", "last_ready_backup_id", "in_flight", "revision", "next_fence", "authority"]);
  text(value.id); home(value.home); authority(value.authority);
  member(value.state, ["active", "retiring", "erasing", "restoring", "closed"]);
  integer(value.revision, 1); integer(value.next_fence, 1); timestamp(value.next_due_at);
  optional(value.last_ready_at, timestamp); optional(value.last_ready_backup_id, text);
  requireThat((value.last_ready_at === null) === (value.last_ready_backup_id === null), "catalog_export_schedule");
  if (value.in_flight !== null) {
    keys(value.in_flight, ["backup_id", "phase", "started_at"]);
    text(value.in_flight.backup_id); timestamp(value.in_flight.started_at);
    member(value.in_flight.phase, ["requested", "creating", "observing"]);
  }
  requireThat(value.state !== "closed" || (!value.authority.lease_present && value.in_flight === null), "catalog_export_schedule");
  return value;
}

export function backup(value) {
  keys(value, ["id", "sandbox_id", "epoch", "source", "cut", "storage", "custody", "retention", "state", "restore_pins", "next_fence", "revision", "failure", "erasure", "authority"]);
  text(value.id); text(value.sandbox_id); integer(value.epoch, 1); authority(value.authority);
  requireThat(value.id === `hb-${value.sandbox_id}-${value.epoch}`, "catalog_export_backup_identity");
  member(value.state, ["requested", "creating", "ready", "failed", "expiring", "deleted"]);
  keys(value.source, ["home_source_id", "home_identity", "runtime_generation", "runtime_version", "boot_id"]);
  [value.source.home_source_id, value.source.home_identity, value.source.runtime_version, value.source.boot_id].forEach(text);
  integer(value.source.runtime_generation, 1);
  keys(value.cut, ["requested_at", "source_creation_time", "ready_at"]);
  const requested = timestamp(value.cut.requested_at);
  optional(value.cut.source_creation_time, timestamp); optional(value.cut.ready_at, timestamp);
  keys(value.storage, ["snapshot_name", "snapshot_class", "source_reference", "restore_size_bytes"]);
  text(value.storage.snapshot_name); text(value.storage.snapshot_class); optional(value.storage.source_reference, text);
  optional(value.storage.restore_size_bytes, (n) => integer(n, 1));
  keys(value.custody, ["encryption_key_version_id", "auth_key_ids"]);
  optional(value.custody.encryption_key_version_id, text);
  requireThat(Array.isArray(value.custody.auth_key_ids) && new Set(value.custody.auth_key_ids).size === value.custody.auth_key_ids.length, "catalog_export_custody");
  value.custody.auth_key_ids.forEach(text);
  keys(value.retention, ["class", "expires_at"]);
  member(value.retention.class, ["short", "hourly", "daily"]);
  const expires = timestamp(value.retention.expires_at);
  integer(value.restore_pins); integer(value.next_fence, 1); integer(value.revision, 1);
  optional(value.failure, (f) => member(f, ["source_changed", "copy_timed_out", "source_unavailable", "copy_unavailable", "copy_missing", "capture_cancelled"]));
  if (value.erasure !== null) { keys(value.erasure, ["erased_at", "receipt"]); timestamp(value.erasure.erased_at); text(value.erasure.receipt); }
  requireThat((value.state === "deleted") === (value.erasure !== null) &&
    (value.state !== "failed" || value.failure !== null) &&
    (["failed", "expiring", "deleted"].includes(value.state) || value.failure === null) &&
    (value.state === "ready" || value.restore_pins === 0), "catalog_export_backup_state");
  const created = value.cut.source_creation_time === null ? null : timestamp(value.cut.source_creation_time);
  const ready = value.cut.ready_at === null ? null : timestamp(value.cut.ready_at);
  requireThat((created === null || created >= requested) &&
    (ready === null || (created !== null && ready >= created)) &&
    expires >= (ready ?? requested) && expires - (ready ?? requested) <= 30 * 86400_000 &&
    (value.state !== "ready" || (ready !== null && value.storage.source_reference !== null)), "catalog_export_backup_times");
  return { ...value, _id: value.id, document_schema_version: 1,
    source: { ...value.source, pvc_uid: value.source.home_source_id },
    cut: { ...value.cut, provider_creation_time: value.cut.source_creation_time },
    storage: { ...value.storage, storage_handle: value.storage.source_reference },
    custody: { ...value.custody, cmek_key_version_id: value.custody.encryption_key_version_id } };
}
