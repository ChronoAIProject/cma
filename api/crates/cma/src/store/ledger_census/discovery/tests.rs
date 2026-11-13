use super::*;

fn request() -> Request {
    Request {
        format: "runtime-ledger-discovery-request-v1".into(),
        after_agent_id: None,
    }
}

fn row(id: &str) -> Document {
    doc! {"_id": id, "document_schema_version": crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
    "sandbox_id": "private-parent", "status": "failed",
        "record": {"id":id, "sandbox_id":"private-parent", "status":"failed",
        "failure":{"class":"adapter", "detail":"runtime mutation journal contains too many entries"}}}
}

#[test]
fn discovery_rejects_foreign_requests_and_invalid_cutlines() {
    let mut value = request();
    value.validate().unwrap();
    for cursor in ["", " padded ", "control\ncharacter", &"x".repeat(257)] {
        value.after_agent_id = Some(cursor.into());
        assert!(value.validate().is_err());
    }
    value.after_agent_id = Some("agt_fixture".into());
    value.validate().unwrap();
    value.format = "unsupported".into();
    assert!(value.validate().is_err());
    assert!(serde_json::from_str::<Request>(
        r#"{"format":"runtime-ledger-discovery-request-v1","after_agent_id":null,"apply":true}"#
    )
    .is_err());
}

#[test]
fn discovery_accounts_for_every_row_without_granting_authority() {
    let candidate = row("a");
    let mut healthy = row("b");
    healthy.insert("status", "ready");
    healthy
        .get_document_mut("record")
        .unwrap()
        .insert("status", "ready");
    let mut unrelated = row("c");
    unrelated
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("failure")
        .unwrap()
        .insert("detail", "other failure");
    let mut unknown = row("d");
    unknown.insert("document_schema_version", 99);
    let page = classify(
        &request(),
        "a".repeat(64),
        vec![candidate, healthy, unrelated, unknown],
    )
    .unwrap();
    assert_eq!(page.scanned, 4);
    assert_eq!(page.candidates.agents.len(), 1);
    assert_eq!(page.candidates.agents[0].agent_id, "a");
    assert_eq!(page.non_failed, 1);
    assert_eq!(page.other_failed, 1);
    assert_eq!(page.unknown.len(), 1);
    assert_eq!(page.unknown[0].agent_id, "d");
    assert_eq!(page.unknown[0].reason, "unsupported_schema_preserve");
    assert!(page.canonical_id_range_exhausted);
    assert!(!page.writes && !page.recovery_authorized);
    page.candidates.validate().unwrap();
    let bytes = serde_json::to_vec(&page).unwrap();
    let output = summary(&page, &bytes);
    assert!(!output.to_string().contains("private-parent"));
    assert!(!output.to_string().contains("other failure"));
    assert!(output.get("next").is_none());
    assert_eq!(output["scanned"], 4);
    assert_eq!(output["writes"], false);
    assert_eq!(output["recovery_authorized"], false);
    assert_eq!(output["snapshot_scope"], "one_page_only");
}

#[test]
fn discovery_preserves_quarantine_projection_and_unreadable_failures() {
    for case in 0..8 {
        let mut source = row("private-agent");
        match case {
            0 => {
                source.insert(crate::store::agent_log_integrity::QUARANTINE_FIELD, false);
            }
            1 => {
                source.insert("status", "deleted");
            }
            2 => {
                source.insert("sandbox_id", "foreign");
            }
            3 => {
                source.insert("record", "unreadable");
            }
            4 => {
                source
                    .get_document_mut("record")
                    .unwrap()
                    .insert("failure", mongodb::bson::Bson::Null);
            }
            5 => {
                source
                    .get_document_mut("record")
                    .unwrap()
                    .get_document_mut("failure")
                    .unwrap()
                    .insert("detail", 7);
            }
            6 => {
                source.insert("status", "future");
                source
                    .get_document_mut("record")
                    .unwrap()
                    .insert("status", "future");
            }
            _ => {
                source
                    .get_document_mut("record")
                    .unwrap()
                    .insert("id", "foreign");
            }
        }
        let page = classify(&request(), "a".repeat(64), vec![source]).unwrap();
        assert!(page.candidates.agents.is_empty(), "case {case}");
        assert_eq!(page.unknown.len(), 1, "case {case}");
        assert_eq!(page.unknown[0].agent_id, "private-agent");
        assert!(page.unknown[0].reason.ends_with("preserve"));
        assert!(!summary(&page, b"artifact")
            .to_string()
            .contains("private-agent"));
    }
}

#[test]
fn discovery_resumes_only_after_the_last_observed_id() {
    let rows = (0..PAGE_SIZE)
        .map(|i| row(&format!("agt_{i:03}")))
        .collect();
    let page = classify(&request(), "a".repeat(64), rows).unwrap();
    assert_eq!(page.next.as_deref(), Some("agt_063"));
    assert!(!page.canonical_id_range_exhausted);
    let next = Request {
        after_agent_id: page.next,
        ..request()
    };
    let page = classify(&next, "b".repeat(64), vec![row("agt_064")]).unwrap();
    assert!(page.next.is_none());
    assert!(page.canonical_id_range_exhausted);
    assert!(classify(&next, "b".repeat(64), vec![row("agt_063")]).is_err());
    assert!(classify(&request(), "a".repeat(64), vec![row("b"), row("a")]).is_err());
    assert!(classify(&request(), "a".repeat(64), vec![row("a"), row("a")]).is_err());
    assert!(classify(&request(), "a".repeat(64), vec![row(" padded ")]).is_err());
    assert!(classify(
        &request(),
        "a".repeat(64),
        vec![row("a"); PAGE_SIZE as usize + 1]
    )
    .is_err());
}

#[test]
fn discovery_empty_page_is_explicitly_not_historical_cohort_proof() {
    let page = classify(&request(), "a".repeat(64), Vec::new()).unwrap();
    assert!(page.canonical_id_range_exhausted);
    assert_eq!(page.scanned, 0);
    assert_eq!(page.id_scope, "canonical_string_ids");
    assert_eq!(page.snapshot_scope, "one_page_only");
    assert!(!page.recovery_authorized);
}

#[tokio::test]
async fn discovery_dispatch_refuses_bad_arity_without_service_initialization() {
    for command in ["--ledger-census", "--ledger-failed-page"] {
        let arguments = vec![command.to_owned()];
        assert!(super::super::dispatch(&arguments).await.unwrap().is_err());
    }
    assert!(super::super::dispatch(&[]).await.is_none());
    assert!(super::super::dispatch(&["unrelated".into()])
        .await
        .is_none());
}
