//! Invoke the public store trait, including admission and transaction roots.
use super::*;
use crate::store::agent_log_segments::{publication, writer_gate as gate};
use crate::store::sandbox_mongo::start_transaction;
use layer2_core::sandbox::{
    store::{AgentLogPublicationIdentity, AgentLogStore},
    AgentLogKind,
};

fn identity(key: &str, kind: &AgentLogKind) -> AgentLogPublicationIdentity {
    AgentLogPublicationIdentity {
        key: key.into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn keyed_trait_replays_original_entry_after_later_append_and_rejects_changed_request() {
    let f = fixture(2).await;
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let original = identity("fixture-keyed-original", &kind);
    let first = f
        .store
        .append_agent_log_keyed(&f.agent.id, &original, kind.clone())
        .await
        .unwrap();
    let next = identity("fixture-keyed-later", &kind);
    let second = f
        .store
        .append_agent_log_keyed(&f.agent.id, &next, kind.clone())
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (3, 4));
    assert_eq!(
        f.store
            .append_agent_log_keyed(&f.agent.id, &original, kind.clone())
            .await
            .unwrap(),
        first
    );
    assert!(f
        .store
        .append_agent_log_keyed(
            &f.agent.id,
            &original,
            AgentLogKind::ConversationGap { frames: 2 }
        )
        .await
        .is_err());
    let mut changed = original.clone();
    changed.fingerprint[0] ^= 1;
    assert!(f
        .store
        .append_agent_log_keyed(&f.agent.id, &changed, kind)
        .await
        .is_err());
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 4);
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn keyed_trait_adopts_legacy_only_after_durable_gate_admission() {
    let f = fixture(2).await;
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    for name in [HEADS, PROJECTIONS] {
        collection(&f.store, name)
            .delete_many(doc! {})
            .await
            .unwrap();
    }
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let operation = identity("fixture-keyed-adoption", &kind);
    assert!(f
        .store
        .append_agent_log_keyed(&f.agent.id, &operation, kind.clone())
        .await
        .is_err());
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    for name in [HEADS, SEGMENTS, PROJECTIONS, PUBLICATIONS, EVIDENCE] {
        assert_eq!(
            collection(&f.store, name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    let mut session = transaction(&f).await;
    start_transaction(&mut session).await.unwrap();
    succeeded(
        gate::replace(
            &f.client.database(&f.store.agents.namespace().db),
            gate::GateName::Segments,
            0,
            gate::Policy {
                enabled: true,
                agents: vec![f.agent.id.to_string()],
                ..Default::default()
            },
            &mut session,
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let appended = f
        .store
        .append_agent_log_keyed(&f.agent.id, &operation, kind.clone())
        .await
        .unwrap();
    assert_eq!(appended.seq, 3);
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert_eq!(
        crate::store::agents::read_agent(&row)
            .unwrap()
            .conversation
            .last_seq,
        3
    );
    assert_eq!(
        collection(&f.store, SEGMENTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        f.store
            .append_agent_log_keyed(&f.agent.id, &operation, kind)
            .await
            .unwrap(),
        appended
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn forwarded_read_ports_preserve_predecessor_and_prove_sealed_points() {
    let f = fixture(10_000).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mutation = update(&plan);
    succeeded(persist(&f.store, &mut scope, plan, mutation).await);
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    let mut sequences = Vec::new();
    f.store
        .visit_agent_log_page_with_predecessor(&f.agent.id, 9999, 10001, 2, &mut |entry| {
            sequences.push(entry.seq);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(sequences, [9999, 10000, 10001]);
    assert_eq!(
        f.store
            .agent_log_entry(&f.agent.id, 1)
            .await
            .unwrap()
            .unwrap()
            .seq,
        1
    );
    assert!(f
        .store
        .agent_log_entry(&f.agent.id, 10002)
        .await
        .unwrap()
        .is_none());
    f.store
        .agent_log
        .update_one(
            doc! {"agent_id":f.agent.id.as_str(),"seq":1_i64},
            doc! {"$set":{"record.appended_at":"2026-11-11T00:00:01Z"}},
        )
        .await
        .unwrap();
    assert!(f.store.agent_log_entry(&f.agent.id, 1).await.is_err());
    f.destroy().await;
}
