import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

export function requireThat(condition, code = "invalid_evidence") {
  if (!condition) throw new Error(code);
}
export function timestamp(value) {
  if (value && typeof value === "object") value = value.$date;
  requireThat(typeof value === "string" && /^\d{4}-\d\d-\d\dT/.test(value) && Number.isFinite(Date.parse(value)), "invalid_timestamp");
  return Date.parse(value);
}
export function nonempty(value) {
  return typeof value === "string" && value.trim().length > 0;
}
export function validateEvidence(e) {
  requireThat(e && /^[a-f0-9]{40}$/.test(e.source_sha));
  requireThat(e.image_digests && Object.keys(e.image_digests).length > 0 &&
    Object.values(e.image_digests).every((d) => /^sha256:[a-f0-9]{64}$/.test(d)));
  requireThat(nonempty(e.environment));
  requireThat(timestamp(e.window?.start) <= timestamp(e.window?.end));
  requireThat(Number.isSafeInteger(e.sample_count) && e.sample_count > 0);
  for (const field of ["configuration", "switches"])
    requireThat(e[field] && typeof e[field] === "object" && !Array.isArray(e[field]) && Object.keys(e[field]).length > 0);
  requireThat(Array.isArray(e.raw_results) && e.raw_results.length > 0 && e.raw_results.every(nonempty));
  requireThat(["synthetic", "fixture", "isolated-staging"].includes(e.mode));
  requireThat(e.attempt === 1 && !e.cancelled);
  return true;
}

// Fail closed on unreadable files and symlinks; never echo a match or its path.
export function scan(root, sentinels) {
  requireThat(Array.isArray(sentinels) && sentinels.length > 0 &&
    sentinels.every((s) => nonempty(s) || (Buffer.isBuffer(s) && s.length > 0)), "invalid_sentinels");
  let files = 0, matches = 0;
  const needles = sentinels.map((s) => Buffer.from(s));
  function walk(path) {
    const stat = lstatSync(path);
    requireThat(!stat.isSymbolicLink(), "scan_symlink_refused");
    if (stat.isDirectory()) {
      for (const name of readdirSync(path)) walk(join(path, name));
    } else {
      requireThat(stat.isFile(), "scan_nonfile_refused");
      files++;
      const bytes = readFileSync(path);
      matches += needles.filter((needle) => bytes.includes(needle)).length;
    }
  }
  walk(root);
  requireThat(files > 0, "scan_empty");
  return { files, matches };
}
