use super::*;
use crate::{
    config::{DocumentStorageConfig, FeatureFlags},
    document_storage::maintenance::StorageMaintenance,
};
use mongodb::Database;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Arc,
};

pub(crate) async fn execute(
    database: &Database,
    arguments: &[String],
    command_deadline: Instant,
) -> Result<serde_json::Value, &'static str> {
    let mut lookup = |key: &str| {
        std::env::var(key).map(Some).or_else(|error| match error {
            std::env::VarError::NotPresent => Ok(None),
            std::env::VarError::NotUnicode(_) => {
                Err("configuration must contain Unicode".to_owned())
            }
        })
    };
    let storage = DocumentStorageConfig::from_lookup(&mut lookup)
        .map_err(|_| "invalid document storage configuration")?;
    let flags =
        FeatureFlags::from_lookup(&mut lookup).map_err(|_| "invalid feature configuration")?;
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| "database URI is required")?;
    let context = digest(
        format!(
            "{}\n{}\n{}",
            uri,
            database.name(),
            storage
                .as_ref()
                .map_or("unconfigured", |v| v.bucket.as_str())
        )
        .as_bytes(),
    );
    let blobs: Option<Arc<dyn DocumentBlobMaintenance>> = storage
        .map(|config| {
            StorageMaintenance::new(config, flags.document_object_deletion)
                .map(|v| v as Arc<dyn DocumentBlobMaintenance>)
        })
        .transpose()
        .map_err(|_| "document maintenance unavailable")?;
    let store = mongo::MongoReconcileStore::new(database);
    let args: Vec<_> = arguments.iter().map(String::as_str).collect();
    match args.first().copied() {
        Some("--document-quota-reconcile") => {
            let (path, owner, resume) = parse_reconcile(&args)?;
            let now = mongodb::bson::DateTime::now().timestamp_millis();
            let mut report = if resume {
                read_report(path)?
            } else {
                ReconcileReport::new(owner.clone(), now)
            };
            if resume
                && (report.owner != owner
                    || report.context != context
                    || report.schema_version != 1)
            {
                return Err("report scope does not match this database, storage or owner");
            }
            if resume {
                report.note_resume();
            }
            report.context = context;
            // Validate the destination before issuing potentially expensive reads.
            if !resume {
                write_report(path, &report, false)?;
            }
            let result = reconcile_until(
                &store,
                blobs.as_deref(),
                &mut report,
                checkpoint_deadline(command_deadline),
            )
            .await;
            // Preserve the last completed unit even when the next query failed.
            write_report(path, &report, true)?;
            result.map_err(|_| "reconciliation unavailable; resume the retained private report")?;
            Ok(report.summary())
        }
        Some("--document-quota-orphan-cleanup") => {
            let [_, path, mode @ ("--dry-run" | "--apply")] = args.as_slice() else {
                return Err("orphan cleanup requires a report and explicit --dry-run or --apply");
            };
            let report = read_report(path)?;
            if report.schema_version != 1 || report.context != context {
                return Err("report belongs to a different database or object repository");
            }
            let apply = *mode == "--apply";
            let bytes = serde_json::to_vec(&report).map_err(|_| "invalid report")?;
            let blobs = blobs
                .as_deref()
                .ok_or("document maintenance is unconfigured")?;
            let report_digest = digest(&bytes);
            if apply {
                append_audit(
                    path,
                    &serde_json::json!({
                        "schema_version": 1, "canonical_report_sha256": report_digest,
                        "at_ms": mongodb::bson::DateTime::now().timestamp_millis(),
                        "action": "apply", "outcome": "intent", "candidate_count": report.orphans.len()
                    }),
                )?;
            }
            let result = cleanup(&store, blobs, &report, apply).await;
            if apply {
                append_audit(
                    path,
                    &serde_json::json!({
                        "schema_version": 1, "canonical_report_sha256": report_digest,
                        "at_ms": mongodb::bson::DateTime::now().timestamp_millis(),
                        "action": "apply", "outcome": if result.is_ok() { "complete" } else { "refused_or_failed" },
                        "deleted": result.as_ref().ok().and_then(|v| v.get("deleted"))
                    }),
                )?;
            }
            result.map_err(|_| "orphan cleanup refused; ledger must be ready and every key must remain unledgered and unreferenced")
        }
        _ => Err("unsupported document maintenance command"),
    }
}
fn parse_reconcile<'a>(
    args: &'a [&'a str],
) -> Result<(&'a str, Option<String>, bool), &'static str> {
    let mut path = None;
    let mut owner = None;
    let mut resume = false;
    let mut index = 1;
    while index < args.len() {
        match args[index] {
            "--report" if path.is_none() && index + 1 < args.len() => {
                path = Some(args[index + 1]);
                index += 2;
            }
            "--owner"
                if owner.is_none() && index + 1 < args.len() && valid_digest(args[index + 1]) =>
            {
                owner = Some(args[index + 1].into());
                index += 2;
            }
            "--resume" if !resume => {
                resume = true;
                index += 1;
            }
            _ => {
                return Err(
                    "reconcile requires --report <private-path> [--owner <hash>] [--resume]",
                )
            }
        }
    }
    Ok((
        path.filter(|v| !v.is_empty())
            .ok_or("a private report path is required")?,
        owner,
        resume,
    ))
}
fn read_report(path: &str) -> Result<ReconcileReport, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "private report unavailable")?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > MAX_REPORT_BYTES as u64
    {
        return Err("report must be a private regular file of bounded size");
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "private report unavailable")?
        .take(MAX_REPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "private report unreadable")?;
    if bytes.len() > MAX_REPORT_BYTES {
        return Err("report exceeds its bounded size");
    }
    let report: ReconcileReport =
        serde_json::from_slice(&bytes).map_err(|_| "invalid reconciliation report")?;
    if report.owner.as_ref().is_some_and(|v| !valid_digest(v))
        || report.owners.keys().any(|v| !valid_digest(v))
        || report.orphans.iter().any(|v| parse_key(v).is_err())
        || report.cursor.phase > 3
    {
        return Err("invalid reconciliation report scope");
    }
    Ok(report)
}
fn write_report(path: &str, report: &ReconcileReport, replace: bool) -> Result<(), &'static str> {
    let mut persisted = report.clone();
    persisted.aggregate = report.summary();
    let bytes = serde_json::to_vec_pretty(&persisted).map_err(|_| "report serialization failed")?;
    if bytes.len() > MAX_REPORT_BYTES {
        return Err("report exceeds its bounded size; reconcile individual owners");
    }
    if !replace {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| "new private report path must not already exist; use --resume")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "private report could not be saved")?;
        return Ok(());
    }
    let path = Path::new(path);
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|_| "private report temporary file unavailable")?;
    let result = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err("private report could not be saved");
    }
    Ok(())
}

