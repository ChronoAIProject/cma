use cmaeg::{
    channels::audit_routing::RoutingAuditMode,
    media::transform::{
        argv::{MEDIA_JOB_FLAG as MEDIA_JOB, SANDBOX_CHECK_FLAG as SANDBOX_CHECK},
        self_test::SELF_TEST_FLAG as SELF_TEST,
    },
};
use std::{ffi::OsString, io::Write, path::Path};

use cma_allocator_qualification as allocator_self_test;

const USAGE: &str = "usage: cmaeg [--replica-drain | --replica-list-unresolved | --replica-abandon-unknown --dry-run|--apply --partition ID --head-work ID --head-version N --request-id UUID --reason REASON [--plan-digest DIGEST] | --replica-abandonment-audit --request-id UUID | --print-openapi | --migrate | --attest-activity-history FROM UNTIL_OR_OPEN REVISION --all-writers-capable | --backfill-activity-history | --migrate-activity-history | --print-activity-history-runtime-role | --print-activity-history-migration-role | --migrate-console-sessions | --print-console-sessions-runtime-role | --print-console-sessions-migration-role | --migrate-replicas | --print-runtime-role | --print-migration-role | --migrate-channel-create-fence-floor --all-writers-drained | --print-channel-create-fence-runtime-role | --print-channel-create-fence-migration-role | --migrate-worker-credentials | --print-worker-credentials-runtime-role | --print-worker-credentials-migration-role | --attest-owner-key-cutover RELEASE --all-writers-drained | --migrate-automation | --print-automation-runtime-role | --print-automation-migration-role | --migrate-thread-fences | --print-thread-fences-runtime-role | --print-thread-fences-migration-role | --migrate-media-continuations | --print-media-continuations-runtime-role | --print-media-continuations-migration-role | --migrate-whatsapp-forms | --print-whatsapp-forms-runtime-role | --print-whatsapp-forms-migration-role | --import-legacy-records PATH | --audit-custom-attachments [--clear] | --audit-routing [--clear [--allow-default-widening]] | --media-job <op> <bin-dir> <job-dir> [arg] | --media-sandbox-check | --media-self-test <fixtures-dir> | --allocator-self-test]";

fn main() {
    // `args_os` never panics: `args` aborts with exit 101 on a non-UTF-8
    // argument before any usage handling could run.
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    // The media job modes come first and are matched on the raw arguments:
    // a malformed job must exit 125 (sandbox unavailable), never 2 or 101.
    // Neither reads the environment, logs or builds a runtime, and a job must
    // not inherit the gateway's non-dumpable flag.
    match arguments.first().map(OsString::as_os_str) {
        Some(flag) if flag == MEDIA_JOB => {
            cmaeg::media::transform::job_main(arguments[1..].to_vec())
        }
        Some(flag) if flag == SANDBOX_CHECK && arguments.len() == 1 => {
            cmaeg::media::transform::sandbox_check_main()
        }
        _ => {}
    }
    // Then, before any other mode, log line or runtime: every other mode
    // holds or can load the gateway's secrets.
    if let Err(error) = cmaeg::hardening::disable_dumping() {
        eprintln!("cmaeg: cannot make the process non-dumpable: {error}");
        std::process::exit(1);
    }
    // Modes that need no runtime are matched on the raw arguments first, so
    // they run before one exists. The self-test builds its own, and takes
    // its fixtures path as given, UTF-8 or not.
    if arguments == ["--print-openapi"] {
        print!("{}", cmaeg::openapi::snapshot());
        return;
    }
    if arguments == ["--allocator-self-test"] {
        if let Err(error) = allocator_self_test::run() {
            eprintln!("allocator-self-test outcome=failed error={error}");
            std::process::exit(1);
        }
        return;
    }
    if let [flag, fixtures] = arguments.as_slice() {
        if flag == SELF_TEST {
            media_self_test(Path::new(fixtures));
        }
    }
    let Some(arguments) = arguments
        .iter()
        .map(|argument| argument.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        usage();
    };
    match arguments.as_slice() {
        [] => serve(),
        ["--replica-drain"] => replica_drain(),
        ["--attest-activity-history", from, until, revision, "--all-writers-capable"] => {
            history_admin(Some((from, until, revision)))
        }
        ["--backfill-activity-history"] => history_admin(None),
        ["--migrate-channel-create-fence-floor", "--all-writers-drained"] => {
            run_database_command("--migrate-channel-create-fence-floor")
        }
        [command, ..]
            if matches!(
                *command,
                "--replica-list-unresolved"
                    | "--replica-abandon-unknown"
                    | "--replica-abandonment-audit"
            ) =>
        {
            if !cmaeg::replicas::valid_operator_args(&arguments) {
                usage();
            }
            let runtime = audit_runtime();
            if let Err(error) = runtime.block_on(cmaeg::replicas::operator_cli(&arguments)) {
                fail(error);
            }
        }
        [command @ ("--migrate"
        | "--migrate-replicas"
        | "--migrate-activity-history"
        | "--print-activity-history-runtime-role"
        | "--print-activity-history-migration-role"
        | "--migrate-console-sessions"
        | "--print-console-sessions-runtime-role"
        | "--print-console-sessions-migration-role"
        | "--print-runtime-role"
        | "--print-migration-role"
        | "--print-channel-create-fence-runtime-role"
        | "--print-channel-create-fence-migration-role"
        | "--migrate-worker-credentials"
        | "--print-worker-credentials-runtime-role"
        | "--print-worker-credentials-migration-role"
        | "--migrate-automation"
        | "--print-automation-runtime-role"
        | "--print-automation-migration-role"
        | "--migrate-thread-fences"
        | "--print-thread-fences-runtime-role"
        | "--print-thread-fences-migration-role"
        | "--migrate-media-continuations"
        | "--print-media-continuations-runtime-role"
        | "--print-media-continuations-migration-role"
        | "--migrate-whatsapp-forms"
        | "--print-whatsapp-forms-runtime-role"
        | "--print-whatsapp-forms-migration-role")] => run_database_command(command),
        ["--import-legacy-records", path] => run_import(path),
        ["--attest-owner-key-cutover", release, "--all-writers-drained"] => {
            run_owner_key_cutover(release)
        }
        ["--audit-custom-attachments"] => audit(false),
        ["--audit-custom-attachments", "--clear"] => audit(true),
        ["--audit-routing"] => audit_routing(RoutingAuditMode::Report),
        ["--audit-routing", "--clear"] => audit_routing(RoutingAuditMode::Clear {
            allow_default_widening: false,
        }),
        ["--audit-routing", "--clear", "--allow-default-widening"] => {
            audit_routing(RoutingAuditMode::Clear {
                allow_default_widening: true,
            })
        }
        _ => usage(),
    }
}

