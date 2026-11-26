//! CLI diagnostics must never echo database configuration or sealed record bytes.
use mongodb::bson::{doc, Document};
use std::{ffi::OsStr, path::PathBuf, process::Output};

const DATABASE: &str = "CMAEG_MONGODB_DATABASE";
const URI: &str = "CMAEG_MONGODB_URI";
const LOCAL_URI: &str = "mongodb://127.0.0.1:1/?serverSelectionTimeoutMS=1";
const COMMANDS: &[&[&str]] = &[
    &["--migrate"],
    &["--migrate-activity-history"],
    &["--migrate-whatsapp-forms"],
    &["--migrate-media-continuations"],
    &["--import-legacy-records", "SYNTHETIC_PRIVATE_EXPORT_PATH"],
];

fn run(arguments: &[&str], environment: &[(&str, &OsStr)]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .args(arguments)
        .env_clear()
        .envs(environment.iter().copied())
        .output()
        .unwrap()
}

fn failed(output: Output, diagnostic: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("{diagnostic}\n")
    );
}

#[test]
fn missing_database_configuration_is_classified_for_both_commands() {
    for arguments in COMMANDS {
        failed(
            run(arguments, &[]),
            "CMAEG_MONGODB_DATABASE is missing or not valid UTF-8",
        );
        failed(
            run(arguments, &[(DATABASE, OsStr::new("fixture"))]),
            "CMAEG_MONGODB_URI is missing or not valid UTF-8",
        );
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_database_configuration_is_not_echoed_for_both_commands() {
    use std::os::unix::ffi::OsStrExt;
    let private = OsStr::from_bytes(b"SYNTHETIC_PRIVATE_CONFIGURATION_\xff");
    for arguments in COMMANDS {
        failed(
            run(arguments, &[(DATABASE, private)]),
            "CMAEG_MONGODB_DATABASE is missing or not valid UTF-8",
        );
        failed(
            run(
                arguments,
                &[(DATABASE, OsStr::new("fixture")), (URI, private)],
            ),
            "CMAEG_MONGODB_URI is missing or not valid UTF-8",
        );
    }
}

#[test]
fn malformed_client_configuration_is_not_echoed_for_both_commands() {
    for arguments in COMMANDS {
        failed(
            run(
                arguments,
                &[
                    (DATABASE, OsStr::new("fixture")),
                    (
                        URI,
                        OsStr::new(
                            "mongodb://127.0.0.1:1/?authMechanism=SYNTHETIC_PRIVATE_MECHANISM",
                        ),
                    ),
                ],
            ),
            "Gateway database client configuration failed; details withheld",
        );
    }
}

#[test]
fn role_commands_still_print_json_without_a_database_uri() {
    for command in ["--print-runtime-role", "--print-migration-role"] {
        let output = run(&[command], &[(DATABASE, OsStr::new("fixture"))]);
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        let role: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            role,
            serde_json::to_value(cmaeg::records::mongo::role(
                "fixture",
                command == "--print-migration-role"
            ))
            .unwrap()
        );
    }
}

struct Export(PathBuf);
impl Export {
    fn new(bytes: &[u8]) -> Self {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("cma-cli-{}.ndjson", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        Self(path)
    }
    fn run(&self) -> Output {
        run(
            &["--import-legacy-records", self.0.to_str().unwrap()],
            &[
                (DATABASE, OsStr::new("fixture")),
                (URI, OsStr::new(LOCAL_URI)),
            ],
        )
    }
}
impl Drop for Export {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.0) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => panic!("cannot remove synthetic export: {error}"),
        }
    }
}

#[test]
fn rejected_export_does_not_echo_private_record_bytes() {
    failed(
        Export::new(b"SYNTHETIC_PRIVATE_RECORD\n").run(),
        "Gateway record import rejected; check export or target records",
    );
}

#[test]
fn unreadable_export_preserves_safe_diagnostic() {
    let file = Export::new(b"");
    std::fs::remove_file(&file.0).unwrap();
    failed(file.run(), "record export is unreadable");
}

#[test]
fn empty_export_preserves_successful_count_output_without_a_database() {
    let output = Export::new(b"").run();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, b"Imported and verified 0 sealed records\n");
}

#[test]
fn valid_export_checks_writer_authority_even_when_every_record_is_expired() {
    let record = serde_json::json!({
        "kind": "interaction",
        "record_id": "00000000-0000-0000-0000-000000000001",
        "owner": "fixture",
        "version": "1",
        "blob": "AQ==",
        "expires_at": 1,
    });
    failed(
        Export::new(record.to_string().as_bytes()).run(),
        "Gateway record import unavailable; details withheld",
    );
}

fn expected_optional_role(mut component_role: Document, migration: bool) -> serde_json::Value {
    if migration {
        // The CLI adds custody of the execution ledger and catalog discovery
        // without broadening the component's collection permissions.
        component_role.get_array_mut("privileges").unwrap().extend([
            doc! {
                "resource": {"db": "fixture", "collection": "eg_migration_ledger"},
                "actions": ["find", "insert", "update", "createCollection"],
            }
            .into(),
            doc! {
                "resource": {"db": "fixture", "collection": ""},
                "actions": ["listCollections"],
            }
            .into(),
        ]);
    }
    serde_json::to_value(component_role).unwrap()
}

#[test]
fn optional_forms_roles_are_additive_and_never_grant_runtime_delete() {
    for (command, migration) in [
        ("--print-whatsapp-forms-runtime-role", false),
        ("--print-whatsapp-forms-migration-role", true),
    ] {
        let output = run(&[command], &[(DATABASE, OsStr::new("fixture"))]);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let role: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            role,
            expected_optional_role(
                cmaeg::records::mongo::whatsapp_flows_role("fixture", migration),
                migration,
            )
        );
        assert!(!role.to_string().contains("remove"));
    }
}

#[test]
fn optional_media_continuation_roles_stay_inside_the_gateway_database() {
    for (command, migration) in [
        ("--print-media-continuations-runtime-role", false),
        ("--print-media-continuations-migration-role", true),
    ] {
        let output = run(&[command], &[(DATABASE, OsStr::new("fixture"))]);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let role: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            role,
            expected_optional_role(
                cmaeg::records::mongo::media_continuations_role("fixture", migration),
                migration,
            )
        );
        let grants = role["privileges"].as_array().unwrap();
        assert_eq!(grants.len(), if migration { 4 } else { 1 });
        for grant in grants.iter().take(if migration { 2 } else { 1 }) {
            assert_eq!(grant["resource"]["db"], "fixture");
            assert!(["eg_media_continuation_records", "eg_schema_migrations"]
                .contains(&grant["resource"]["collection"].as_str().unwrap()));
        }
        assert_eq!(role.to_string().contains("remove"), !migration);
    }
}
