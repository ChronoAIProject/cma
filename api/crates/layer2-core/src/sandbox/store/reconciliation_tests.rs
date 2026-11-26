use super::*;
use crate::sandbox::lifecycle_test_support::{agent, sandbox, NOW};
use crate::sandbox::*;
use crate::{SandboxId, SubjectId};

#[test]
fn retirement_requires_native_finality_for_pending_restart_custody() {
    let mut parent = sandbox();
    parent.status = SandboxStatus::Retiring;
    let mut request = loss_request(&parent);
    for step in [
        AgentWorkStep::RestartConversation,
        AgentWorkStep::RestartTeardown,
    ] {
        let mut pending = agent("agt_restart", "conversation", 1);
        pending.work.step = step;
        request.native_retirement_finality = false;
        assert_eq!(
            precheck_agent_recovery(&parent, &pending, &request).unwrap(),
            Some(SandboxRuntimeLossOutcome::NotEligible),
        );
        request.native_retirement_finality = true;
        assert_eq!(
            precheck_agent_recovery(&parent, &pending, &request).unwrap(),
            None
        );
    }
}

fn loss_request(value: &Sandbox) -> SandboxRuntimeLossRequest {
    SandboxRuntimeLossRequest {
        sandbox_id: value.id.clone(),
        expected_revision: value.revision,
        expected_runtime: value.runtime.clone(),
        native_retirement_finality: false,
        retain_runtime: false,
        interruption_cause: Default::default(),
        observed_at: NOW.into(),
    }
}

use crate::sandbox::store::{
    apply_sandbox_runtime_loss, precheck_runtime_loss_agents, precheck_sandbox_runtime_loss,
    SandboxRuntimeLossApplication,
};

#[test]
fn sandbox_precheck_matches_full_application_for_every_row_refusal() {
    for case in [
        "eligible", "request", "identity", "revision", "runtime", "status", "updated", "invalid",
    ] {
        let mut row = sandbox();
        let mut request = loss_request(&row);
        match case {
            "request" => request.expected_revision = 0,
            "identity" => request.sandbox_id = SandboxId("sbx_other".into()),
            "revision" => request.expected_revision += 1,
            "runtime" => request.expected_runtime = None,
            "status" => {
                row.status = SandboxStatus::Paused;
                row.runtime = None;
                request.expected_runtime = None;
            }
            "updated" => request.observed_at = "2000-01-01T00:00:00Z".into(),
            "invalid" => row.revision = 0,
            _ => {}
        }
        let precheck = precheck_sandbox_runtime_loss(&row, &request);
        let full = apply_sandbox_runtime_loss(&row, &[], &request);
        match precheck {
            Ok(Some(expected)) => assert!(
                matches!(full, Ok(SandboxRuntimeLossApplication::Complete(actual)) if actual == expected),
                "{case}"
            ),
            Ok(None) => assert!(
                matches!(full, Ok(SandboxRuntimeLossApplication::Persist(_))),
                "{case}: {full:?}"
            ),
            Err(expected) => assert!(matches!(full, Err(actual) if actual == expected), "{case}"),
        }
    }
}

#[test]
fn agent_precheck_matches_full_application_without_changing_head_error_precedence() {
    let row = sandbox();
    let request = loss_request(&row);
    for case in [
        "eligible",
        "sandbox",
        "owner",
        "revision",
        "fence",
        "duplicate",
        "timestamp",
        "updated",
    ] {
        let mut value = agent("agt_check", "conversation", 1);
        match case {
            "sandbox" => value.sandbox_id = SandboxId("sbx_other".into()),
            "owner" => value.owner = SubjectId("other".into()),
            "revision" => value.revision = 0,
            "fence" => value.work.next_fence = 0,
            "timestamp" => value.updated_at = "invalid".into(),
            "updated" => value.updated_at = "2099-01-01T00:00:00Z".into(),
            _ => {}
        }
        let mut values = vec![value];
        if case == "duplicate" {
            values.push(values[0].clone());
        }
        let precheck = precheck_runtime_loss_agents(&row, &values, &request);
        let evidence = values
            .iter()
            .map(|a| (a.clone(), a.conversation.last_seq))
            .collect::<Vec<_>>();
        let full = apply_sandbox_runtime_loss(&row, &evidence, &request);
        match precheck {
            Ok(Some(expected)) => assert!(
                matches!(full, Ok(SandboxRuntimeLossApplication::Complete(actual)) if actual == expected),
                "{case}"
            ),
            Ok(None) => assert!(
                matches!(full, Ok(SandboxRuntimeLossApplication::Persist(_))),
                "{case}: {full:?}"
            ),
            Err(expected) => assert!(matches!(full, Err(actual) if actual == expected), "{case}"),
        }
    }
    let mut newer = agent("agt_check", "conversation", 1);
    newer.updated_at = "2099-01-01T00:00:00Z".into();
    assert!(matches!(
        apply_sandbox_runtime_loss(&row, &[(newer, 99)], &request),
        Err(SandboxStoreError::SequenceConflict)
    ));
}
