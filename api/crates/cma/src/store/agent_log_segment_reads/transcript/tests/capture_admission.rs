use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn projection_capture_retains_head_authority_and_creation_refusals() {
    for fault in [
        "missing-head",
        "head-revision",
        "quarantine",
        "audit",
        "future-seal",
        "creation",
        "changed-agent",
        "format",
    ] {
        let (_, db, store, mut agent, entries) = fixture(0, false).await;
        let sequence = expected(&agent, &entries).turns[0]
            .final_answer()
            .unwrap()
            .sequence;
        let filter = doc! {"_id":agent.id.as_str()};
        match fault {
            "missing-head" => {
                store.segment_logs.heads.delete_one(filter).await.unwrap();
            }
            "head-revision" => {
                store
                    .segment_logs
                    .heads
                    .update_one(filter, doc! {"$inc":{"revision":1_i64}})
                    .await
                    .unwrap();
            }
            "quarantine" => {
                store
                    .agents
                    .update_one(
                        filter,
                        doc! {"$set":{
                            crate::store::agent_log_integrity::QUARANTINE_FIELD: {"format":1_i64}
                        }},
                    )
                    .await
                    .unwrap();
            }
            "audit" => {
                let row = store.agents.find_one(filter).await.unwrap().unwrap();
                let (incarnation, _) = segments::read_format(&row).unwrap().unwrap();
                store
                    .segment_logs
                    .audits
                    .insert_one(doc! {
                        "agent_id":agent.id.as_str(), "incarnation":incarnation, "status":"corrupt"
                    })
                    .bypass_document_validation(true)
                    .await
                    .unwrap();
            }
            "future-seal" => {
                let row = store.agents.find_one(filter).await.unwrap().unwrap();
                let (incarnation, _) = segments::read_format(&row).unwrap().unwrap();
                store
                    .segment_logs
                    .seals
                    .insert_one(doc! {
                        "agent_id":agent.id.as_str(), "incarnation":incarnation, "ordinal":0_i64
                    })
                    .bypass_document_validation(true)
                    .await
                    .unwrap();
            }
            "creation" => {
                store
                    .agent_log
                    .update_one(
                        doc! {"agent_id":agent.id.as_str(),"seq":1_i64},
                        doc! {"$set":{"record.kind.workspace_id":"wks_foreign"}},
                    )
                    .await
                    .unwrap();
            }
            "changed-agent" => agent.revision += 1,
            "format" => {
                store
                    .agents
                    .update_one(filter, doc! {"$set":{"agent_log_format":"unsupported"}})
                    .bypass_document_validation(true)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let usage = store.agent_log_usage_projection(&agent).await;
        let transcript = store.agent_log_transcript_projection(&agent).await;
        let page = store.agent_log_transcript_page(&agent, &first(4)).await;
        let answer = store.agent_log_final_answer(&agent, sequence).await;
        for error in [
            usage.map(|_| ()),
            transcript.map(|_| ()),
            page.map(|_| ()),
            answer.map(|_| ()),
        ] {
            assert!(
                matches!(
                    error,
                    Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict)
                ),
                "{fault}: {error:?}"
            );
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn legacy_projection_dispatch_keeps_existing_fallback() {
    let (_, db, store, agent, entries) = fixture(0, false).await;
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    // A legacy source must not depend on the segmented head at all.
    store.segment_logs.heads.delete_many(doc! {}).await.unwrap();
    assert_eq!(
        store.agent_log_transcript_projection(&agent).await.unwrap(),
        None
    );
    let sequence = expected(&agent, &entries).turns[0]
        .final_answer()
        .unwrap()
        .sequence;
    assert!(store
        .agent_log_final_answer(&agent, sequence)
        .await
        .unwrap()
        .is_none());
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    let expected_usage = crate::store::agent_opening_reads::usage(&store, &agent, &mut session)
        .await
        .unwrap();
    assert_eq!(
        super::super::super::projected_usage(&store, &agent, &mut session)
            .await
            .unwrap(),
        expected_usage
    );
    let request = first(4);
    let expected_page =
        crate::store::agent_opening_reads::transcript::page(&store, &agent, &mut session, &request)
            .await
            .unwrap();
    assert_eq!(
        page(&store, &agent, &mut session, &request).await.unwrap(),
        expected_page
    );
    db.drop().await.unwrap();
}
