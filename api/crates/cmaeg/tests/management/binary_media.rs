//! Binary-level tests of inbound media modes; `binary.rs` keeps the original journeys.
use super::binary_support::{assert_no_secrets, curl, free_address, spawn_ready, MountedSecrets};
use super::*;
use cmaeg_core::{profile::canonical_json, seal::seal_blob};

const USAGE: &str = "usage: cmaeg [--replica-drain | --replica-list-unresolved | --replica-abandon-unknown --dry-run|--apply --partition ID --head-work ID --head-version N --request-id UUID --reason REASON [--plan-digest DIGEST] | --replica-abandonment-audit --request-id UUID | --print-openapi | --migrate | --attest-activity-history FROM UNTIL_OR_OPEN REVISION --all-writers-capable | --backfill-activity-history | --migrate-activity-history | --print-activity-history-runtime-role | --print-activity-history-migration-role | --migrate-console-sessions | --print-console-sessions-runtime-role | --print-console-sessions-migration-role | --migrate-replicas | --print-runtime-role | --print-migration-role | --migrate-channel-create-fence-floor --all-writers-drained | --print-channel-create-fence-runtime-role | --print-channel-create-fence-migration-role | --migrate-worker-credentials | --print-worker-credentials-runtime-role | --print-worker-credentials-migration-role | --attest-owner-key-cutover RELEASE --all-writers-drained | --migrate-automation | --print-automation-runtime-role | --print-automation-migration-role | --migrate-thread-fences | --print-thread-fences-runtime-role | --print-thread-fences-migration-role | --migrate-media-continuations | --print-media-continuations-runtime-role | --print-media-continuations-migration-role | --migrate-whatsapp-forms | --print-whatsapp-forms-runtime-role | --print-whatsapp-forms-migration-role | --import-legacy-records PATH | --audit-custom-attachments [--clear] | --audit-routing [--clear [--allow-default-widening]] | --media-job <op> <bin-dir> <job-dir> [arg] | --media-sandbox-check | --media-self-test <fixtures-dir> | --allocator-self-test]\n";
const OWNER: &str = "binary-audit-owner-fixture";

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run the compiled gateway's audit mode against the fakes at `RUST_LOG=trace`.
async fn audit(
    files: &MountedSecrets,
    fakes: (&FakeCma, &FakeNyxid, &FakeProvider),
    arguments: &[&str],
) -> Run {
    let (cma, nyxid, provider) = fakes;
    let output = files
        .command("127.0.0.1:0", cma, nyxid, provider)
        .env("RUST_LOG", "trace")
        .args(arguments)
        .output()
        .await
        .unwrap();
    let run = Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    };
    for text in [&run.stdout, &run.stderr] {
        assert_no_secrets(text);
        assert!(!text.contains(OWNER), "owner subject leaked");
        assert!(!text.contains("files.example"), "definition leaked");
    }
    run
}

/// Store one custom channel carrying the attachments setting, sealed with the
/// mounted master key, and return its record ID.
fn seed_setting(cma: &FakeCma) -> String {
    let record_id = uuid::Uuid::new_v4().to_string();
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["record_id"] = record_id.clone().into();
    value["owner"]["subject"] = OWNER.into();
    value["sources"][0]["attachments"] = json!({"origins":["https://files.example"]});
    let definition: cmaeg_core::channel::ChannelDefinition = serde_json::from_value(value).unwrap();
    let blob = seal_blob(
        &key_ring(1),
        canonical_json(definition.to_blob_json()).as_bytes(),
    )
    .unwrap();
    cma.seed(&record_id, OWNER, blob);
    record_id
}

#[tokio::test]
async fn audit_usage_errors_exit_two_with_the_usage_line() {
    for arguments in [
        &["--audit-custom-attachments", "--bogus"][..],
        &["--audit-custom-attachments", "--clear", "--clear"],
        &["--clear"],
    ] {
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
            .args(arguments)
            .env_clear()
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(String::from_utf8_lossy(&output.stderr), USAGE);
    }
}

