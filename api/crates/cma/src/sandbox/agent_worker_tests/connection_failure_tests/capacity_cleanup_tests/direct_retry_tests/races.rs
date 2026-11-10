//! A selection observation cannot authorize changed authority or corrupt history.
use super::*;

fn refusal_envelope(mut expected: Document, quarantined: bool) -> Document {
    if quarantined {
        let head = expected.get_i64("last_seq").unwrap();
        // A failed transaction must preserve the complete domain envelope, but
        // proven retained-history corruption durably revokes its checkpoint.
        assert!(expected
            .insert(
                "agent_log_integrity_quarantine_v1",
                doc! {"format": 1_i64, "reason": "integrity_validation_failed",
                "observed_head": head},
            )
            .is_none());
    }
    expected
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn selected_retry_revalidates_revision_identity_work_lease_head_and_tail() {
    let f = Fixture::new().await;
    let (current, now) = f.marked_claim().await;
    let proposal = f.selected(&current, now).await;
    let db = f.client.database(&f.name);
    let agents = db.collection::<Document>("agents");
    let logs = db.collection::<Document>("agent_log");
    let agent_filter = doc! {"_id": current.id.as_str()};
    let tail_filter =
        doc! {"agent_id": current.id.as_str(), "seq": current.conversation.last_seq as i64};
    let original = agents
        .find_one(agent_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let tail = logs.find_one(tail_filter.clone()).await.unwrap().unwrap();
    for variant in 0..10 {
        let mut request = proposal.clone();
        let mut changed = current.clone();
        match variant {
            0 => {
                changed.revision += 1;
                agents
                    .update_one(
                        agent_filter.clone(),
                        doc! {"$set": {"revision": changed.revision as i64,
                        "record": serialize_to_bson(&changed).unwrap()}},
                    )
                    .await
                    .unwrap();
            }
            1 => request.holder = "different-worker".into(),
            2 => request.fence += 1,
            3 => request.agent.owner = SubjectId("different-owner".into()),
            4 => request.agent.work.step = AgentWorkStep::RemoveHome,
            5 => {
                request.appended_at = current.work.lease.as_ref().unwrap().expires_at.clone();
                request.agent.updated_at = request.appended_at.clone();
                request.agent.work.ready_at = rfc3339_at(now + Duration::from_secs(19 * 60));
            }
            6 => {
                changed.work.lease.as_mut().unwrap().holder = "new-holder".into();
                agents
                    .update_one(
                        agent_filter.clone(),
                        doc! {"$set": {"work_lease_holder": "new-holder",
                        "record": serialize_to_bson(&changed).unwrap()}},
                    )
                    .await
                    .unwrap();
            }
            7 => {
                let mut entry: AgentLogEntry =
                    mongodb::bson::deserialize_from_bson(tail.get("record").unwrap().clone())
                        .unwrap();
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut entry.kind {
                    failure.detail.push('!');
                }
                logs.update_one(
                    tail_filter.clone(),
                    doc! {"$set": {"record": serialize_to_bson(&entry).unwrap()}},
                )
                .await
                .unwrap();
            }
            8 => {
                logs.delete_one(tail_filter.clone()).await.unwrap();
            }
            _ => {
                // An append committed after selection changes both captured head
                // and revision. The selected old proposal must not settle it.
                changed.revision += 1;
                changed.conversation.last_seq += 1;
                let entry = AgentLogEntry {
                    agent_id: current.id.clone(),
                    seq: changed.conversation.last_seq,
                    appended_at: rfc3339_at(now),
                    kind: AgentLogKind::OwnerMessage {
                        content: "race fixture".into(),
                    },
                };
                logs.insert_one(doc! {"_id": format!("{}:{:020}", current.id, entry.seq),
                    "agent_id": current.id.as_str(), "seq": entry.seq as i64, "document_schema_version": 1_i64,
                    "record": serialize_to_bson(&entry).unwrap()}).await.unwrap();
                agents.update_one(agent_filter.clone(), doc! {"$set": {"revision": changed.revision as i64,
                    "last_seq": changed.conversation.last_seq as i64, "record": serialize_to_bson(&changed).unwrap()}}).await.unwrap();
            }
        }
        // All races occur after the exact marker observation and before the
        // production commit's fresh snapshot, with real Mongo records.
        let expected = agents
            .find_one(agent_filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(
            f.h.store.commit_agent_work(request).await.is_err(),
            "variant {variant}"
        );
        assert_eq!(
            agents
                .find_one(agent_filter.clone())
                .await
                .unwrap()
                .unwrap(),
            refusal_envelope(expected, matches!(variant, 7 | 8)),
            "variant {variant}"
        );
        agents
            .replace_one(agent_filter.clone(), original.clone())
            .await
            .unwrap();
        logs.delete_many(doc! {"agent_id": current.id.as_str(), "seq": {"$gte": current.conversation.last_seq as i64}}).await.unwrap();
        logs.insert_one(tail.clone()).await.unwrap();
    }
    f.h.store.commit_agent_work(proposal).await.unwrap();
    assert_retry(&current, &f.h.stored(&current.id).await, 30);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn selected_retry_still_checks_gaps_schema_extra_tail_and_physical_limits() {
    let f = Fixture::new().await;
    let (current, now) = f.marked_claim().await;
    let proposal = f.selected(&current, now).await;
    let db = f.client.database(&f.name);
    let agents = db.collection::<Document>("agents");
    let logs = db.collection::<Document>("agent_log");
    let agent_filter = doc! {"_id": current.id.as_str()};
    let original = agents
        .find_one(agent_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let middle_filter = doc! {"agent_id": current.id.as_str(), "seq": 5_000_i64};
    let middle = logs.find_one(middle_filter.clone()).await.unwrap().unwrap();
    for variant in 0..5 {
        match variant {
            0 => {
                logs.delete_one(middle_filter.clone()).await.unwrap();
            }
            1 => {
                logs.update_one(
                    middle_filter.clone(),
                    doc! {"$set": {"document_schema_version": 99_i64}},
                )
                .await
                .unwrap();
            }
            2 => {
                let entry = AgentLogEntry {
                    agent_id: current.id.clone(),
                    seq: current.conversation.last_seq + 1,
                    appended_at: rfc3339_at(now),
                    kind: AgentLogKind::OwnerMessage {
                        content: "extra tail fixture".into(),
                    },
                };
                logs.insert_one(doc! {"_id": format!("{}:{:020}", current.id, entry.seq),
                    "agent_id": current.id.as_str(), "seq": entry.seq as i64, "document_schema_version": 1_i64,
                    "record": serialize_to_bson(&entry).unwrap()}).await.unwrap();
            }
            3 => {
                let mut invalid = current.clone();
                invalid.conversation.last_seq = 10_129;
                agents
                    .update_one(
                        agent_filter.clone(),
                        doc! {"$set": {"last_seq": 10_129_i64,
                        "record": serialize_to_bson(&invalid).unwrap()}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                // Padding counts as consumed physical BSON even though it does
                // not alter the domain record. Spread 70 MiB across small writes
                // so fixture setup fits the runner's 0.25 GiB storage cache.
                for seq in 2..72 {
                    logs.update_one(
                        doc! {"agent_id": current.id.as_str(), "seq": seq as i64},
                        doc! {"$set": {"physical_padding": "x".repeat(1024 * 1024)}},
                    )
                    .await
                    .unwrap();
                }
            }
        }
        let expected = agents
            .find_one(agent_filter.clone())
            .await
            .unwrap()
            .unwrap();
        let result = f.h.store.commit_agent_work(proposal.clone()).await;
        assert!(result.is_err(), "variant {variant}");
        match variant {
            0 => assert_eq!(result, Err(SandboxStoreError::SequenceConflict)),
            1 => assert!(matches!(result, Err(SandboxStoreError::Corrupt(_)))),
            2 => assert_eq!(
                result,
                Err(SandboxStoreError::Corrupt(
                    "Agent log has rows beyond its trusted head".into()
                ))
            ),
            _ => assert_eq!(result, Err(SandboxStoreError::AgentLogCapacityExceeded)),
        }
        assert_eq!(
            agents
                .find_one(agent_filter.clone())
                .await
                .unwrap()
                .unwrap(),
            // Gaps, schema/extra-tail contradictions and retained physical
            // overflow all invalidate the checkpoint, without settling work.
            refusal_envelope(expected, true),
            "variant {variant}"
        );
        agents
            .replace_one(agent_filter.clone(), original.clone())
            .await
            .unwrap();
        logs.delete_one(middle_filter.clone()).await.unwrap();
        logs.insert_one(middle.clone()).await.unwrap();
        logs.delete_one(doc! {"agent_id": current.id.as_str(), "seq": (current.conversation.last_seq + 1) as i64})
            .await
            .unwrap();
        logs.update_many(
            doc! {"agent_id": current.id.as_str()},
            doc! {"$unset": {"physical_padding": ""}},
        )
        .await
        .unwrap();
    }
    f.h.store.commit_agent_work(proposal).await.unwrap();
    f.cleanup().await;
}
