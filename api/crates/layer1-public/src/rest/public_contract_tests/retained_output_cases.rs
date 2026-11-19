//! Retained-output discovery and chronology survive safe failure projection.
use super::{super::agents_tests, Scenario};
use crate::agent_v2::fixtures::{append, context, fixture};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{store::AgentStore, AgentLogKind, TypedFailure},
    AgentId, SubjectId,
};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("retained_discovery_")?;
    assert!(["owner", "shared", "public", "other"].contains(&mode));
    let f = fixture(true, false).await;
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    append(&f.store, AgentLogKind::TurnFailed {
        turn_sequence: agent.conversation.active_turn.unwrap(),
        failure: TypedFailure {
            class: if mode == "other" { "agent_turn_failed" } else { "agent_log_capacity_exceeded" }.into(),
            detail: if mode == "public" {
                "Durable Agent history reached its lifetime budget. Retained output and runtime identity are preserved. This Agent cannot continue under the current limits; inspect capacity before explicit recovery or replacement."
            } else {
                "This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export."
            }.into(),
        },
    }).await;
    let mut settled = f.store.get_agent(&agent.id).await.unwrap().unwrap();
    let revision = settled.revision;
    settled.conversation.active_turn = None;
    settled.revision += 1;
    f.store.save_agent(revision, settled).await.unwrap();
    if mode == "shared" {
        f.access
            .change_share(
                &context("owner-a"),
                Some("fixture-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent { agent_id: agent.id },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "retained-discovery-share".into(),
                },
            )
            .await
            .unwrap();
    }
    // All read routers share the same authority and log. Mutation ports are not used.
    let (mut conversation, _) = agents_tests::conversation_fixture().await;
    conversation.reads = f.reads();
    let mut rest = agents_tests::fixture().await.state;
    rest.reads = f.reads();
    let subject = if mode == "shared" {
        "reader"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    Some(Scenario {
        router: f
            .router
            .merge(crate::agui::router(conversation))
            .merge(super::super::agents::agent_router(rest))
            .layer(Extension(caller.clone()))
            .layer(Extension(crate::DelegatedCredential::new(
                "fixture-credential".into(),
            ))),
        subject: Some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}

#[test]
fn discovery_contract_preserves_redaction_and_legacy_envelopes() {
    let corpus: super::Corpus = serde_json::from_str(include_str!(
        "../../../tests/fixtures/public-contract/retained-discovery.json"
    ))
    .unwrap();
    for mode in ["owner", "shared", "public", "other"] {
        let body = |surface: &str| {
            corpus
                .cases
                .iter()
                .find(|case| case.id == format!("retained-discovery-{mode}-{surface}"))
                .unwrap()
                .response
                .body
                .clone()
        };
        let expected = if ["owner", "shared"].contains(&mode) {
            serde_json::json!({"available":true})
        } else {
            serde_json::Value::Null
        };
        let events = body("events");
        let failure = events["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["kind"]["type"] == "turn_failed")
            .unwrap();
        assert_eq!(failure["retained_output"], expected);
        let transcript = body("transcript");
        let turn = &transcript["turns"][0];
        assert_eq!(turn["retained_output"], expected);
        let response = body("response");
        assert_eq!(response["retained_output"], expected);
        let agui = body("agui");
        let error = agui
            .as_array()
            .unwrap()
            .iter()
            .find(|frame| frame["data"]["type"] == "RUN_ERROR")
            .unwrap();
        assert_eq!(error["data"]["cma:retainedOutput"], expected);
        let (class, detail) = if mode == "other" {
            ("agent_turn_failed", "The Agent run failed. The failed turn will not be replayed automatically. Earlier work may have completed; review the conversation before sending a new message to continue.")
        } else {
            ("agent_log_capacity_exceeded", "This Agent cannot continue because its conversation history is full. If retained output is available, contact support to recover it.")
        };
        let redacted = serde_json::json!({"class":class, "detail":detail});
        assert_eq!(failure["kind"]["failure"], redacted);
        assert_eq!(turn["outcome"]["failure"], redacted);
        assert_eq!(response["error"]["message"], detail);
        assert_eq!(error["data"]["message"], detail);
        let stream = body("response-stream");
        for (index, frame) in stream.as_array().unwrap().iter().enumerate() {
            assert_eq!(frame["id"], index.to_string());
            assert_eq!(frame["data"]["sequence_number"], index);
            assert_eq!(
                frame["data"]["response"]["retained_output"],
                if index == 2 {
                    expected.clone()
                } else {
                    serde_json::Value::Null
                }
            );
        }
        let tail = body("tail");
        let failed = tail
            .as_array()
            .unwrap()
            .iter()
            .find(|frame| frame["data"]["kind"]["type"] == "turn_failed")
            .unwrap();
        assert_eq!(failed["data"]["retained_output"], expected);
        let mut negotiated = body("snapshot");
        for entry in negotiated["items"].as_array_mut().unwrap() {
            if entry["kind"]["type"] == "turn_failed" {
                assert_eq!(entry["retained_output"], expected);
            } else {
                assert!(entry.get("retained_output").is_none());
            }
            entry.as_object_mut().unwrap().remove("retained_output");
        }
        assert_eq!(
            negotiated,
            body("legacy-snapshot"),
            "old strict consumers retain exact bytes"
        );
    }
}
