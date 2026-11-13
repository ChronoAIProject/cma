use super::*;
use tracing::{instrument::WithSubscriber, Instrument};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

struct PendingMembership;

#[async_trait::async_trait]
impl OrganizationAuthority for PendingMembership {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if caller.subject.as_str() == "owner" {
            OrganizationAdmin
                .organization_context(caller, credential, organization)
                .await
        } else {
            std::future::pending().await
        }
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!("file reads do not list organizations")
    }
}

fn subscriber(output: Output, json: bool, filter: &str, disable_root: bool) -> tracing::Dispatch {
    // Exercise a filter that enables the event but rejects the safety span.
    let span_filter = tracing_subscriber::filter::filter_fn(move |metadata| {
        !disable_root || metadata.is_event() || metadata.name() == "private_parent"
    });
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new(filter))
        .with(span_filter);
    let layer = tracing_subscriber::fmt::layer()
        .without_time()
        .with_ansi(false)
        .with_writer(output);
    if json {
        tracing::Dispatch::new(subscriber.with(layer.json()))
    } else {
        tracing::Dispatch::new(subscriber.with(layer))
    }
}

async fn exercise(json: bool, filter: &str, enabled: bool, disable_root: bool) -> Vec<String> {
    let (service, access, owner) = fixture_with_authority(Arc::new(PendingMembership)).await;
    access
        .change_share(
            &owner,
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Sandbox {
                    sandbox_id: sandbox().id,
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId("organization".into()),
                    mode: ShareMode::OrgRoles,
                },
                expected_share_revision: None,
                idempotency_key: "timing-privacy".into(),
            },
        )
        .await
        .unwrap();
    let service = service.with_read_timings(enabled);
    let output = Output::default();
    let dispatch = subscriber(output.clone(), json, filter, disable_root);
    let diagnostic = crate::diagnostic_context::DiagnosticContext {
        route: "GET /api/v1/agents/{id}/files/content".into(),
        request_hash: "safe-file-request".into(),
        ..Default::default()
    };
    async {
        let private = tracing::warn_span!(target: "private_context", "private_parent", private_canary = "synthetic-private-parent");
        diagnostic.scope(async {
            let id = AgentId("agt_test".into());
            service.read(&owner.caller, Some(&owner), None, &id, "private-file.txt", 1024).await.unwrap();
            // Missing resource fails before membership lookup; an actual shared
            // read parks in membership IO and is then dropped by its caller.
            assert!(service.read(&owner.caller, Some(&owner), None, &AgentId("private-missing".into()), "private-file.txt", 1024).await.is_err());
            let recipient = context("private-recipient");
            assert!(tokio::time::timeout(std::time::Duration::from_millis(10), service.read(&recipient.caller, Some(&recipient), Some("private-credential"), &id, "private-file.txt", 1024)).await.is_err());
        }).instrument(private).await;
    }.with_subscriber(dispatch).await;
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    logs.lines()
        .filter(|line| line.contains("agent file read phase"))
        .map(str::to_owned)
        .collect()
}

fn assert_safe(lines: &[String], json: bool) {
    assert_eq!(lines.len(), 4, "{lines:?}");
    let mut outcomes = Vec::new();
    for line in lines {
        for private in [
            "private_canary",
            "synthetic-private-parent",
            "private-file",
            "private-recipient",
            "private-credential",
            "private-missing",
            "agt_test",
        ] {
            assert!(!line.contains(private), "{line}");
        }
        assert!(line.contains("safe-file-request"), "{line}");
        assert!(line.contains("elapsed_us"), "{line}");
        if json {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(event["span"], serde_json::json!({"name":"agent_file_read"}));
            assert_eq!(
                event["spans"],
                serde_json::json!([{"name":"agent_file_read"}])
            );
            assert_eq!(event["target"], "agent_file_read_phase");
            assert_eq!(
                event["fields"]["route"],
                "GET /api/v1/agents/{id}/files/content"
            );
            assert!(event["fields"]["elapsed_us"].is_u64());
            outcomes.push(event["fields"]["outcome"].as_str().unwrap().to_owned());
        } else {
            outcomes.push(
                ["completed", "error", "cancelled"]
                    .into_iter()
                    .find(|value| line.contains(&format!("outcome=\"{value}\"")))
                    .unwrap()
                    .to_owned(),
            );
        }
    }
    outcomes.sort();
    assert_eq!(outcomes, ["cancelled", "completed", "completed", "error"]);
}

#[tokio::test]
async fn actual_file_read_timing_json_never_inherits_private_spans() {
    assert_safe(&exercise(true, "info", true, false).await, true);
}

#[tokio::test]
async fn actual_file_read_timing_privacy_survives_filters_and_disabled_diagnostics() {
    for json in [false, true] {
        for filter in ["info", "warn,agent_file_read_phase=info"] {
            assert_safe(&exercise(json, filter, true, false).await, json);
            assert!(exercise(json, filter, false, false).await.is_empty());
        }
        for filter in ["warn", "info,agent_file_read_phase=off"] {
            assert!(exercise(json, filter, true, false).await.is_empty());
        }
        assert!(exercise(json, "info", true, true).await.is_empty());
    }
}
