use super::*;
use crate::store::agent_log_segments::{
    publication, writer_admission as admission, writer_bootstrap,
    writer_gate::{self as gate, slots},
};
use crate::store::sandbox_mongo::start_transaction;
use futures_util::TryStreamExt;
use layer2_core::sandbox::AgentLogKind;
use mongodb::bson::{DateTime, RawDocumentBuf};

async fn legacy_fixture() -> Fixture {
    let mut f = fixture(2).await;
    let mut kinds = vec![
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: Some("fixture-command".into()),
            model: Some("original-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
    ];
    kinds.extend((0..65).map(|_| AgentLogKind::AgentMessage {
        content: "fixture output".into(),
    }));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let rows = kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| {
            crate::store::agent_log::log_document(&AgentLogEntry {
                agent_id: f.agent.id.clone(),
                seq: offset as u64 + 3,
                appended_at: NOW.into(),
                kind,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    f.agent.conversation.last_seq += rows.len() as u64;
    f.agent.revision = 7;
    f.store.agent_log.insert_many(rows).await.unwrap();
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    collection(&f.store, HEADS)
        .delete_many(doc! {})
        .await
        .unwrap();
    collection(&f.store, PROJECTIONS)
        .delete_many(doc! {})
        .await
        .unwrap();
    f
}

async fn admitted(f: &Fixture) -> (PublicationIdentity, Admission, slots::Permit) {
    let db = f.client.database(&f.store.agents.namespace().db);
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    succeeded(
        gate::replace(
            &db,
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
    let operation = PublicationIdentity {
        key: "fixture-bootstrap".into(),
        appended_at: NOW.into(),
        fingerprint: publication::result_digest(&"fixture-original-request").unwrap(),
    };
    start_transaction(&mut session).await.unwrap();
    let admitted = succeeded(
        admission::admit(
            &db,
            &f.agent,
            &operation,
            admission::AdmissionKind::Activation,
            NOW,
            &mut session,
        )
        .await,
    );
    let permit = succeeded(
        slots::claim(
            &db,
            &f.agent,
            &operation,
            &admitted,
            DateTime::now(),
            &mut session,
        )
        .await,
    )
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    (operation, admitted, permit)
}

async fn raw_source(f: &Fixture) -> Vec<RawDocumentBuf> {
    f.store
        .agent_log
        .clone_with_type::<RawDocumentBuf>()
        .find(doc! {})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bootstrap_stages_multiple_fenced_batches_without_exposing_or_rewriting_source() {
    let f = legacy_fixture().await;
    let original = raw_source(&f).await;
    let (operation, admission, permit) = admitted(&f).await;
    let _plan = succeeded(
        writer_bootstrap::stage(&f.store, &f.agent, &operation, &admission, permit).await,
    );
    assert_eq!(raw_source(&f).await, original);
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 1);
    assert_eq!(crate::store::agents::read_agent(&row).unwrap(), f.agent);
    for name in [HEADS, SEGMENTS, PROJECTIONS, PUBLICATIONS] {
        assert_eq!(
            collection(&f.store, name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    let evidence: Vec<Document> = collection(&f.store, EVIDENCE)
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(evidence.len(), 67);
    let generation = evidence[0].get_str("evidence_generation").unwrap();
    for row in &evidence {
        assert_eq!(row.get_i64("head_revision").unwrap(), 1);
        assert_eq!(
            row.get_i64("through_seq").unwrap(),
            f.agent.conversation.last_seq as i64
        );
        assert_eq!(row.get_str("evidence_generation").unwrap(), generation);
    }
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bootstrap_rejects_replaced_permit_and_changed_original_request() {
    let f = legacy_fixture().await;
    let (operation, admission, original) = admitted(&f).await;
    let db = f.client.database(&f.store.agents.namespace().db);
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let replacement = succeeded(
        slots::claim(
            &db,
            &f.agent,
            &operation,
            &admission,
            DateTime::now(),
            &mut session,
        )
        .await,
    )
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    assert!(
        writer_bootstrap::stage(&f.store, &f.agent, &operation, &admission, original)
            .await
            .is_err()
    );
    assert_eq!(
        collection(&f.store, EVIDENCE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let mut different = operation.clone();
    different.appended_at = "2026-11-11T01:00:00Z".into();
    assert!(
        writer_bootstrap::stage(&f.store, &f.agent, &different, &admission, replacement)
            .await
            .is_err()
    );
    assert_eq!(
        collection(&f.store, EVIDENCE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.destroy().await;
}