fn history_admin(boundary: Option<(&str, &str, &str)>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| fail("cannot initialize history administration runtime"));
    let result = runtime.block_on(async {
        if let Some((from, until, revision)) = boundary {
            cmaeg::activity::history::admin::run_attestation(from, until, revision).await?;
            println!("activity history coverage updated");
        } else {
            let count = cmaeg::activity::history::admin::run_backfill(None).await?;
            println!("activity history backfill processed {count} surviving receipts; coverage remains partial");
        }
        Ok::<(), cmaeg::activity::history::Error>(())
    });
    if result.is_err() {
        fail("activity history administration failed; details withheld");
    }
}

/// The build-time media self-test. Its report is the only stdout; logs go
/// to stderr.
fn media_self_test(fixtures: &Path) -> ! {
    cmaeg::init_logging_to_stderr();
    cmaeg::install_panic_hook();
    cmaeg::media::transform::self_test::main(fixtures)
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn serve() {
    cmaeg::init_logging();
    cmaeg::install_panic_hook();
    // Both failures exit through `process::exit`: threads a failed prewarm
    // left parked on its barrier end with the process, before any listener binds.
    let runtime = match cmaeg::runtime::build() {
        Ok(runtime) => runtime,
        Err(cmaeg::runtime::RuntimeError::Prewarm) => {
            tracing::error!("CMAEG blocking pool prewarm failed");
            std::process::exit(1);
        }
        Err(error) => {
            tracing::error!(%error, "CMAEG runtime unavailable");
            std::process::exit(1);
        }
    };
    if let Err(error) = runtime.block_on(cmaeg::run()) {
        tracing::error!(%error, "CMAEG stopped");
        std::process::exit(1);
    }
}

fn replica_drain() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| fail("replica drain runtime unavailable"));
    if let Err(error) = runtime.block_on(cmaeg::replicas::drain_cli()) {
        fail(error);
    }
}

/// The custom attachment rollback audit. It exits 0 only when no setting
/// remains and every record could be read.
fn audit(clear: bool) {
    let runtime = audit_runtime();
    let report = match runtime.block_on(cmaeg::audit_custom_attachments(clear)) {
        Ok(report) => report,
        Err(error) => fail(&error.to_string()),
    };
    finish_audit(&report.lines(), report.complete());
}

/// The multi-consumer rollback audit. It exits 0 only when no definition an
/// older image cannot read remains and every record could be read.
fn audit_routing(mode: RoutingAuditMode) {
    let runtime = audit_runtime();
    let report = match runtime.block_on(cmaeg::audit_routing(mode)) {
        Ok(report) => report,
        Err(error) => fail(&error.to_string()),
    };
    finish_audit(&report.lines(), report.complete());
}