fn append_audit(report_path: &str, value: &serde_json::Value) -> Result<(), &'static str> {
    let path = format!("{report_path}.audit");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| "private cleanup audit unavailable; deletion refused")?;
    let opened = file
        .metadata()
        .map_err(|_| "cleanup audit metadata unavailable")?;
    let named =
        std::fs::symlink_metadata(&path).map_err(|_| "cleanup audit metadata unavailable")?;
    if !named.is_file()
        || !opened.is_file()
        || opened.permissions().mode() & 0o077 != 0
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
        || opened.len() > MAX_REPORT_BYTES as u64
    {
        return Err("cleanup audit must be a private regular file of bounded size");
    }
    let mut bytes = serde_json::to_vec(value).map_err(|_| "cleanup audit serialization failed")?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "cleanup audit could not be persisted; retain any intent as uncertain")
}

fn checkpoint_deadline(command_deadline: Instant) -> Instant {
    command_deadline - Duration::from_secs(5)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconcile_private_report_persists_checks_and_refuses_public_permissions() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".reconcile-report-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let report_path = directory.join("report.json");
        let path = report_path.to_str().unwrap();
        write_report(path, &ReconcileReport::new(None, 0), false).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let saved = read_report(path).unwrap();
        assert_eq!(saved.aggregate["invariants"]["I1"], "NOT_CHECKED");
        assert!(write_report(path, &saved, false).is_err());
        write_report(path, &saved, true).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_report(path).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn orphan_cleanup_audit_is_private_and_contains_only_aggregate_authority() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".reconcile-audit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let report = directory.join("report.json");
        let path = report.to_str().unwrap();
        let digest = "a".repeat(64);
        append_audit(path, &serde_json::json!({"canonical_report_sha256": digest, "outcome": "intent", "candidate_count": 1})).unwrap();
        append_audit(path, &serde_json::json!({"canonical_report_sha256": digest, "outcome": "complete", "deleted": 1})).unwrap();
        let audit = directory.join("report.json.audit");
        assert_eq!(
            std::fs::metadata(&audit).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let bytes = std::fs::read_to_string(&audit).unwrap();
        assert_eq!(bytes.lines().count(), 2);
        for forbidden in ["subject", "title", "markdown", "documents/"] {
            assert!(!bytes.contains(forbidden));
        }
        std::fs::set_permissions(&audit, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(append_audit(path, &serde_json::json!({"outcome": "intent"})).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn reconcile_checkpoint_deadline_includes_preflight_time() {
        let started = Instant::now();
        let command_deadline = started + Duration::from_secs(60);
        tokio::time::advance(Duration::from_secs(20)).await;
        assert_eq!(
            checkpoint_deadline(command_deadline),
            started + Duration::from_secs(55)
        );
        assert_eq!(
            checkpoint_deadline(command_deadline) - Instant::now(),
            Duration::from_secs(35)
        );
    }
}
