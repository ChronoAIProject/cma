//! The compiled gateway's routing rollback audit against the shared fakes.
use super::binary_support::{assert_no_secrets, MountedSecrets};
use super::*;
use cmaeg_core::{channel::ChannelDefinition, profile::canonical_json, seal::seal_blob};

const OWNER: &str = "binary-routing-owner-fixture";
const REVIEWER: &str = "csm_00000000000a";
const REVIEWER_KEY: &str = "nyx_fixture_binary_reviewer";
const DEFAULT_KEY: &str = "nyx_fixture_binary_default";

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run `cmaeg --audit-routing ...` against the fakes at `RUST_LOG=trace`,
/// asserting that neither stream carries a secret, key or owner subject.
async fn audit(
    files: &MountedSecrets,
    fakes: (&FakeCma, &FakeNyxid, &FakeProvider),
    arguments: &[&str],
    environment: &[(&str, &str)],
) -> Run {
    let (cma, nyxid, provider) = fakes;
    let mut command = files.command("127.0.0.1:0", cma, nyxid, provider);
    command
        .env("RUST_LOG", "trace")
        .env_remove("CMAEG_NON_HUMAN_PROVIDERS")
        .args(arguments);
    for (key, value) in environment {
        command.env(key, value);
    }
    let output = command.output().await.unwrap();
    let run = Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    };
    for text in [&run.stdout, &run.stderr] {
        assert_no_secrets(text);
        for leaked in [OWNER, REVIEWER_KEY, DEFAULT_KEY, "Zml4dHVyZQ=="] {
            assert!(!text.contains(leaked), "{leaked} leaked");
        }
    }
    run
}

/// `sha256:` and the lowercase hex digest of an agent key.
fn key_hash(key: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, key.as_bytes());
    let hex: String = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// The base custom channel, owned by [`OWNER`] and keyed by `DEFAULT_KEY`.
fn base(record_id: &str) -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["record_id"] = record_id.into();
    value["owner"]["subject"] = OWNER.into();
    value["profile_digest"] = profile_digest(&value["profile"]).into();
    value["secrets"] = json!({
        "agent_key_hash": key_hash(DEFAULT_KEY),
        "provider_credential": DEFAULT_KEY,
        "hmac_secrets": [["key-1", "Zml4dHVyZQ=="]]
    });
    value
}

fn profile_digest(profile: &serde_json::Value) -> String {
    serde_json::from_value::<cmaeg_core::profile::EventWorkerProfile>(profile.clone())
        .unwrap()
        .digest()
}

/// A channel routed to `default` alone, with one additional `cma_codex`
/// consumer. Its custom source needs `default` to answer non-human actors.
fn routed(record_id: &str) -> serde_json::Value {
    let mut value = base(record_id);
    let profile = value["profile"].clone();
    value["secrets"]["consumer_keys"] = json!([[REVIEWER, REVIEWER_KEY]]);
    value["routing"] = json!({
        "version": 1,
        "default_consumer": {"name": "lead"},
        "consumers": [{"id": REVIEWER, "name": "reviewer", "profile": profile,
            "profile_digest": profile_digest(&profile),
            "provider": {"slug": "cma_codex", "binding_id": "binding-reviewer", "level": "l1"},
            "agent_key_hash": key_hash(REVIEWER_KEY),
            "mentionable": false, "created_at": "2026-10-01T00:00:00Z"}],
        "default_targets": ["default"],
        "addressing": {"mentions": false, "command": false, "event_field": false,
            "unknown_target_notice": false}
    });
    value
}

/// A chat channel with a relay per platform in `platforms`; two or more
/// need the extended source rules.
fn relays(record_id: &str, platforms: &[&str]) -> serde_json::Value {
    let mut value = base(record_id);
    value["sources"] = platforms
        .iter()
        .enumerate()
        .map(|(index, platform)| {
            json!({"type": "nyxid_relay", "issuer": "https://nyx.example",
                "jwks_url": "https://nyx.example/.well-known/jwks.json", "key_id": "key-1",
                "route_ids": [format!("5b3f0c6e-1c1d-4d39-9a8e-2f5b6c7d8e9{index}")],
                "platform": platform, "bot_username": format!("{platform}_bot"),
                "admission": {"type": "all"}})
        })
        .collect();
    value["partition"] = json!({"version": 1, "key": "conversation"});
    value
}

/// Seal `value` with the mounted master key and store it at version 1.
fn seed(cma: &FakeCma, value: serde_json::Value) {
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    let blob = seal_blob(
        &key_ring(1),
        canonical_json(definition.to_blob_json()).as_bytes(),
    )
    .unwrap();
    cma.seed(definition.record_id.as_str(), OWNER, blob);
}

fn line(record_id: &str, extensions: &str, receives_all: bool, action: &str) -> String {
    let reason = match action {
        "found" | "cleared" => "null".to_owned(),
        reason => format!("\"{reason}\""),
    };
    let action = match action {
        "found" | "cleared" => action,
        _ => "blocked",
    };
    format!(
        "{{\"record_id\":\"{record_id}\",\"channel_hash\":null,\"extensions\":[\"{extensions}\"],\"default_receives_all\":{receives_all},\"action\":\"{action}\",\"reason\":{reason}}}"
    )
}

/// The stdout of a run: its finding lines in record order, then `summary`.
fn report(lines: &[(&str, String)], summary: &str) -> String {
    let mut lines = lines.to_vec();
    lines.sort();
    let mut stdout: String = lines.into_iter().map(|(_, line)| line + "\n").collect();
    stdout.push_str(summary);
    stdout.push('\n');
    stdout
}