/// An audit's logging and runtime. Stdout carries only the JSON report, so
/// logs go to stderr, and an error is printed as its classified name alone.
fn audit_runtime() -> tokio::runtime::Runtime {
    cmaeg::init_logging_to_stderr();
    cmaeg::install_panic_hook();
    match cmaeg::runtime::build() {
        Ok(runtime) => runtime,
        Err(cmaeg::runtime::RuntimeError::Prewarm) => fail("blocking pool prewarm failed"),
        Err(_) => fail("runtime unavailable"),
    }
}

/// Print the report's lines and exit 0 when it is complete, else 1.
fn finish_audit(lines: &[String], complete: bool) -> ! {
    let mut stdout = std::io::stdout().lock();
    let written = lines
        .iter()
        .try_for_each(|line| writeln!(stdout, "{line}"))
        .and_then(|()| stdout.flush());
    if written.is_err() {
        fail("stdout unavailable");
    }
    std::process::exit(if complete { 0 } else { 1 });
}

fn fail(reason: &str) -> ! {
    eprintln!("{reason}");
    std::process::exit(1);
}

fn run_database_command(command: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| fail("cannot initialize database command runtime"));
    if let Err(error) = runtime.block_on(async {
        let database = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE)
            .map_err(|_| "CMAEG_MONGODB_DATABASE is missing or not valid UTF-8")?;
        if !matches!(
            command,
            "--migrate"
                | "--migrate-replicas"
                | "--migrate-activity-history"
                | "--migrate-console-sessions"
                | "--migrate-automation"
                | "--migrate-worker-credentials"
                | "--migrate-thread-fences"
                | "--migrate-whatsapp-forms"
                | "--migrate-media-continuations"
                | "--migrate-channel-create-fence-floor"
        ) {
            println!(
                "{}",
                if command.contains("activity-history") {
                    cmaeg::activity::history::schema::role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else if command.contains("channel-create-fence") {
                    cmaeg::records::mongo::create_fence_role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else if command.contains("console-sessions") {
                    cmaeg::console::session_backend::schema::role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else if command.contains("automation") {
                    if command.ends_with("migration-role") {
                        cmaeg::github::migration_role(&database)
                    } else {
                        cmaeg::github::runtime_role(&database)
                    }
                } else if command.contains("worker-credentials") {
                    cmaeg::worker_credentials::mongo::role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else if command.contains("thread-fences") {
                    cmaeg::records::mongo::thread_fences_role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else if command.contains("media-continuations") {
                    cmaeg::records::mongo::media_continuations_role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else if command.contains("whatsapp-forms") {
                    cmaeg::records::mongo::whatsapp_flows_role(
                        &database,
                        command.ends_with("migration-role"),
                    )
                } else {
                    cmaeg::records::mongo::role(&database, command == "--print-migration-role")
                }
            );
            return Ok::<(), &'static str>(());
        }
        let uri = std::env::var(cmaeg::CMAEG_MONGODB_URI)
            .map_err(|_| "CMAEG_MONGODB_URI is missing or not valid UTF-8")?;
        let client = mongodb::Client::with_uri_str(uri)
            .await
            .map_err(|_| "Gateway database client configuration failed; details withheld")?;
        if command == "--migrate-activity-history" {
            cmaeg::activity::history::schema::migrate(&client.database(&database))
                .await
                .map_err(|_| "Gateway activity history migration failed; details withheld")?;
            return Ok(());
        }
        if command == "--migrate-console-sessions" {
            cmaeg::console::session_backend::schema::migrate(&client.database(&database))
                .await
                .map_err(|_| "Gateway Console session migration failed; details withheld")?;
            return Ok(());
        }
        if command == "--migrate-automation" {
            cmaeg::github::migrate(&client.database(&database))
                .await
                .map_err(|_| "Gateway automation migration failed; details withheld")?;
            return Ok(());
        }
        if command == "--migrate-channel-create-fence-floor" {
            let issuer = std::env::var(cmaeg::CMAEG_NYXID_BASE_URL).map_err(|_| {
                "CMAEG_NYXID_BASE_URL is required for the Channel create fence floor"
            })?;
            cmaeg::records::mongo::migrate_create_fence_floor(
                &client.database(&database),
                issuer.trim_end_matches('/'),
            )
            .await
        } else if command == "--migrate-replicas" {
            cmaeg::records::mongo::migrate_replicas(&client.database(&database)).await
        } else if command == "--migrate-worker-credentials" {
            cmaeg::worker_credentials::mongo::migrate(&client.database(&database)).await
        } else if command == "--migrate-thread-fences" {
            cmaeg::records::mongo::migrate_thread_fences(&client.database(&database)).await
        } else if command == "--migrate-media-continuations" {
            cmaeg::records::mongo::migrate_media_continuations(&client.database(&database)).await
        } else if command == "--migrate-whatsapp-forms" {
            cmaeg::records::mongo::migrate_whatsapp_flows(&client.database(&database)).await
        } else {
            cmaeg::records::mongo::migrate(&client.database(&database)).await
        }
        .map_err(|_| "Gateway schema migration failed; details withheld")?;
        Ok(())
    }) {
        fail(error);
    }
}

fn run_import(path: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| fail("cannot initialize database command runtime"));
    if let Err(error) = runtime.block_on(async {
        let database = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE)
            .map_err(|_| "CMAEG_MONGODB_DATABASE is missing or not valid UTF-8")?;
        let uri = std::env::var(cmaeg::CMAEG_MONGODB_URI)
            .map_err(|_| "CMAEG_MONGODB_URI is missing or not valid UTF-8")?;
        let client = mongodb::Client::with_uri_str(uri)
            .await
            .map_err(|_| "Gateway database client configuration failed; details withheld")?;
        let count =
            cmaeg::records::import::import(&client.database(&database), std::path::Path::new(path))
                .await
                .map_err(import_failure)?;
        println!("Imported and verified {count} sealed records");
        Ok::<(), &'static str>(())
    }) {
        fail(error);
    }
}

// Never display environment/driver errors or unrecognized record error text:
// these commands run with database credentials and a private sealed export.
fn import_failure(error: cmaeg::records::RecordError) -> &'static str {
    use cmaeg::records::RecordError;
    match error {
        RecordError::Rejected(reason)
            if reason == "import conflicts with an existing record; no row was overwritten" =>
        {
            "import conflicts with an existing record; no row was overwritten"
        }
        RecordError::Rejected(reason) if reason == "record export is unreadable" => {
            "record export is unreadable"
        }
        // Fixed policy text, never record bytes: the operator must see which
        // gate refused the row and that the flag or key ring decides it.
        RecordError::Rejected(reason) if reason == cmaeg::records::GITHUB_IMPORT_DISABLED => {
            cmaeg::records::GITHUB_IMPORT_DISABLED
        }
        RecordError::Rejected(reason) if reason == cmaeg::records::GITHUB_IMPORT_KEYS_REQUIRED => {
            cmaeg::records::GITHUB_IMPORT_KEYS_REQUIRED
        }
        RecordError::Rejected(reason) if reason == cmaeg::records::GITHUB_IMPORT_UNREADABLE => {
            cmaeg::records::GITHUB_IMPORT_UNREADABLE
        }
        RecordError::Rejected(_) => {
            "Gateway record import rejected; check export or target records"
        }
        RecordError::Unavailable(_) => "Gateway record import unavailable; details withheld",
        _ => "Gateway record import failed; details withheld",
    }
}

/// Explicit operator statement, never a server startup side effect. Every
/// historical untracked writer/importer must be drained before this command.
fn run_owner_key_cutover(release: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| fail("cannot initialize cutover command runtime"));
    if runtime
        .block_on(async {
            let name = std::env::var(cmaeg::CMAEG_MONGODB_DATABASE).map_err(|_| ())?;
            let uri = std::env::var(cmaeg::CMAEG_MONGODB_URI).map_err(|_| ())?;
            let client = mongodb::Client::with_uri_str(uri).await.map_err(|_| ())?;
            let receipt =
                cmaeg::worker_credentials::cutover::attest(&client.database(&name), release, true)
                    .await
                    .map_err(|_| ())?;
            println!("{}", serde_json::to_string(&receipt).map_err(|_| ())?);
            Ok::<(), ()>(())
        })
        .is_err()
    {
        fail("owner-key cutover failed or uncertain; inspect durable attestation before retry")
    }
}

#[cfg(test)]
mod database_diagnostics_tests {
    use super::import_failure;
    use cmaeg::records::RecordError;

    #[test]
    fn import_diagnostics_preserve_conflicts_but_withhold_unrecognized_text() {
        let conflict = "import conflicts with an existing record; no row was overwritten";
        assert_eq!(
            import_failure(RecordError::Rejected(conflict.into())),
            conflict
        );
        for error in [
            RecordError::Rejected("SYNTHETIC_PRIVATE_RECORD".into()),
            RecordError::Unavailable("SYNTHETIC_PRIVATE_DRIVER_ERROR".into()),
        ] {
            assert!(!import_failure(error).contains("SYNTHETIC_PRIVATE"));
        }
    }
}
