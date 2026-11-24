use super::*;
use crate::agent_sandbox::RetainedStartupPolicyUpgrade;

fn predecessor(fixture: &Resume) -> (String, String) {
    let state = fixture.fixture.state.lock().unwrap();
    let metadata = &state.rows[MEMBER]["metadata"];
    (
        metadata["uid"].as_str().unwrap().into(),
        metadata["resourceVersion"].as_str().unwrap().into(),
    )
}

async fn dispatch(
    fixture: &Resume,
    admission: &RetainedStartupPolicyUpgrade<'_>,
) -> crate::Result<()> {
    client(&fixture.fixture, true)
        .upgrade_retained_startup_policy(
            &fixture.handle,
            &fixture.previous,
            &fixture.home,
            admission,
        )
        .await
}

#[tokio::test]
async fn retained_policy_upgrade_rejects_a_changed_predecessor_version() {
    let fixture = Resume::new().await;
    let (uid, version) = predecessor(&fixture);
    let admission = RetainedStartupPolicyUpgrade {
        operation_id: "retained-policy-operation",
        predecessor_resource_version: &version,
        predecessor_uid: &uid,
        target_image: &fixture.previous.image.uri,
        successor_generation: 2,
        journal_recovery: true,
    };
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(MEMBER)
        .unwrap()["metadata"]["resourceVersion"] = json!("42");
    let error = dispatch(&fixture, &admission).await.unwrap_err();
    assert!(matches!(error, crate::AdapterError::Rejected(message)
        if message == "retained startup policy predecessor changed after admission"));
    fixture.no_writes();
}

#[tokio::test]
async fn retained_policy_upgrade_replays_an_exact_lost_ack_after_version_advance() {
    let fixture = Resume::new().await;
    let (uid, version) = predecessor(&fixture);
    let mut admission = RetainedStartupPolicyUpgrade {
        operation_id: "retained-policy-operation",
        predecessor_resource_version: &version,
        predecessor_uid: &uid,
        target_image: &fixture.previous.image.uri,
        successor_generation: 2,
        journal_recovery: true,
    };
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .lose
        .push(format!("PATCH {MEMBER}"));
    assert!(dispatch(&fixture, &admission).await.is_err());
    {
        let mut state = fixture.fixture.state.lock().unwrap();
        let row = state.rows.get_mut(MEMBER).unwrap();
        let proof: serde_json::Value = serde_json::from_str(
            row["metadata"]["annotations"]["cma/native-runtime-upgrade"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(proof["version"], 2);
        assert_eq!(
            proof["retained_policy"]["operation_id"],
            admission.operation_id
        );
        assert_eq!(proof["retained_policy"]["journal_recovery"], true);
        row["metadata"]["resourceVersion"] = json!("42");
        state.calls.clear();
    }
    dispatch(&fixture, &admission).await.unwrap();
    fixture.no_writes();
    admission.operation_id = "different-operation";
    assert!(dispatch(&fixture, &admission).await.is_err());
    fixture.no_writes();
}