/// The manual check on the local harness, end to end: a routed channel and a
/// multi-relay channel are reported (exit 1), the routed one is blocked
/// while clearing would widen `default`, then cleared with its consumer
/// unbound while the relays stay blocked (exit 1), and once their owner
/// detaches a relay the audit exits 0.
#[tokio::test]
async fn audit_reports_clears_and_blocks_through_the_compiled_binary() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let fakes = (&cma, &nyxid, &provider);
    let routed_id = uuid::Uuid::new_v4().to_string();
    let relays_id = uuid::Uuid::new_v4().to_string();
    seed(&cma, routed(&routed_id));
    seed(&cma, relays(&relays_id, &["lark", "slack"]));

    let found = audit(&files, fakes, &["--audit-routing"], &[]).await;
    assert_eq!(found.code, Some(1), "{}", found.stderr);
    assert_eq!(
        found.stdout,
        report(
            &[
                (&routed_id, line(&routed_id, "routing", true, "found")),
                (&relays_id, line(&relays_id, "sources", true, "found")),
            ],
            r#"{"scanned":2,"channels":2,"unreadable":0,"found":2,"cleared":0,"blocked":0,"remaining":2}"#
        )
    );
    assert!(!found.stderr.contains("CMAEG listening"));
    assert!(cma.records().values().all(|record| record.version == 1));
    assert!(provider.log().is_empty());

    // Without cma_codex in the non-human set, `default` does not answer the
    // custom source, so clearing would widen what it sees.
    let blocked = audit(
        &files,
        fakes,
        &["--audit-routing", "--clear"],
        &[("CMAEG_NON_HUMAN_PROVIDERS", "")],
    )
    .await;
    assert_eq!(blocked.code, Some(1), "{}", blocked.stderr);
    assert_eq!(
        blocked.stdout,
        report(
            &[
                (
                    &routed_id,
                    line(&routed_id, "routing", false, "default_would_widen")
                ),
                (
                    &relays_id,
                    line(&relays_id, "sources", true, "sources_need_owner")
                ),
            ],
            r#"{"scanned":2,"channels":2,"unreadable":0,"found":2,"cleared":0,"blocked":2,"remaining":2}"#
        )
    );
    assert!(cma.records().values().all(|record| record.version == 1));
    assert!(provider.log().is_empty());

    let cleared = audit(&files, fakes, &["--audit-routing", "--clear"], &[]).await;
    assert_eq!(cleared.code, Some(1), "{}", cleared.stderr);
    assert_eq!(
        cleared.stdout,
        report(
            &[
                (&routed_id, line(&routed_id, "routing", true, "cleared")),
                (
                    &relays_id,
                    line(&relays_id, "sources", true, "sources_need_owner")
                ),
            ],
            r#"{"scanned":2,"channels":2,"unreadable":0,"found":2,"cleared":1,"blocked":1,"remaining":1}"#
        )
    );
    assert!(cleared.stderr.contains("routing extensions cleared"));
    assert!(cleared.stderr.contains(&routed_id));
    assert_eq!(cma.records()[routed_id.as_str()].version, 2);
    assert_eq!(cma.records()[relays_id.as_str()].version, 1);
    let unbinds: Vec<_> = provider
        .requests()
        .into_iter()
        .map(|request| (request.method, request.path))
        .collect();
    assert_eq!(
        unbinds,
        [(
            "DELETE".to_owned(),
            "/api/v1/cmaeg/bindings/binding-reviewer".to_owned()
        )]
    );

    // The owner detaches the second relay.
    seed(&cma, relays(&relays_id, &["lark"]));
    let again = audit(&files, fakes, &["--audit-routing"], &[]).await;
    assert_eq!(again.code, Some(0), "{}", again.stderr);
    assert_eq!(
        again.stdout,
        "{\"scanned\":2,\"channels\":2,\"unreadable\":0,\"found\":0,\"cleared\":0,\"blocked\":0,\"remaining\":0}\n"
    );
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn an_unreadable_record_exits_one() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    cma.seed(
        &uuid::Uuid::new_v4().to_string(),
        OWNER,
        b"not a sealed blob".to_vec(),
    );
    for arguments in [&["--audit-routing"][..], &["--audit-routing", "--clear"]] {
        let run = audit(&files, (&cma, &nyxid, &provider), arguments, &[]).await;
        assert_eq!(run.code, Some(1), "{}", run.stderr);
        assert_eq!(
            run.stdout,
            "{\"scanned\":1,\"channels\":0,\"unreadable\":1,\"found\":0,\"cleared\":0,\"blocked\":0,\"remaining\":0}\n"
        );
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn an_invalid_non_human_provider_set_is_a_classified_stop() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let run = audit(
        &files,
        (&cma, &nyxid, &provider),
        &["--audit-routing"],
        &[("CMAEG_NON_HUMAN_PROVIDERS", "unconfigured_provider")],
    )
    .await;
    assert_eq!(run.code, Some(1));
    assert!(run.stdout.is_empty());
    assert_eq!(run.stderr.lines().last(), Some("configuration invalid"));
    assert!(
        !run.stderr.contains("unconfigured_provider"),
        "{}",
        run.stderr
    );
    assert_eq!(cma.lists.load(std::sync::atomic::Ordering::SeqCst), 0);
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