#[tokio::test]
async fn audit_without_findings_exits_zero_and_binds_nothing() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let run = audit(
        &files,
        (&cma, &nyxid, &provider),
        &["--audit-custom-attachments"],
    )
    .await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        "{\"scanned\":0,\"channels\":0,\"unreadable\":0,\"found\":0,\"cleared\":0,\"remaining\":0}\n"
    );
    assert!(!run.stderr.contains("CMAEG listening"));
    assert_eq!(cma.lists.load(std::sync::atomic::Ordering::SeqCst), 1);
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn audit_reports_a_setting_with_exit_one_until_it_is_cleared() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let record_id = seed_setting(&cma);
    let fakes = (&cma, &nyxid, &provider);

    let found = audit(&files, fakes, &["--audit-custom-attachments"]).await;
    assert_eq!(found.code, Some(1), "{}", found.stderr);
    assert_eq!(
        found.stdout,
        format!(
            "{{\"record_id\":\"{record_id}\",\"channel_hash\":null,\"action\":\"found\"}}\n\
             {{\"scanned\":1,\"channels\":1,\"unreadable\":0,\"found\":1,\"cleared\":0,\"remaining\":1}}\n"
        )
    );
    assert_eq!(cma.records()[record_id.as_str()].version, 1);

    let cleared = audit(&files, fakes, &["--audit-custom-attachments", "--clear"]).await;
    assert_eq!(cleared.code, Some(0), "{}", cleared.stderr);
    assert!(cleared.stdout.starts_with(&format!(
        "{{\"record_id\":\"{record_id}\",\"channel_hash\":null,\"action\":\"cleared\"}}\n"
    )));
    assert!(cleared
        .stdout
        .ends_with("\"found\":1,\"cleared\":1,\"remaining\":0}\n"));
    assert!(cleared.stderr.contains("custom attachments cleared"));
    assert!(cleared.stderr.contains(&record_id));

    let again = audit(&files, fakes, &["--audit-custom-attachments"]).await;
    assert_eq!(again.code, Some(0), "{}", again.stderr);
    assert_eq!(
        again.stdout,
        "{\"scanned\":1,\"channels\":1,\"unreadable\":0,\"found\":0,\"cleared\":0,\"remaining\":0}\n"
    );
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn audit_errors_print_only_their_classification() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let output = files
        .command("127.0.0.1:0", &cma, &nyxid, &provider)
        .env("CMAEG_CMA_BASE_URL", "http://127.0.0.1:9")
        .arg("--audit-custom-attachments")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.ends_with("record store unavailable\n"), "{stderr}");
    assert!(!stderr.contains("127.0.0.1:9"), "{stderr}");
    assert_no_secrets(&stderr);
    let output = files
        .command("127.0.0.1:0", &cma, &nyxid, &provider)
        .env_remove("CMAEG_MASTER_KEYS_FILE")
        .arg("--audit-custom-attachments")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    // Log lines may precede it; the error itself is the classification alone.
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr.lines().last(),
        Some("configuration invalid"),
        "{stderr}"
    );
    assert!(!stderr.contains("CMAEG_MASTER_KEYS_FILE"), "{stderr}");
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

/// `tools/list` of the gateway at `address`, through curl like an agent.
async fn listed_tools(address: &str) -> Vec<String> {
    let output = tokio::process::Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "5",
            "--request",
            "POST",
            "--header",
            "Content-Type: application/json",
            "--header",
            "Authorization: Bearer nyx_abcdefghijklmnop",
            "--data-binary",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &format!("http://{address}/mcp"),
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn compiled_binary_lists_the_attachment_tool_with_media_enabled() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let address = free_address();
    let mut command = files.command(&address, &cma, &nyxid, &provider);
    command.env("CMAEG_MEDIA_ENABLED", "true");
    let mut child = spawn_ready(command, &address, &files.dir.join("trace.log")).await;
    // Graduated message-effect and history gates add their tools by default.
    assert_eq!(
        listed_tools(&address).await,
        [
            "replyToEvent",
            "updateMessage",
            "sendTyping",
            "readEventContext",
            "readEventAttachment",
            "readFormPreparation",
            "readMessageCapabilities",
            "deleteMessage",
            "setMessageReaction",
            "readEventHistory"
        ]
    );
    let document = curl("GET", &format!("http://{address}/openapi.json"), None, 200).await;
    assert_eq!(
        document["paths"]["/events/{event_ref}/attachments/{index}"]["get"]["operationId"],
        "readEventAttachment"
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

/// The gateway makes itself non-dumpable before anything else, so a same-uid
/// process (a `kubectl exec`, a debugger) cannot read its secrets.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_running_gateway_is_not_dumpable() {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root bypasses the /proc ownership check");
        return;
    }
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let address = free_address();
    let log_path = files.dir.join("gateway.log");
    let command = files.command(&address, &cma, &nyxid, &provider);
    let mut child = spawn_ready(command, &address, &log_path).await;
    let pid = child.id().expect("the gateway is running");
    // The kernel leaves the world-readable `/proc/<pid>` directory with the
    // task's own uid; the files in it are what non-dumpability hands to root.
    let owner = std::fs::metadata(format!("/proc/{pid}/environ"))
        .unwrap()
        .uid();
    let environ = std::fs::File::open(format!("/proc/{pid}/environ"));
    child.kill().await.unwrap();
    assert_eq!(owner, 0, "/proc/{pid}/environ must be owned by root");
    assert_eq!(
        environ.map(drop).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
