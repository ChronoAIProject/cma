use super::*;
use crate::{RecoveryConversation, RuntimeHandle, RuntimeStatusRequest};

fn request() -> RuntimeRecoveryInspectionRequest {
    RuntimeRecoveryInspectionRequest {
        runtime: RuntimeHandle::new("agent-sandbox:original:uid"),
        identity: RuntimeStatusRequest {
            sandbox_id: format!("sbx_{}", "a".repeat(32)),
            generation: 3,
            home_identity: "b".repeat(43),
        },
        boot_id: "c".repeat(32),
        conversations: vec![RecoveryConversation {
            agent_id: format!("agt_{}", "d".repeat(32)),
            conversation_id: "019a2c89-0100-7000-8000-000000000001".into(),
        }],
    }
}

#[test]
fn recovery_history_requires_every_original_conversation_exactly_once() {
    let request = request();
    let history = json!({
        "agentId": request.conversations[0].agent_id,
        "conversationId": request.conversations[0].conversation_id,
        "historySha256": "f".repeat(64), "files": [],
    });
    assert!(checked_histories(&request, &json!([history.clone()])).is_ok());
    assert!(checked_histories(&request, &json!([])).is_err());
    assert!(checked_histories(&request, &json!([history.clone(), history.clone()])).is_err());
    for (key, value) in [
        ("agentId", json!(format!("agt_{}", "e".repeat(32)))),
        (
            "conversationId",
            json!("019a2c89-0100-7000-8000-000000000002"),
        ),
        ("historySha256", json!("f".repeat(63))),
        ("historySha256", json!("F".repeat(64))),
    ] {
        let mut changed = history.clone();
        changed[key] = value;
        assert!(checked_histories(&request, &json!([changed])).is_err());
    }
}

#[test]
fn recovery_inspection_cannot_authorize_a_signal_or_arbitrary_agent_path() {
    let request = request();
    let input = recovery_input(&request).unwrap();
    assert!(input.get("operationId").is_none());
    assert!(input.get("expectedPlan").is_none());
    assert_eq!(input["action"], "inspect");
    for bad in ["../other", "agt_invalid", "agt_/home", ""] {
        let mut invalid = request.clone();
        invalid.conversations[0].agent_id = bad.into();
        assert!(recovery_input(&invalid).is_err());
    }
    let mut duplicate = request.clone();
    duplicate
        .conversations
        .push(duplicate.conversations[0].clone());
    assert!(recovery_input(&duplicate).is_err());
    let mut changed = request;
    changed.boot_id = "c".repeat(31);
    assert!(recovery_input(&changed).is_err());
}

#[test]
fn recovery_plan_is_bounded_and_digest_covers_full_original_evidence() {
    assert!(plan_digest(&json!({})).is_err());
    assert!(plan_digest(&json!(null)).is_err());
    assert!(plan_digest(&json!({"overflow": "x".repeat(LIMIT)})).is_err());
    let plan = json!({"runtime": {"generation":3}, "history": "f".repeat(64)});
    let before = plan_digest(&plan).unwrap();
    let mut after = plan;
    after["runtime"]["generation"] = json!(4);
    assert_ne!(before, plan_digest(&after).unwrap());
}

#[test]
fn recovery_plan_digest_matches_provider_utf8_workspace_paths() {
    let plan = json!({"cwd": "/home/cma/研究", "record": "😀"});
    assert_eq!(
        plan_digest(&plan).unwrap(),
        "3bfcf358614050535610e2302cefbc84e5b6284e3df6695151e67d1703b7a0a4"
    );
}

#[test]
fn idle_reopen_cannot_substitute_the_weaker_inspection_result() {
    let request = request();
    let mut input = recovery_input(&request).unwrap();
    input["action"] = json!("inspect_idle");
    let mut plan = json!({
        "runtime": input["runtime"], "agents": input["agents"],
        "histories": [{"agentId": request.conversations[0].agent_id,
            "conversationId": request.conversations[0].conversation_id, "historySha256": "f".repeat(64)}]
    });
    let response = |plan: &Value| json!({"status":"inspected", "plan":plan, "planSha256":plan_digest(plan).unwrap()});
    assert!(checked_inspection(&request, &input, response(&plan)).is_err());
    plan["idleReopen"] = json!(true);
    assert!(checked_inspection(&request, &input, response(&plan)).is_err());
    plan["idleLedger"] = json!({"format":"legacy_v1_unused_process_scope", "sha256":"a".repeat(64), "journals":2, "authorities":2});
    assert!(checked_inspection(&request, &input, response(&plan)).is_ok());
    plan["idleLedger"]["authorities"] = json!(129);
    assert!(checked_inspection(&request, &input, response(&plan)).is_err());
}

#[test]
fn retained_idle_native_history_success_and_refusal_paths() {
    let test =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/codex/recovery_idle_test.py");
    let output = std::process::Command::new("python3")
        .arg("-B")
        .arg(test)
        .output()
        .expect("Python is required for pinned recovery evidence");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn recovery_ledger_refusals_have_fixed_content_free_categories() {
    for code in [
        "ledger_capacity",
        "ledger_unproved",
        "ledger_unsettled",
        "ledger_process_scope_used",
    ] {
        assert_eq!(provider_refusal(code), Reason::LedgerHeld);
    }
    assert_eq!(
        provider_refusal("ledger_format_unproved"),
        Reason::LedgerFormatUnproved
    );
    assert_eq!(provider_refusal("ledger_changed"), Reason::NativeChanged);
    assert_eq!(
        provider_refusal("unrecognized private detail"),
        Reason::Ineligible
    );
}
