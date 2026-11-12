use super::*;
use crate::retirement_archive::ExclusionReceipt;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn database_time_refuses_expired_future_and_wrong_operation_release() {
    let (_temp, store, request, bytes) = super::super::tests::fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let authority = std::fs::read(&request.exclusion_receipt).unwrap();
    let now = store
        .client
        .database("admin")
        .run_command(doc! {"hello":1})
        .await
        .unwrap()
        .get_datetime("localTime")
        .unwrap()
        .timestamp_millis() as u64
        / 1000;
    for case in 0..4 {
        let mut proof: ExclusionReceipt = serde_json::from_slice(&authority).unwrap();
        let mut bound = request.clone();
        match case {
            0 => {
                proof.expires_at = layer2_core::rfc3339_at(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(now - 1),
                )
            }
            1 => {
                proof.qualified_at = layer2_core::rfc3339_at(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(now + 3600),
                )
            }
            2 => bound.operation_id = uuid::Uuid::new_v4().to_string(),
            _ => (),
        }
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let result = store
            .parents()
            .update_one(
                release_filter(&bound, &proof).unwrap(),
                doc! {"$inc":{"runtime_writer_claim_fence":1_i64}},
            )
            .session(&mut session)
            .await
            .unwrap();
        assert_eq!(result.matched_count, u64::from(case == 3));
        session.abort_transaction().await.unwrap();
    }
    assert!(
        read_sandbox(&store.inspect(&request.sandbox_id).await.unwrap())
            .unwrap()
            .retirement_archive_pending()
    );
    store.database.drop().await.unwrap();
}
