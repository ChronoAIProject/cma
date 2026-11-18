# Changelog

- **1.0.4:** Add bounded static-library traversal, parsed CA proofs, exact component content decisions and exact Debian and Arch package-bound upstream vector policies. Require complete CA DER consumption and physical ar metadata coverage. Preserve raw findings and fail closed on every other key or incomplete scan.

- **1.0.3 — 2026-11-17:** Add explicit owner-authorized Policy A (`--blocking-rules=credential`): full detection remains unchanged, generic entropy is recorded as advisory, credentials and unresolved paths remain blocking, and every REAL classification blocks. Add archive/policy-bound per-finding SA-01 review packets, distinct candidate/finding hashes, separate receipt counts and mode-aware artifact-to-push validation. Default strict mode is unchanged.

- **1.0.2 — 2026-11-17:** Require ZIP directory evidence for prefixed archives, preventing Python 3.9 from misclassifying incidental EOCD bytes in bytecode; raw binary inspection remains complete and recognized corrupt archives still fail closed with format-specific errors. Add reviewed path-glob × generic-entropy exceptions pinned to exact file SHA256; enforce an explicit exact-rule allowlist so provider credentials, PATs, private keys and secret assignments remain strict everywhere. Add cross-version decoder and policy regressions. SA-01 review required before adoption; no Ornn exceptions included.

- **1.0.1 — 2026-11-17:** Fix SA-01 R1 by exporting opaque path hashes on every outcome, including detector errors; fix R2 by recursively decoding ZIP, gzip, tar, bzip2, xz and zstd by content signatures, with fail-closed handling for malformed/encrypted/unsupported containers and resource limits. Add direct shipped-wrapper and partial-detector regressions. Supersedes rejected 1.0.0-review-candidate; SA-01 re-verification required.

The unreleased 1.0.4 review correction also binds exact parser-marker and npm-placeholder proofs to package identity and complete file bytes; all unreviewed key-shaped material continues to block.

The same correction adds exact Omarchy parser markers, ancestry-bound static-library proofs and generated-MIME record/input checks; no new generic allowance or detector bypass is introduced.

The report adapter indexes exact staged-file aliases and caches each staged file digest, preserving distinct findings and rejecting ambiguous/unknown aliases while avoiding quadratic report processing on large images.
