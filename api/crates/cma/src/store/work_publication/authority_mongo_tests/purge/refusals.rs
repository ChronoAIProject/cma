use super::*;
use mongodb::bson::Bson;

#[derive(Clone, Copy)]
enum Refusal {
    Revision,
    NotDue,
    Active,
    RevokeDebt,
    Archive,
    Ledger,
    Channel,
    Log,
    Allocation,
    Title,
    Fence,
}

async fn exercise_refusal(case: Refusal) {
    let p = PurgeFixture::new().await;
    let fields = match case {
        Refusal::Revision => doc! {},
        Refusal::NotDue => {
            doc! {"record.purge_after":"2099-01-01T00:00:00Z","purge_after":"2099-01-01T00:00:00Z"}
        }
        Refusal::Active => {
            doc! {"record.status":"active","status":"active","record.retired_at":Bson::Null,"retired_at":Bson::Null,"record.purge_after":Bson::Null,"purge_after":Bson::Null}
        }
        Refusal::RevokeDebt => doc! {"record.credential.revoke_debt":[{
        "provider_key_id":"synthetic-revocation","queued_at":MINUTE}]},
        Refusal::Archive => doc! {"record.lifecycle.retirement_archive":{
        "format":1_i32,"operation_id":uuid::Uuid::new_v4().to_string(),
        "plan_sha256":"a".repeat(64),"source_sha256":"b".repeat(64),
        "exclusion_sha256":"c".repeat(64),"verified":Bson::Null}},
        Refusal::Ledger => doc! {"ledger_collection_v1":Bson::Null},
        Refusal::Channel => doc! {"channel_storage_retention":Bson::Null},
        Refusal::Allocation => doc! {"record.runtime":{
        "runtime_id":"synthetic-runtime","runtime_version":"synthetic-image","bound_at":MINUTE},"runtime_version":"synthetic-image"},
        Refusal::Fence => doc! {"work_publication_fence":i64::MAX},
        Refusal::Log | Refusal::Title => doc! {},
    };
    if !fields.is_empty() {
        p.f.independent
            .sandboxes
            .update_one(doc! {"_id":p.claimed.id.as_str()}, doc! {"$set":fields})
            .await
            .unwrap();
    }
    if matches!(case, Refusal::Log | Refusal::Title) {
        let field = if matches!(case, Refusal::Log) {
            "agent_log_format"
        } else {
            "title_effect_custody"
        };
        p.f.independent
            .agents
            .update_one(
                doc! {"_id":format!("agent-{}",p.claimed.id)},
                doc! {"$set":{field:Bson::Null}},
            )
            .await
            .unwrap();
    }
    let before = p.state().await;
    p.f.reset_trace();
    let result = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(p.claimed.clone())),
        Instant::now(),
    )
    .run(
        p.f.store.purge_sandbox(
            &p.claimed.id,
            p.claimed.revision + u64::from(matches!(case, Refusal::Revision)),
            MINUTE,
        ),
        || async {
            p.f.independent
                .renew_sandbox_authority(
                    &p.claimed,
                    &now(),
                    &p.claimed.work.lease.as_ref().unwrap().expires_at,
                )
                .await
        },
    )
    .await;
    let after = p.state().await;
    let events = p.f.events.lock().unwrap().clone();
    let starts = p.f.starts();
    p.f.drop_database().await;
    assert!(
        matches!(result, Err(SandboxStoreError::Conflict)),
        "eligibility or custody refusal must remain terminal"
    );
    assert!(starts.len() <= 1, "domain refusals must not retry");
    assert!(!events
        .iter()
        .any(|event| matches!(event, CommandEvent::Started(event)
        if event.command_name == "commitTransaction")));
    assert!(
        before == after,
        "every child and custody record must remain unchanged"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn revision_mismatch() {
    exercise_refusal(Refusal::Revision).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn not_yet_due() {
    exercise_refusal(Refusal::NotDue).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn non_retired() {
    exercise_refusal(Refusal::Active).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn revoke_debt() {
    exercise_refusal(Refusal::RevokeDebt).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn archive_hold() {
    exercise_refusal(Refusal::Archive).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn ledger_retention() {
    exercise_refusal(Refusal::Ledger).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn channel_retention() {
    exercise_refusal(Refusal::Channel).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn segmented_log_retention() {
    exercise_refusal(Refusal::Log).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn outstanding_allocation() {
    exercise_refusal(Refusal::Allocation).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn title_custody() {
    exercise_refusal(Refusal::Title).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn zero_match_fence() {
    exercise_refusal(Refusal::Fence).await;
}
