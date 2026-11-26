import { createHash } from "node:crypto";
import { constants, openSync, fstatSync, closeSync, readSync, lstatSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, normalize } from "node:path";
import { requireThat } from "./evidence.mjs";
import { keys, integer, digest, timestamp, sandbox, schedule, backup } from "./catalog-projection.mjs";

const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const collections = ["sandboxes", "home_backup_schedules", "home_backups"];
const MAX_BYTES = 16 * 1024 * 1024;
function parse(bytes) {
  try { return JSON.parse(bytes); } catch { throw new Error("catalog_export_invalid_json"); }
}

function privateRead(path, maxBytes) {
  requireThat(typeof path === "string" && isAbsolute(path) && normalize(path) === path, "catalog_export_private_path");
  const parent = dirname(path), parentStat = lstatSync(parent);
  requireThat(parentStat.isDirectory() && !parentStat.isSymbolicLink() &&
    (parentStat.mode & 0o777) === 0o700 && realpathSync(parent) === parent, "catalog_export_private_path");
  const before = lstatSync(path);
  requireThat(before.isFile() && !before.isSymbolicLink() && before.nlink === 1 &&
    (before.mode & 0o777) === 0o600 && before.size <= maxBytes, "catalog_export_private_file");
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = fstatSync(fd);
    requireThat(opened.dev === before.dev && opened.ino === before.ino && opened.size === before.size, "catalog_export_file_changed");
    const bytes = Buffer.alloc(before.size + 1);
    let used = 0;
    while (used < bytes.length) {
      const read = readSync(fd, bytes, used, bytes.length - used, null);
      if (!read) break;
      used += read;
    }
    requireThat(used === before.size, "catalog_export_file_changed");
    return bytes.subarray(0, used);
  } finally { closeSync(fd); }
}

export function loadCatalogExport(paths) {
  try {
    keys(paths, ["artifact_path", "receipt_path"]);
    return validateCatalogExport(privateRead(paths.artifact_path, MAX_BYTES),
      parse(privateRead(paths.receipt_path, 8192)));
  } catch (error) {
    throw new Error(/^catalog_export_[a-z_]+$/.test(error.message) ? error.message : "catalog_export_unreadable");
  }
}

export function validateCatalogExport(bytes, receipt) {
  requireThat((Buffer.isBuffer(bytes) || typeof bytes === "string") && Buffer.byteLength(bytes) <= MAX_BYTES, "catalog_export_size");
  const envelope = parse(bytes);
  keys(envelope, ["format", "payload_sha256", "payload_json"]);
  requireThat(envelope.format === "home-backup-catalog-export-v1" && typeof envelope.payload_json === "string", "catalog_export_format");
  digest(envelope.payload_sha256);
  requireThat(hash(envelope.payload_json) === envelope.payload_sha256, "catalog_export_payload_digest");
  const payload = parse(envelope.payload_json);
  keys(payload, ["format", "source_kind", "run_id", "identity", "provenance", "observed_start", "observed_end", "snapshot", "scope", "page_size", "collections"]);
  integer(payload.page_size, 1); requireThat(payload.page_size <= 128, "catalog_export_limit");
  requireThat(payload.format === "home-backup-catalog-payload-v1" && payload.source_kind === "mongo_snapshot" &&
    payload.scope === "all_records_in_three_collections" && payload.provenance === "operator_claim_requires_external_verification" &&
    /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(payload.run_id), "catalog_export_manifest");
  keys(payload.identity, ["source_revision_claim", "executable_sha256"]);
  requireThat(/^[a-f0-9]{40}$/.test(payload.identity.source_revision_claim), "catalog_export_source_identity");
  digest(payload.identity.executable_sha256);
  const start = timestamp(payload.observed_start), end = timestamp(payload.observed_end);
  requireThat(start <= end && end - start <= 60_000, "catalog_export_observation_window");
  keys(payload.snapshot, ["time", "increment", "session_sha256", "scope_sha256"]);
  integer(payload.snapshot.time, 1); integer(payload.snapshot.increment); digest(payload.snapshot.session_sha256); digest(payload.snapshot.scope_sha256);
  requireThat(payload.snapshot.time <= 0xffff_ffff && payload.snapshot.increment <= 0xffff_ffff &&
    payload.snapshot.time * 1000 <= end, "catalog_export_snapshot");
  keys(payload.collections, collections);
  let total = 0, pages = 0;
  for (const name of collections) {
    const entry = payload.collections[name];
    keys(entry, ["rows", "count", "pages", "eof"]);
    integer(entry.count); integer(entry.pages, 1);
    requireThat(entry.eof === true && Array.isArray(entry.rows) && entry.count === entry.rows.length &&
      entry.pages === Math.floor(entry.count / payload.page_size) + 1, "catalog_export_incomplete");
    const ids = entry.rows.map((row) => row.id);
    requireThat(ids.every((id, i) => typeof id === "string" && id.length > 0 && (!i || Buffer.compare(Buffer.from(ids[i - 1]), Buffer.from(id)) < 0)), "catalog_export_identity_order");
    total += entry.count; pages += entry.pages;
  }
  requireThat(total <= 100_000 && pages <= 4096, "catalog_export_limit");
  keys(receipt, ["format", "artifact_sha256", "payload_sha256", "run_sha256", "source_revision_claim", "executable_sha256", "counts"]);
  keys(receipt.counts, collections);
  requireThat(receipt.format === "home-backup-catalog-receipt-v1" && receipt.artifact_sha256 === hash(bytes) &&
    receipt.payload_sha256 === envelope.payload_sha256 && receipt.run_sha256 === hash(payload.run_id) &&
    receipt.source_revision_claim === payload.identity.source_revision_claim &&
    receipt.executable_sha256 === payload.identity.executable_sha256 &&
    collections.every((name) => receipt.counts[name] === payload.collections[name].count), "catalog_export_receipt_mismatch");
  return { sandboxes: payload.collections.sandboxes.rows.map(sandbox),
    home_backup_schedules: payload.collections.home_backup_schedules.rows.map(schedule),
    home_backups: payload.collections.home_backups.rows.map(backup),
    observed_end: payload.observed_end, snapshot_time: payload.snapshot.time,
    provenance_status: "RECEIPT_BOUND_UNVERIFIED", artifact_sha256: receipt.artifact_sha256 };
}
