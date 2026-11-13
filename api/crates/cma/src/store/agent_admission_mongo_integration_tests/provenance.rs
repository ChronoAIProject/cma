use super::*;
use layer2_core::resource_access::ResourceActor;
use layer2_core::sandbox::store::AgentProvenanceStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_concurrent_retry_records_one_original_creator() {
    let (client, database, store, _, _, mut request) = fixture("provenance_retry").await;
    request.created_by = Some(ResourceActor::Api {
        subject: SubjectId::new("actual-creator"),
        delegation_id: None,
    });
    let (left, right) = tokio::join!(
        store.admit_agent(request.clone()),
        store.admit_agent(request.clone())
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|v| matches!(v, AgentAdmissionResult::Created(_)))
            .count(),
        1
    );
    let id = &results[0].creation().agent.id;
    let before = store
        .agent_provenance_snapshot(id)
        .await
        .unwrap()
        .creation_provenance(id)
        .unwrap();
    assert_eq!(before.created_by, request.created_by);
    request.created_by = Some(ResourceActor::Human {
        subject: SubjectId::new("later-retry"),
    });
    assert!(matches!(
        store.admit_agent(request).await.unwrap(),
        AgentAdmissionResult::Existing(_)
    ));
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .agent_provenance_snapshot(id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        before
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_late_failure_and_aborted_creation_leave_no_facts() {
    let (client, database, store, parent, workspace, request) =
        fixture("provenance_rollback").await;
    store
        .agent_log
        .insert_one(log_document(&orphan(&request, 2)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.admit_agent(request.clone()).await.unwrap_err(),
        SandboxStoreError::SequenceConflict
    );
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    store.agent_log.delete_many(doc! {}).await.unwrap();
    let mut session = store.client.start_session().await.unwrap();
    let mut scope =
        crate::store::agent_log_segments::transaction_scope::TransactionScope::start(&mut session)
            .await
            .unwrap();
    assert!(
        super::super::apply_admission(&store, &request, false, None, &mut scope)
            .await
            .is_ok()
    );
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.get_agent(&request.agent_id).await.unwrap(), None);
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_retained_record_blocks_agent_id_recycling() {
    let (client, database, store, _, _, request) = fixture("provenance_recycle").await;
    let creation = store.admit_agent(request.clone()).await.unwrap();
    let id = &creation.creation().agent.id;
    let retained = store
        .agent_creation_provenance
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    store
        .agents
        .delete_one(doc! {"_id":id.as_str()})
        .await
        .unwrap();
    store
        .worktrees
        .delete_one(doc! {"_id":request.worktree_id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id":id.as_str()})
        .await
        .unwrap();
    let mut recycled = request.clone();
    recycled.creation_key = "different-creation".into();
    recycled.worktree_id = WorktreeId::generate();
    assert_eq!(
        store.admit_agent(recycled).await.unwrap_err(),
        SandboxStoreError::Duplicate
    );
    assert_eq!(
        store
            .agent_creation_provenance
            .find_one(doc! {"_id":id.as_str()})
            .await
            .unwrap(),
        Some(retained)
    );
    assert!(store.get_agent(id).await.unwrap().is_none());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_commit_reconciliation_requires_exact_creation_facts() {
    let (client, database, store, _, _, request) = fixture("provenance_reconcile").await;
    let mut session = store.client.start_session().await.unwrap();
    let mut scope =
        crate::store::agent_log_segments::transaction_scope::TransactionScope::start(&mut session)
            .await
            .unwrap();
    let receipt =
        match super::super::apply_admission(&store, &request, false, None, &mut scope).await {
            Ok(AdmissionReceipt::Created(receipt)) => receipt,
            _ => panic!("fresh creation expected"),
        };
    scope.release().commit_transaction().await.unwrap();
    assert!(
        reconcile_created(&store, &receipt, session.operation_time().unwrap())
            .await
            .unwrap()
    );
    store
        .agent_creation_provenance
        .update_one(
            doc! {"_id":request.agent_id.as_str()},
            doc! {"$set":{"record.created_by":{"kind":"human","subject":"corrupt-creator"}}},
        )
        .session(&mut session)
        .await
        .unwrap();
    assert_eq!(
        reconcile_created(&store, &receipt, session.operation_time().unwrap())
            .await
            .unwrap_err(),
        SandboxStoreError::SequenceConflict
    );
    store
        .agent_creation_provenance
        .delete_one(doc! {"_id":request.agent_id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    assert!(
        !reconcile_created(&store, &receipt, session.operation_time().unwrap())
            .await
            .unwrap()
    );
    client.database(&database).drop().await.unwrap();
}
