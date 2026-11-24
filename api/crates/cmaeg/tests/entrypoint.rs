//! Process argument dispatch: unknown arguments are a usage error, never a panic.
use std::{ffi::OsStr, process::Output};

const USAGE: &str = "usage: cmaeg [--replica-drain | --replica-list-unresolved | --replica-abandon-unknown --dry-run|--apply --partition ID --head-work ID --head-version N --request-id UUID --reason REASON [--plan-digest DIGEST] | --replica-abandonment-audit --request-id UUID | --print-openapi | --migrate | --attest-activity-history FROM UNTIL_OR_OPEN REVISION --all-writers-capable | --backfill-activity-history | --migrate-activity-history | --print-activity-history-runtime-role | --print-activity-history-migration-role | --migrate-console-sessions | --print-console-sessions-runtime-role | --print-console-sessions-migration-role | --migrate-replicas | --print-runtime-role | --print-migration-role | --migrate-channel-create-fence-floor --all-writers-drained | --print-channel-create-fence-runtime-role | --print-channel-create-fence-migration-role | --migrate-worker-credentials | --print-worker-credentials-runtime-role | --print-worker-credentials-migration-role | --attest-owner-key-cutover RELEASE --all-writers-drained | --migrate-automation | --print-automation-runtime-role | --print-automation-migration-role | --migrate-thread-fences | --print-thread-fences-runtime-role | --print-thread-fences-migration-role | --migrate-media-continuations | --print-media-continuations-runtime-role | --print-media-continuations-migration-role | --migrate-whatsapp-forms | --print-whatsapp-forms-runtime-role | --print-whatsapp-forms-migration-role | --import-legacy-records PATH | --audit-custom-attachments [--clear] | --audit-routing [--clear [--allow-default-widening]] | --media-job <op> <bin-dir> <job-dir> [arg] | --media-sandbox-check | --media-self-test <fixtures-dir> | --allocator-self-test]\n";

fn run(arguments: &[&OsStr]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .args(arguments)
        .env_clear()
        .output()
        .unwrap()
}

fn assert_usage(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stderr), USAGE);
}

#[test]
fn unknown_arguments_print_the_usage_line_and_exit_two() {
    assert_usage(&run(&[OsStr::new("--serve")]));
    assert_usage(&run(&[OsStr::new("--print-openapi"), OsStr::new("extra")]));
    for command in [
        "--replica-drain",
        "--allocator-self-test",
        "--migrate",
        "--migrate-console-sessions",
        "--print-console-sessions-runtime-role",
        "--print-console-sessions-migration-role",
        "--migrate-replicas",
        "--print-runtime-role",
        "--print-migration-role",
        "--migrate-worker-credentials",
        "--print-worker-credentials-runtime-role",
        "--print-worker-credentials-migration-role",
        "--migrate-media-continuations",
        "--print-media-continuations-runtime-role",
        "--print-media-continuations-migration-role",
        "--migrate-whatsapp-forms",
        "--print-whatsapp-forms-runtime-role",
        "--print-whatsapp-forms-migration-role",
    ] {
        assert_usage(&run(&[OsStr::new(command), OsStr::new("extra")]));
    }
    for arguments in [
        &["--attest-owner-key-cutover"][..],
        &["--attest-owner-key-cutover", "release"],
        &[
            "--attest-owner-key-cutover",
            "release",
            "--wrong-drain-flag",
        ],
        &[
            "--attest-owner-key-cutover",
            "release",
            "--all-writers-drained",
            "extra",
        ],
    ] {
        assert_usage(&run_strs(arguments));
    }
    assert_usage(&run(&[OsStr::new("--import-legacy-records")]));
    assert_usage(&run(&[
        OsStr::new("--import-legacy-records"),
        OsStr::new("input.json"),
        OsStr::new("extra"),
    ]));
}

/// Only the three documented forms run the routing audit; the override is
/// accepted only after `--clear`, and nothing may follow it.
#[test]
fn the_routing_audit_accepts_only_its_documented_forms() {
    for arguments in [
        &["--audit-routing", "--allow-default-widening"][..],
        &["--audit-routing", "--allow-default-widening", "--clear"],
        &["--audit-routing", "--clear", "--clear"],
        &[
            "--audit-routing",
            "--clear",
            "--allow-default-widening",
            "extra",
        ],
        &["--audit-routing", "--bogus"],
        &["--allow-default-widening"],
        &["--clear", "--audit-routing"],
    ] {
        assert_usage(&run_strs(arguments));
    }
}

