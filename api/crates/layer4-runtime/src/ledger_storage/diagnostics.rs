//! Fixed classifications only: never format paths, identities or error payloads.
use std::os::unix::fs::MetadataExt;

use crate::failure::RuntimeError;

pub(super) fn io(
    predicate: &'static str,
    operation: &'static str,
    object_role: &'static str,
    error: &std::io::Error,
    classify: fn() -> RuntimeError,
) -> RuntimeError {
    tracing::error!(predicate, operation, object_role,
        error_kind = ?error.kind(), raw_os_error = ?error.raw_os_error(),
        "runtime ledger storage IO refused");
    classify()
}

pub(super) fn metadata(
    predicate: &'static str,
    operation: &'static str,
    object_role: &'static str,
    metadata: &std::fs::Metadata,
) -> RuntimeError {
    tracing::error!(predicate, operation, object_role,
        mode_octal = %format_args!("{:04o}", metadata.mode() & 0o7777),
        group_other_bits = metadata.mode() & 0o077, required_group_other_bits = 0_u32,
        regular_file = metadata.is_file(), directory = metadata.is_dir(),
        single_link = metadata.nlink() == 1,
        effective_owner = metadata.uid() == unsafe { libc::geteuid() },
        "runtime ledger storage metadata refused");
    super::invalid()
}

pub(super) fn predicate(
    predicate: &'static str,
    operation: &'static str,
    object_role: &'static str,
    classify: fn() -> RuntimeError,
) -> RuntimeError {
    tracing::error!(
        predicate,
        operation,
        object_role,
        "runtime ledger storage proof refused"
    );
    classify()
}

pub(super) fn sqlite(
    predicate: &'static str,
    error: &rusqlite::Error,
    classify: fn() -> RuntimeError,
) -> RuntimeError {
    tracing::error!(predicate, operation = "inspect_closed", object_role = "database",
        sqlite_extended_code = ?error.sqlite_error().map(|error| error.extended_code),
        "runtime ledger storage SQLite inspection refused");
    classify()
}