/// Each documented form is dispatched to the audit, which fails on the
/// cleared environment as a classified stop rather than a usage error.
#[test]
fn every_routing_audit_form_runs_the_audit() {
    for arguments in [
        &["--audit-routing"][..],
        &["--audit-routing", "--clear"],
        &["--audit-routing", "--clear", "--allow-default-widening"],
    ] {
        let output = run_strs(arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).lines().last(),
            Some("configuration invalid"),
            "{arguments:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_a_usage_error_instead_of_a_panic() {
    use std::os::unix::ffi::OsStrExt;

    assert_usage(&run(&[OsStr::from_bytes(b"--print-openapi\xff")]));
    assert_usage(&run(&[
        OsStr::new("--print-openapi"),
        OsStr::from_bytes(b"\xff"),
    ]));
}

fn run_strs(arguments: &[&str]) -> Output {
    let arguments: Vec<&OsStr> = arguments.iter().map(OsStr::new).collect();
    run(&arguments)
}

/// A media job that cannot run is `sandbox_unavailable` to the gateway: exit
/// 125 with nothing on stdout, never the usage line's 2 or a panic's 101.
#[test]
fn a_malformed_media_job_exits_125_without_executing() {
    for arguments in [
        &["--media-job"][..],
        &[
            "--media-job",
            "frame",
            "/opt/ff",
            "/var/cmaeg/media/a",
            "1.5",
        ],
        &["--media-job", "probe", "opt/ff", "/var/cmaeg/media/a"],
        &[
            "--media-job",
            "sandbox-check",
            "/opt/ff",
            "/var/cmaeg/media/a",
        ],
        &["--media-job", "audio", "/opt/ff", "/var/cmaeg/media/a", "0"],
    ] {
        let output = run_strs(arguments);
        assert_eq!(output.status.code(), Some(125), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_non_utf8_media_job_argument_exits_125() {
    use std::os::unix::ffi::OsStrExt;

    let output = run(&[
        OsStr::new("--media-job"),
        OsStr::new("probe"),
        OsStr::new("/opt/ff"),
        OsStr::from_bytes(b"/var/cmaeg/media/\xff"),
    ]);
    assert_eq!(output.status.code(), Some(125));
    assert!(output.stdout.is_empty());
}

#[test]
fn the_sandbox_check_takes_no_arguments() {
    assert_usage(&run_strs(&["--media-sandbox-check", "extra"]));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn allocator_qualification_without_procfs_fails_before_serving() {
    let output = run_strs(&["--allocator-self-test"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.starts_with("allocator-self-test outcome=failed error="));
    assert!(!error.contains("configuration invalid"));
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn the_sandbox_check_fails_where_no_sandbox_exists() {
    let output = run_strs(&["--media-sandbox-check"]);
    assert_eq!(output.status.code(), Some(125));
    assert!(output.stdout.is_empty());
}

#[test]
fn the_self_test_takes_exactly_one_fixtures_directory() {
    assert_usage(&run_strs(&["--media-self-test"]));
    assert_usage(&run_strs(&["--media-self-test", "/fixtures", "extra"]));
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn the_self_test_fails_where_no_sandbox_exists() {
    let output = run_strs(&["--media-self-test", "/fixtures"]);
    assert_eq!(output.status.code(), Some(125));
    assert!(output.stdout.is_empty());
}

#[test]
fn replica_operator_commands_reject_ambiguous_or_incomplete_arguments_before_io() {
    for arguments in [
        vec!["--replica-list-unresolved", "--apply"],
        vec!["--replica-list-unresolved", "--unexpected", "value"],
        vec!["--replica-abandonment-audit"],
        vec!["--replica-abandon-unknown"],
        vec!["--replica-abandon-unknown", "--apply", "--dry-run"],
        vec![
            "--replica-abandon-unknown",
            "--apply",
            "--partition",
            "p",
            "--head-work",
            "w",
            "--head-version",
            "1",
            "--request-id",
            "id",
            "--reason",
            "r",
        ],
    ] {
        assert_usage(&run_strs(&arguments));
    }
}
